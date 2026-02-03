//! OSH CLI
//!
//! Command execution tool for remote devices.
//!
//! Usage: osh <device_id|alias> "command"
//!
//! Responsibilities:
//! - Connect to Relay Service via WebSocket (ws://127.0.0.1:8081)
//! - Send EXEC requests and forward stdin
//! - Display remote stdout/stderr
//! - Handle heartbeat (1s ping, 2s timeout)

use anyhow::{Context, Result};
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use std::io::{Read, Write};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, trace, Level};
use tracing_subscriber::FmtSubscriber;

use osh_cli::protocol::{decode_payload, timestamp_ms, ExecRequest, InputMessage, RelayResponse};

#[cfg(unix)]
use std::os::unix::io::AsRawFd;

/// OSH CLI - Remote command execution
#[derive(Parser)]
#[command(name = "osh")]
#[command(author, version, about = "Execute commands on remote devices")]
struct Cli {
    /// Device ID or alias (supports prefix match)
    device: String,
    
    /// Command to execute
    command: String,
    
    /// Relay service WebSocket URL
    #[arg(long, default_value = "ws://127.0.0.1:8081")]
    relay_url: String,
    
    /// Enable PTY mode (interactive terminal with escaped control sequences)
    #[arg(long)]
    pty: bool,
    
    /// Enable verbose output
    #[arg(short, long)]
    verbose: bool,
}

/// Get terminal size (cols, rows)
fn get_terminal_size() -> (u16, u16) {
    // Try to get terminal size, default to 80x24
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        unsafe {
            let mut size: libc::winsize = std::mem::zeroed();
            if libc::ioctl(std::io::stdout().as_raw_fd(), libc::TIOCGWINSZ, &mut size) == 0 {
                return (size.ws_col, size.ws_row);
            }
        }
    }
    
    #[cfg(windows)]
    {
        // Use Windows API to get console size
        use windows_sys::Win32::System::Console::{
            GetConsoleScreenBufferInfo, GetStdHandle, CONSOLE_SCREEN_BUFFER_INFO,
            STD_OUTPUT_HANDLE,
        };
        
        unsafe {
            let handle = GetStdHandle(STD_OUTPUT_HANDLE);
            if !handle.is_null() && handle != -1isize as _ {
                let mut info: CONSOLE_SCREEN_BUFFER_INFO = std::mem::zeroed();
                if GetConsoleScreenBufferInfo(handle, &mut info) != 0 {
                    let cols = (info.srWindow.Right - info.srWindow.Left + 1) as u16;
                    let rows = (info.srWindow.Bottom - info.srWindow.Top + 1) as u16;
                    return (cols, rows);
                }
            }
        }
    }
    
    (80, 24)
}

/// Terminal raw mode guard for Unix
#[cfg(unix)]
struct RawModeGuard {
    original_termios: libc::termios,
    fd: i32,
}

#[cfg(unix)]
impl RawModeGuard {
    fn new() -> Option<Self> {
        unsafe {
            let fd = std::io::stdin().as_raw_fd();
            let mut termios: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(fd, &mut termios) != 0 {
                return None;
            }
            let original = termios;
            
            // Set raw mode
            libc::cfmakeraw(&mut termios);
            
            if libc::tcsetattr(fd, libc::TCSANOW, &termios) != 0 {
                return None;
            }
            
            Some(Self {
                original_termios: original,
                fd,
            })
        }
    }
}

#[cfg(unix)]
impl Drop for RawModeGuard {
    fn drop(&mut self) {
        unsafe {
            libc::tcsetattr(self.fd, libc::TCSANOW, &self.original_termios);
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    
    // Initialize logging
    let log_level = if cli.verbose { Level::DEBUG } else { Level::WARN };
    let _subscriber = FmtSubscriber::builder()
        .with_max_level(log_level)
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();
    
    // Run the main execution loop
    let exit_code = run_exec(&cli).await?;
    
    std::process::exit(exit_code);
}

async fn run_exec(cli: &Cli) -> Result<i32> {
    let req_id = uuid::Uuid::new_v4().to_string();
    let (cols, rows) = get_terminal_size();
    
    debug!("Connecting to Relay Service at {}...", cli.relay_url);
    debug!("Request ID: {}", req_id);
    debug!("Terminal size: {}x{}", cols, rows);
    debug!("PTY mode: {}", cli.pty);
    
    // Enable raw mode for PTY mode on Unix
    #[cfg(unix)]
    let _raw_guard = if cli.pty {
        RawModeGuard::new()
    } else {
        None
    };
    
    // Connect to Relay Service
    let (ws_stream, _) = tokio::time::timeout(
        Duration::from_secs(5),
        connect_async(&cli.relay_url)
    )
    .await
    .context("Connection timeout")?
    .context("Failed to connect to Relay Service")?;
    
    let (mut ws_write, mut ws_read) = ws_stream.split();
    
    // Send EXEC request
    let exec_request = ExecRequest::new(
        cli.device.clone(),
        req_id.clone(),
        cli.command.clone(),
        cols,
        rows,
        cli.pty,
    );
    let json = serde_json::to_string(&exec_request)?;
    debug!("Sending EXEC request: {}", json);
    ws_write.send(Message::Binary(json.into_bytes().into())).await?;
    
    // Create channel for stdin
    let (stdin_tx, mut stdin_rx) = mpsc::channel::<Vec<u8>>(32);
    
    // Clone req_id for use in tasks
    let req_id_for_stdin = req_id.clone();
    let req_id_for_ws = req_id.clone();
    
    // Spawn stdin reader task (blocking in separate thread)
    let stdin_handle = tokio::task::spawn_blocking(move || {
        let stdin = std::io::stdin();
        let mut stdin_lock = stdin.lock();
        let mut buffer = [0u8; 1024];
        
        loop {
            match stdin_lock.read(&mut buffer) {
                Ok(0) => {
                    // EOF
                    break;
                }
                Ok(n) => {
                    if stdin_tx.blocking_send(buffer[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    debug!("Stdin read error: {}", e);
                    break;
                }
            }
        }
    });
    
    // Heartbeat and timeout tracking
    let mut last_activity = Instant::now();
    let heartbeat_interval = Duration::from_secs(1);
    let timeout_duration = Duration::from_secs(2);
    let mut heartbeat_timer = tokio::time::interval(heartbeat_interval);
    
    // Main event loop
    let exit_code: i32;
    
    loop {
        tokio::select! {
            // Handle incoming WebSocket messages
            msg = ws_read.next() => {
                match msg {
                    Some(Ok(Message::Binary(data))) => {
                        last_activity = Instant::now();
                        
                        // Debug: log raw message
                        debug!("Received binary message ({} bytes): {}", data.len(), String::from_utf8_lossy(&data));
                        
                        match serde_json::from_slice::<RelayResponse>(&data) {
                            Ok(response) => {
                                match response {
                                    RelayResponse::DATA { req_id: rid, payload } => {
                                        if rid == req_id_for_ws {
                                            debug!("DATA message, payload len: {}", payload.len());
                                            let data = decode_payload(&payload)?;
                                            if !data.is_empty() {
                                                let mut stdout = std::io::stdout().lock();
                                                stdout.write_all(&data)?;
                                                stdout.flush()?;
                                            }
                                        }
                                    }
                                    RelayResponse::EXIT { req_id: rid, exit_code: code } => {
                                        if rid == req_id_for_ws {
                                            debug!("Process exited with code: {}", code);
                                            exit_code = code;
                                            break;
                                        }
                                    }
                                    RelayResponse::ERROR { req_id: rid, message } => {
                                        if rid == req_id_for_ws {
                                            eprintln!("Error: {}", message);
                                            exit_code = 1;
                                            break;
                                        }
                                    }
                                    RelayResponse::DEVICE_OFFLINE { req_id: rid, message } => {
                                        if rid == req_id_for_ws {
                                            eprintln!("Device offline: {}", message);
                                            exit_code = 1;
                                            break;
                                        }
                                    }
                                    RelayResponse::DEVICE_NOT_APPROVED { req_id: rid, message } => {
                                        if rid == req_id_for_ws {
                                            eprintln!("Device not approved: {}", message);
                                            exit_code = 1;
                                            break;
                                        }
                                    }
                                    RelayResponse::DEVICE_NOT_FOUND { req_id: rid, message } => {
                                        if rid == req_id_for_ws {
                                            eprintln!("Device not found: {}", message);
                                            exit_code = 1;
                                            break;
                                        }
                                    }
                                    RelayResponse::INVALID_REQ_ID { req_id: rid, message } => {
                                        if rid == req_id_for_ws {
                                            eprintln!("Invalid request: {}", message);
                                            exit_code = 1;
                                            break;
                                        }
                                    }
                                    RelayResponse::PING { ts } => {
                                        // Reply with PONG
                                        let pong = serde_json::json!({"type": "PONG", "ts": ts});
                                        let _ = ws_write.send(Message::Binary(pong.to_string().into_bytes().into())).await;
                                    }
                                    RelayResponse::PONG { .. } => {
                                        // Heartbeat response received
                                        trace!("Received PONG");
                                    }
                                    RelayResponse::ADMIN_RELOAD_OK { .. } |
                                    RelayResponse::ADMIN_RELOAD_ERROR { .. } => {
                                        // Ignore admin messages
                                    }
                                }
                            }
                            Err(e) => {
                                debug!("Failed to parse response: {} - raw: {}", e, String::from_utf8_lossy(&data));
                            }
                        }
                    }
                    Some(Ok(Message::Text(text))) => {
                        // Also handle text messages
                        last_activity = Instant::now();
                        debug!("Received text message: {}", text);
                    }
                    Some(Ok(Message::Close(_))) => {
                        debug!("WebSocket closed by server");
                        eprintln!("Connection closed by server");
                        exit_code = 1;
                        break;
                    }
                    Some(Ok(_)) => {
                        // Ping/Pong frames handled by tungstenite
                    }
                    Some(Err(e)) => {
                        error!("WebSocket error: {}", e);
                        eprintln!("Connection error: {}", e);
                        exit_code = 1;
                        break;
                    }
                    None => {
                        debug!("WebSocket stream ended");
                        eprintln!("Connection closed");
                        exit_code = 1;
                        break;
                    }
                }
            }
            
            // Handle stdin input
            Some(data) = stdin_rx.recv() => {
                let input_msg = InputMessage::new(req_id_for_stdin.clone(), &data);
                let json = serde_json::to_string(&input_msg)?;
                ws_write.send(Message::Binary(json.into_bytes().into())).await?;
            }
            
            // Heartbeat timer
            _ = heartbeat_timer.tick() => {
                // Check for timeout
                if last_activity.elapsed() > timeout_duration {
                    eprintln!("Connection timeout");
                    exit_code = 1;
                    break;
                }
                
                // Send PING
                let ping = serde_json::json!({"type": "PING", "ts": timestamp_ms()});
                let _ = ws_write.send(Message::Binary(ping.to_string().into_bytes().into())).await;
            }
        }
    }
    
    // Clean up
    let _ = ws_write.close().await;
    stdin_handle.abort();
    
    // Ensure cursor is on a new line after output
    println!();
    
    Ok(exit_code)
}
