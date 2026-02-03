//! PTY Manager for osh-daemon
//!
//! Manages pseudo-terminal processes using portable-pty.

use anyhow::{Context, Result};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tracing::{debug, error, info};

/// Events from PTY processes
#[derive(Debug)]
pub enum PtyEvent {
    /// Output data from process
    Data { req_id: String, data: Vec<u8> },
    /// Process exited
    Exit { req_id: String, exit_code: i32 },
    /// Error occurred
    Error { req_id: String, error: String },
}

/// Handle to a running PTY process (stored in sync Mutex for thread safety)
struct PtyProcess {
    /// Writer to send input to the process
    writer: Box<dyn Write + Send>,
    /// Child process handle
    child: Box<dyn portable_pty::Child + Send + Sync>,
    /// Master PTY handle for resize
    master: Box<dyn portable_pty::MasterPty + Send>,
}

/// Manages multiple PTY processes
pub struct PtyManager {
    /// Shell to use for spawning processes
    shell: String,
    /// Active processes by req_id (using std::sync::Mutex for Sync)
    processes: Arc<Mutex<HashMap<String, PtyProcess>>>,
    /// Channel to send events back to the daemon
    event_tx: mpsc::Sender<PtyEvent>,
}

impl PtyManager {
    /// Create a new PTY manager
    pub fn new(shell: String, event_tx: mpsc::Sender<PtyEvent>) -> Self {
        Self {
            shell,
            processes: Arc::new(Mutex::new(HashMap::new())),
            event_tx,
        }
    }
    
    /// Spawn a new process
    pub async fn spawn(
        &self,
        req_id: String,
        command: &str,
        cols: u16,
        rows: u16,
    ) -> Result<()> {
        let pty_system = native_pty_system();
        
        let pair = pty_system.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        }).context("Failed to open PTY")?;
        
        // Build command based on shell
        let mut cmd = CommandBuilder::new(&self.shell);
        
        #[cfg(target_os = "windows")]
        {
            if self.shell.to_lowercase().contains("powershell") {
                cmd.arg("-NoLogo");
                cmd.arg("-Command");
                cmd.arg(command);
            } else {
                // cmd.exe
                cmd.arg("/C");
                cmd.arg(command);
            }
        }
        
        #[cfg(not(target_os = "windows"))]
        {
            cmd.arg("-c");
            cmd.arg(command);
        }
        
        // Spawn the process
        let child = pair.slave.spawn_command(cmd)
            .context("Failed to spawn process")?;
        
        // Get reader and writer from master
        let reader = pair.master.try_clone_reader()
            .context("Failed to clone reader")?;
        let mut writer = pair.master.take_writer()
            .context("Failed to take writer")?;
        
        // On Windows, disable problematic terminal modes that PowerShell enables
        // These sequences cause issues when the output is displayed on Linux terminals
        #[cfg(target_os = "windows")]
        {
            // Disable Win32 Input Mode and Focus Reporting
            // \x1b[?9001l - Disable Win32 Input Mode
            // \x1b[?1004l - Disable Focus Reporting
            let disable_modes = b"\x1b[?9001l\x1b[?1004l";
            let _ = writer.write_all(disable_modes);
            let _ = writer.flush();
        }
        
        // Store the process
        {
            let mut processes = self.processes.lock().unwrap();
            processes.insert(req_id.clone(), PtyProcess {
                writer,
                child,
                master: pair.master,
            });
        }
        
        // Spawn thread to read output
        let event_tx = self.event_tx.clone();
        let processes = Arc::clone(&self.processes);
        let req_id_clone = req_id.clone();
        let runtime = tokio::runtime::Handle::current();
        
        std::thread::spawn(move || {
            Self::reader_thread(reader, req_id_clone, event_tx, processes, runtime);
        });
        
        info!("Spawned process for req_id={}, command={}", req_id, command);
        Ok(())
    }
    
    /// Reader thread that handles PTY output and process exit detection
    fn reader_thread(
        mut reader: Box<dyn Read + Send>,
        req_id: String,
        event_tx: mpsc::Sender<PtyEvent>,
        processes: Arc<Mutex<HashMap<String, PtyProcess>>>,
        runtime: tokio::runtime::Handle,
    ) {
        let mut buf = [0u8; 4096];
        let mut consecutive_eofs = 0;
        
        loop {
            // First, check if process has exited
            let exit_status = {
                let mut procs = processes.lock().unwrap();
                if let Some(process) = procs.get_mut(&req_id) {
                    match process.child.try_wait() {
                        Ok(Some(status)) => Some(status.exit_code() as i32),
                        Ok(None) => None, // Still running
                        Err(e) => {
                            error!("Error checking process status: {}", e);
                            None
                        }
                    }
                } else {
                    // Process was removed (killed externally)
                    debug!("Process {} not found in map, exiting reader", req_id);
                    return;
                }
            };
            
            if let Some(exit_code) = exit_status {
                // Process exited, remove from map
                {
                    let mut procs = processes.lock().unwrap();
                    procs.remove(&req_id);
                }
                info!("Process {} exited with code {}", req_id, exit_code);
                let _ = runtime.block_on(event_tx.send(PtyEvent::Exit {
                    req_id,
                    exit_code,
                }));
                return;
            }
            
            // Try to read from PTY
            match reader.read(&mut buf) {
                Ok(0) => {
                    // EOF - might mean process ended, wait a bit and check
                    consecutive_eofs += 1;
                    debug!("PTY EOF #{} for {}", consecutive_eofs, req_id);
                    
                    if consecutive_eofs >= 10 {
                        // Many consecutive EOFs, process probably ended
                        // Check one more time for exit status
                        let exit_code = {
                            let mut procs = processes.lock().unwrap();
                            if let Some(process) = procs.get_mut(&req_id) {
                                // Wait a bit for process to finish
                                std::thread::sleep(std::time::Duration::from_millis(100));
                                match process.child.try_wait() {
                                    Ok(Some(status)) => {
                                        procs.remove(&req_id);
                                        Some(status.exit_code() as i32)
                                    }
                                    Ok(None) => {
                                        // Process still running but not producing output
                                        // This shouldn't happen, but let's continue
                                        debug!("Process {} still running after EOF, continuing", req_id);
                                        None
                                    }
                                    Err(_) => {
                                        procs.remove(&req_id);
                                        Some(0)
                                    }
                                }
                            } else {
                                Some(-1)
                            }
                        };
                        
                        if let Some(code) = exit_code {
                            info!("Process {} finished with code {}", req_id, code);
                            let _ = runtime.block_on(event_tx.send(PtyEvent::Exit {
                                req_id,
                                exit_code: code,
                            }));
                            return;
                        }
                    }
                    
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    continue;
                }
                Ok(n) => {
                    consecutive_eofs = 0; // Reset EOF counter
                    let data = buf[..n].to_vec();
                    debug!("Read {} bytes from PTY for {}", n, req_id);
                    let _ = runtime.block_on(event_tx.send(PtyEvent::Data {
                        req_id: req_id.clone(),
                        data,
                    }));
                }
                Err(e) => {
                    // On Windows, ERROR_BROKEN_PIPE (109) means process ended
                    #[cfg(target_os = "windows")]
                    {
                        if e.raw_os_error() == Some(109) {
                            debug!("PTY pipe broken for {} (process ended)", req_id);
                            // Get exit code
                            let exit_code = {
                                let mut procs = processes.lock().unwrap();
                                if let Some(process) = procs.get_mut(&req_id) {
                                    std::thread::sleep(std::time::Duration::from_millis(50));
                                    let code = match process.child.try_wait() {
                                        Ok(Some(status)) => status.exit_code() as i32,
                                        _ => 0,
                                    };
                                    procs.remove(&req_id);
                                    code
                                } else {
                                    0
                                }
                            };
                            info!("Process {} finished with exit code: {}", req_id, exit_code);
                            let _ = runtime.block_on(event_tx.send(PtyEvent::Exit {
                                req_id,
                                exit_code,
                            }));
                            return;
                        }
                    }
                    
                    // WouldBlock is expected for non-blocking reads
                    if e.kind() == std::io::ErrorKind::WouldBlock {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                        continue;
                    }
                    
                    error!("PTY read error for {}: {}", req_id, e);
                    // Remove process from map
                    {
                        let mut procs = processes.lock().unwrap();
                        procs.remove(&req_id);
                    }
                    let _ = runtime.block_on(event_tx.send(PtyEvent::Error {
                        req_id,
                        error: e.to_string(),
                    }));
                    return;
                }
            }
        }
    }
    
    /// Send input data to a process
    pub async fn send_input(&self, req_id: &str, data: &[u8]) -> Result<()> {
        let mut processes = self.processes.lock().unwrap();
        if let Some(process) = processes.get_mut(req_id) {
            process.writer.write_all(data)
                .context("Failed to write to PTY")?;
            process.writer.flush()
                .context("Failed to flush PTY")?;
            Ok(())
        } else {
            anyhow::bail!("Process not found: {}", req_id)
        }
    }
    
    /// Resize a process's terminal
    pub async fn resize(&self, req_id: &str, cols: u16, rows: u16) -> Result<()> {
        let processes = self.processes.lock().unwrap();
        if let Some(process) = processes.get(req_id) {
            process.master.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            }).context("Failed to resize PTY")?;
            Ok(())
        } else {
            anyhow::bail!("Process not found: {}", req_id)
        }
    }
    
    /// Kill a specific process
    pub async fn kill(&self, req_id: &str) -> Result<()> {
        let mut processes = self.processes.lock().unwrap();
        if let Some(mut process) = processes.remove(req_id) {
            // Kill the child process
            let _ = process.child.kill();
            info!("Killed process: {}", req_id);
            Ok(())
        } else {
            anyhow::bail!("Process not found: {}", req_id)
        }
    }
    
    /// Kill all running processes
    pub async fn kill_all(&self) {
        let mut processes = self.processes.lock().unwrap();
        for (req_id, mut process) in processes.drain() {
            let _ = process.child.kill();
            info!("Killed process: {}", req_id);
        }
    }
    
    /// Check if a process exists
    #[allow(dead_code)]
    pub async fn has_process(&self, req_id: &str) -> bool {
        let processes = self.processes.lock().unwrap();
        processes.contains_key(req_id)
    }
    
    /// Get number of active processes
    #[allow(dead_code)]
    pub async fn process_count(&self) -> usize {
        let processes = self.processes.lock().unwrap();
        processes.len()
    }
}
