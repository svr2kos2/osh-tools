//! PTY Manager for osh-daemon
//!
//! Manages pseudo-terminal processes using portable-pty.

use anyhow::{Context, Result};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

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
    /// Master PTY handle for resize
    master: Box<dyn portable_pty::MasterPty + Send>,
    /// Flag to signal that the process has exited (set by waiter thread)
    exited: Arc<AtomicBool>,
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
        
        // Create exit flag for synchronization between waiter and reader threads
        let exited = Arc::new(AtomicBool::new(false));
        
        // Store the process
        {
            let mut processes = self.processes.lock().unwrap();
            processes.insert(req_id.clone(), PtyProcess {
                writer,
                master: pair.master,
                exited: Arc::clone(&exited),
            });
        }
        
        // Spawn waiter thread to detect process exit reliably
        // This thread blocks on child.wait() which is the most reliable way to detect exit
        let waiter_event_tx = self.event_tx.clone();
        let waiter_processes = Arc::clone(&self.processes);
        let waiter_req_id = req_id.clone();
        let waiter_exited = Arc::clone(&exited);
        let waiter_runtime = tokio::runtime::Handle::current();
        
        std::thread::spawn(move || {
            Self::waiter_thread(child, waiter_req_id, waiter_event_tx, waiter_processes, waiter_exited, waiter_runtime);
        });
        
        // Spawn reader thread to handle PTY output
        let reader_event_tx = self.event_tx.clone();
        let reader_processes = Arc::clone(&self.processes);
        let reader_req_id = req_id.clone();
        let reader_exited = Arc::clone(&exited);
        let reader_runtime = tokio::runtime::Handle::current();
        
        std::thread::spawn(move || {
            Self::reader_thread(reader, reader_req_id, reader_event_tx, reader_processes, reader_exited, reader_runtime);
        });
        
        info!("Spawned process for req_id={}, command={}", req_id, command);
        Ok(())
    }
    
    /// Waiter thread that blocks on child.wait() to reliably detect process exit
    /// This is the primary mechanism for detecting process exit
    fn waiter_thread(
        mut child: Box<dyn portable_pty::Child + Send + Sync>,
        req_id: String,
        event_tx: mpsc::Sender<PtyEvent>,
        processes: Arc<Mutex<HashMap<String, PtyProcess>>>,
        exited: Arc<AtomicBool>,
        runtime: tokio::runtime::Handle,
    ) {
        debug!("Waiter thread started for req_id={}", req_id);
        
        // Block until process exits - this is the most reliable way
        let exit_code = match child.wait() {
            Ok(status) => {
                let code = status.exit_code() as i32;
                info!("Process {} exited with code {} (detected by waiter)", req_id, code);
                code
            }
            Err(e) => {
                warn!("Error waiting for process {}: {}, assuming exit code 0", req_id, e);
                0
            }
        };
        
        // Mark as exited so reader thread knows to stop
        exited.store(true, Ordering::SeqCst);
        
        // Remove from processes map
        {
            let mut procs = processes.lock().unwrap();
            procs.remove(&req_id);
        }
        
        // Send exit event
        let _ = runtime.block_on(event_tx.send(PtyEvent::Exit {
            req_id: req_id.clone(),
            exit_code,
        }));
        
        debug!("Waiter thread finished for req_id={}", req_id);
    }
    
    /// Reader thread that handles PTY output
    /// This thread reads output and forwards it, but does NOT handle exit detection
    fn reader_thread(
        mut reader: Box<dyn Read + Send>,
        req_id: String,
        event_tx: mpsc::Sender<PtyEvent>,
        processes: Arc<Mutex<HashMap<String, PtyProcess>>>,
        exited: Arc<AtomicBool>,
        runtime: tokio::runtime::Handle,
    ) {
        debug!("Reader thread started for req_id={}", req_id);
        let mut buf = [0u8; 4096];
        let mut consecutive_errors = 0;
        
        loop {
            // Check if process has exited (set by waiter thread)
            if exited.load(Ordering::SeqCst) {
                debug!("Reader thread for {} detected exit flag, stopping", req_id);
                break;
            }
            
            // Check if process was removed from map (e.g., killed externally)
            {
                let procs = processes.lock().unwrap();
                if !procs.contains_key(&req_id) {
                    debug!("Process {} not found in map, reader exiting", req_id);
                    break;
                }
            }
            
            // Try to read from PTY
            match reader.read(&mut buf) {
                Ok(0) => {
                    // EOF - process likely ended, wait a bit then check exit flag
                    debug!("PTY EOF for {}", req_id);
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    
                    // If exited flag is set, waiter thread has handled exit
                    if exited.load(Ordering::SeqCst) {
                        debug!("Reader thread for {} saw EOF and exit flag, stopping", req_id);
                        break;
                    }
                    
                    // Otherwise, keep trying for a bit in case there's more data
                    consecutive_errors += 1;
                    if consecutive_errors >= 20 {
                        // After 1 second of EOFs, check if we should give up
                        debug!("Reader thread for {} hit EOF limit, checking if still valid", req_id);
                        if exited.load(Ordering::SeqCst) {
                            break;
                        }
                        consecutive_errors = 0; // Reset and keep waiting for waiter
                    }
                    continue;
                }
                Ok(n) => {
                    consecutive_errors = 0;
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
                            // Waiter thread will handle the exit event
                            // Just wait for the exit flag
                            for _ in 0..20 {
                                if exited.load(Ordering::SeqCst) {
                                    break;
                                }
                                std::thread::sleep(std::time::Duration::from_millis(50));
                            }
                            break;
                        }
                    }
                    
                    // WouldBlock is expected for non-blocking reads
                    if e.kind() == std::io::ErrorKind::WouldBlock {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                        continue;
                    }
                    
                    // Other errors - log and check exit flag
                    consecutive_errors += 1;
                    if consecutive_errors >= 5 {
                        error!("PTY read error for {}: {} (consecutive: {})", req_id, e, consecutive_errors);
                        // Wait for waiter thread to handle exit
                        for _ in 0..20 {
                            if exited.load(Ordering::SeqCst) {
                                break;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(50));
                        }
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }
        }
        
        debug!("Reader thread finished for req_id={}", req_id);
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
        if let Some(process) = processes.remove(req_id) {
            // Set exited flag to signal threads to stop
            process.exited.store(true, Ordering::SeqCst);
            info!("Killed process: {}", req_id);
            Ok(())
        } else {
            anyhow::bail!("Process not found: {}", req_id)
        }
    }
    
    /// Kill all running processes
    pub async fn kill_all(&self) {
        let mut processes = self.processes.lock().unwrap();
        for (req_id, process) in processes.drain() {
            process.exited.store(true, Ordering::SeqCst);
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
