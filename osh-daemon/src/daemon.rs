//! Main daemon logic for osh-daemon
//!
//! Handles WebSocket connection, pairing, heartbeat, and command execution.

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, RwLock};
use tokio::time::{interval, timeout};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

use crate::config::{load_config, DaemonConfig};
use crate::protocol::{
    decode_payload, timestamp_ms, DataResponse, ErrorResponse, ExitResponse,
    GenericMessage, PairRequest, PingMessage, PongMessage,
};
use crate::pty_manager::{PtyEvent, PtyManager};
use crate::cmd_executor::{CmdEvent, execute_command};

/// Heartbeat interval
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);
/// Heartbeat timeout
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(2);

/// Daemon state
#[derive(Debug, Clone, PartialEq, Eq)]
enum DaemonState {
    #[allow(dead_code)]
    Connecting,
    WaitingForPairing,
    Approved,
    Disconnected,
}

/// Run the daemon
pub async fn run() -> Result<()> {
    let (_tx, rx) = tokio::sync::mpsc::channel::<()>(1);
    run_with_shutdown(rx).await
}

/// Run the daemon with shutdown signal support
/// The shutdown receiver can be used by GUI or other frontends to gracefully stop the daemon
pub async fn run_with_shutdown(shutdown_rx: tokio::sync::mpsc::Receiver<()>) -> Result<()> {
    info!("Starting osh-daemon...");
    
    // Load configuration
    let config = load_config().context("Failed to load configuration")?;
    info!("Configuration loaded:");
    info!("  Device ID: {}", config.device_id);
    info!("  Device Name: {}", config.device_name);
    info!("  Server: {}", config.server_url);
    info!("  Shell: {}", config.effective_shell());
    info!("  Platform: {}", DaemonConfig::platform());
    
    // Connect and run
    let result = connect_and_run(config, shutdown_rx).await;
    
    if let Err(ref e) = result {
        error!("Daemon error: {:#}", e);
    }
    
    result
}

/// Connect to the relay server and run the main loop
async fn connect_and_run(config: DaemonConfig, mut shutdown_rx: tokio::sync::mpsc::Receiver<()>) -> Result<()> {
    info!("Connecting to {}...", config.server_url);
    
    // Connect to WebSocket
    let (ws_stream, _) = connect_async(&config.server_url)
        .await
        .context("Failed to connect to relay server")?;
    
    info!("Connected to relay server");
    
    // Split the stream
    let (mut ws_write, mut ws_read) = ws_stream.split();
    
    // Send pairing request
    let pair_request = PairRequest::new(
        config.device_id.clone(),
        config.device_name.clone(),
        DaemonConfig::platform(),
        config.effective_shell(),
        config.secret_key.clone(),
    );
    
    let msg = serde_json::to_vec(&pair_request)?;
    ws_write.send(Message::Binary(msg.into())).await?;
    info!("Sent pairing request");
    
    // Create channels
    let (pty_event_tx, mut pty_event_rx) = mpsc::channel::<PtyEvent>(100);
    let (cmd_event_tx, mut cmd_event_rx) = mpsc::channel::<CmdEvent>(100);
    let (ws_send_tx, mut ws_send_rx) = mpsc::channel::<Message>(100);
    
    // Create PTY manager (for --pty mode)
    let pty_manager = Arc::new(PtyManager::new(config.effective_shell(), pty_event_tx));
    
    // Shell for command executor
    let shell = config.effective_shell();
    
    // State
    let state = Arc::new(RwLock::new(DaemonState::WaitingForPairing));
    let last_activity = Arc::new(RwLock::new(Instant::now()));
    
    // Heartbeat task
    let heartbeat_state = Arc::clone(&state);
    let heartbeat_last_activity = Arc::clone(&last_activity);
    let heartbeat_ws_tx = ws_send_tx.clone();
    let heartbeat_handle = tokio::spawn(async move {
        let mut heartbeat_interval = interval(HEARTBEAT_INTERVAL);
        
        loop {
            heartbeat_interval.tick().await;
            
            // Check state
            let current_state = heartbeat_state.read().await.clone();
            if current_state == DaemonState::Disconnected {
                break;
            }
            
            // Check timeout
            let elapsed = heartbeat_last_activity.read().await.elapsed();
            if elapsed > HEARTBEAT_TIMEOUT {
                error!("Heartbeat timeout! No activity for {:?}", elapsed);
                *heartbeat_state.write().await = DaemonState::Disconnected;
                break;
            }
            
            // Send ping
            let ping = PingMessage::new();
            if let Ok(msg) = serde_json::to_vec(&ping) {
                if heartbeat_ws_tx.send(Message::Binary(msg.into())).await.is_err() {
                    break;
                }
            }
        }
    });
    
    // PTY event handler task
    let pty_event_ws_tx = ws_send_tx.clone();
    let pty_event_handle = tokio::spawn(async move {
        while let Some(event) = pty_event_rx.recv().await {
            let msg = match event {
                PtyEvent::Data { req_id, data } => {
                    // In PTY mode, escape control characters for visibility
                    let escaped = escape_control_chars(&data);
                    let response = DataResponse::new(req_id, &escaped);
                    serde_json::to_vec(&response).ok()
                }
                PtyEvent::Exit { req_id, exit_code } => {
                    info!("PTY process exited: req_id={}, exit_code={}", req_id, exit_code);
                    let response = ExitResponse::new(req_id, exit_code);
                    serde_json::to_vec(&response).ok()
                }
                PtyEvent::Error { req_id, error } => {
                    error!("PTY process error: req_id={}, error={}", req_id, error);
                    let response = ErrorResponse::new(req_id, error);
                    serde_json::to_vec(&response).ok()
                }
            };
            
            if let Some(data) = msg {
                if pty_event_ws_tx.send(Message::Binary(data.into())).await.is_err() {
                    break;
                }
            }
        }
    });
    
    // Command executor event handler task
    let cmd_event_ws_tx = ws_send_tx.clone();
    let cmd_event_handle = tokio::spawn(async move {
        while let Some(event) = cmd_event_rx.recv().await {
            let msg = match event {
                CmdEvent::Data { req_id, data } => {
                    let response = DataResponse::new(req_id, &data);
                    serde_json::to_vec(&response).ok()
                }
                CmdEvent::Exit { req_id, exit_code } => {
                    info!("Command finished: req_id={}, exit_code={}", req_id, exit_code);
                    let response = ExitResponse::new(req_id, exit_code);
                    serde_json::to_vec(&response).ok()
                }
                CmdEvent::Error { req_id, error } => {
                    error!("Command error: req_id={}, error={}", req_id, error);
                    let response = ErrorResponse::new(req_id, error);
                    serde_json::to_vec(&response).ok()
                }
            };
            
            if let Some(data) = msg {
                if cmd_event_ws_tx.send(Message::Binary(data.into())).await.is_err() {
                    break;
                }
            }
        }
    });
    
    // WebSocket send task
    let ws_send_handle = tokio::spawn(async move {
        while let Some(msg) = ws_send_rx.recv().await {
            if ws_write.send(msg).await.is_err() {
                break;
            }
        }
    });
    
    // Main receive loop
    let main_state = Arc::clone(&state);
    let main_last_activity = Arc::clone(&last_activity);
    let main_pty_manager = Arc::clone(&pty_manager);
    let main_ws_tx = ws_send_tx.clone();
    let main_cmd_event_tx = cmd_event_tx.clone();
    let main_shell = shell.clone();
    
    loop {
        tokio::select! {
            // Check for shutdown signal
            _ = shutdown_rx.recv() => {
                info!("Shutdown signal received");
                *main_state.write().await = DaemonState::Disconnected;
                break;
            }
            
            // Read message
            read_result = async {
                timeout(HEARTBEAT_TIMEOUT, ws_read.next()).await
            } => {
                match read_result {
                    Ok(Some(Ok(msg))) => {
                        // Update last activity
                        *main_last_activity.write().await = Instant::now();
                        
                        // Handle message
                        if let Message::Binary(data) = msg {
                            if let Err(e) = handle_message(
                                &data,
                                &main_state,
                                &main_pty_manager,
                                &main_ws_tx,
                                &main_shell,
                                &main_cmd_event_tx,
                            ).await {
                                error!("Error handling message: {:#}", e);
                            }
                        }
                    }
                    Ok(Some(Err(e))) => {
                        error!("WebSocket error: {}", e);
                        *main_state.write().await = DaemonState::Disconnected;
                        break;
                    }
                    Ok(None) => {
                        info!("WebSocket closed by server");
                        *main_state.write().await = DaemonState::Disconnected;
                        break;
                    }
                    Err(_) => {
                        // Timeout - check if we should continue
                        let elapsed = main_last_activity.read().await.elapsed();
                        if elapsed > HEARTBEAT_TIMEOUT {
                            error!("Read timeout");
                            *main_state.write().await = DaemonState::Disconnected;
                            break;
                        }
                    }
                }
            }
        }
    }
    
    let current_state = main_state.read().await.clone();
    if current_state == DaemonState::Disconnected {
        info!("Daemon disconnected");
    }
    
    // Cleanup
    info!("Cleaning up...");
    
    // Kill all processes
    pty_manager.kill_all().await;
    
    // Cancel tasks
    heartbeat_handle.abort();
    pty_event_handle.abort();
    cmd_event_handle.abort();
    ws_send_handle.abort();
    
    info!("Daemon stopped");
    Ok(())
}

/// Handle an incoming message
async fn handle_message(
    data: &[u8],
    state: &Arc<RwLock<DaemonState>>,
    pty_manager: &Arc<PtyManager>,
    ws_tx: &mpsc::Sender<Message>,
    shell: &str,
    cmd_event_tx: &mpsc::Sender<CmdEvent>,
) -> Result<()> {
    // Parse as generic message
    let msg: GenericMessage = serde_json::from_slice(data)
        .context("Failed to parse message")?;
    
    debug!("Received message: type={}", msg.msg_type);
    
    // Handle PING/PONG
    if msg.is_ping() {
        let pong = PongMessage::new(msg.ts.unwrap_or_else(timestamp_ms));
        let response = serde_json::to_vec(&pong)?;
        ws_tx.send(Message::Binary(response.into())).await?;
        return Ok(());
    }
    
    if msg.is_pong() {
        // Just update activity time (already done)
        return Ok(());
    }
    
    // Handle pairing responses
    if msg.is_pair_response() {
        handle_pair_response(&msg, state).await?;
        return Ok(());
    }
    
    // Handle commands (only if approved)
    if msg.is_command() {
        let current_state = state.read().await.clone();
        if current_state != DaemonState::Approved {
            warn!("Received command while not approved, ignoring");
            return Ok(());
        }
        
        handle_command(&msg, pty_manager, ws_tx, shell, cmd_event_tx).await?;
    }
    
    Ok(())
}

/// Handle pairing response
async fn handle_pair_response(
    msg: &GenericMessage,
    state: &Arc<RwLock<DaemonState>>,
) -> Result<()> {
    match msg.msg_type.as_str() {
        "PAIR_APPROVED" => {
            info!("Pairing approved! Ready to receive commands.");
            *state.write().await = DaemonState::Approved;
        }
        "PAIR_PENDING" => {
            info!("Pairing pending. Waiting for admin approval...");
            if let Some(message) = &msg.message {
                info!("Server message: {}", message);
            }
            *state.write().await = DaemonState::WaitingForPairing;
        }
        "PAIR_REJECTED" => {
            error!("Pairing rejected by server.");
            if let Some(message) = &msg.message {
                error!("Reason: {}", message);
            }
            *state.write().await = DaemonState::Disconnected;
            anyhow::bail!("Pairing rejected");
        }
        "PAIR_AUTH_FAILED" => {
            error!("Authentication failed! Check your secret_key configuration.");
            if let Some(message) = &msg.message {
                error!("Server message: {}", message);
            }
            *state.write().await = DaemonState::Disconnected;
            anyhow::bail!("Authentication failed");
        }
        "PAIR_LIMIT_EXCEEDED" => {
            error!("Device limit exceeded! Contact the server administrator.");
            if let Some(message) = &msg.message {
                error!("Server message: {}", message);
            }
            *state.write().await = DaemonState::Disconnected;
            anyhow::bail!("Device limit exceeded");
        }
        _ => {
            warn!("Unknown pairing response: {}", msg.msg_type);
        }
    }
    
    Ok(())
}

/// Handle a command from the relay
async fn handle_command(
    msg: &GenericMessage,
    pty_manager: &Arc<PtyManager>,
    ws_tx: &mpsc::Sender<Message>,
    shell: &str,
    cmd_event_tx: &mpsc::Sender<CmdEvent>,
) -> Result<()> {
    let req_id = msg.req_id.clone().unwrap_or_default();
    
    if req_id.is_empty() {
        warn!("Command without req_id, ignoring");
        return Ok(());
    }
    
    match msg.msg_type.as_str() {
        "SPAWN" => {
            let payload = msg.payload.as_ref()
                .ok_or_else(|| anyhow::anyhow!("SPAWN missing payload"))?;
            let command = String::from_utf8(decode_payload(payload)?)
                .context("Invalid UTF-8 in command")?;
            let cols = msg.cols.unwrap_or(80);
            let rows = msg.rows.unwrap_or(24);
            let use_pty = msg.pty.unwrap_or(false);
            
            info!("SPAWN: req_id={}, command={}, pty={}, cols={}, rows={}", req_id, command, use_pty, cols, rows);
            
            if use_pty {
                // PTY mode - use portable-pty with escaped output
                if let Err(e) = pty_manager.spawn(req_id.clone(), &command, cols, rows).await {
                    error!("Failed to spawn PTY process: {}", e);
                    let response = ErrorResponse::new(req_id, e.to_string());
                    let data = serde_json::to_vec(&response)?;
                    ws_tx.send(Message::Binary(data.into())).await?;
                }
            } else {
                // Non-PTY mode - use simple command executor
                if let Err(e) = execute_command(shell, req_id.clone(), &command, cmd_event_tx.clone()).await {
                    error!("Failed to execute command: {}", e);
                    let response = ErrorResponse::new(req_id, e.to_string());
                    let data = serde_json::to_vec(&response)?;
                    ws_tx.send(Message::Binary(data.into())).await?;
                }
            }
        }
        "DATA" => {
            // DATA only works with PTY mode
            let payload = msg.payload.as_ref()
                .ok_or_else(|| anyhow::anyhow!("DATA missing payload"))?;
            let data = decode_payload(payload)?;
            
            // Log received input for debugging
            info!("DATA: req_id={}, len={}, content={:?}", req_id, data.len(), String::from_utf8_lossy(&data));
            
            // Convert \r to \r\n on Windows for PTY compatibility
            #[cfg(target_os = "windows")]
            let data = convert_line_endings(&data);
            
            if let Err(e) = pty_manager.send_input(&req_id, &data).await {
                warn!("Failed to send input (process might be non-PTY): {}", e);
            }
        }
        "RESIZE" => {
            // RESIZE only works with PTY mode
            let cols = msg.cols.unwrap_or(80);
            let rows = msg.rows.unwrap_or(24);
            
            debug!("RESIZE: req_id={}, cols={}, rows={}", req_id, cols, rows);
            
            if let Err(e) = pty_manager.resize(&req_id, cols, rows).await {
                warn!("Failed to resize (process might be non-PTY): {}", e);
            }
        }
        "KILL" => {
            info!("KILL: req_id={}", req_id);
            
            if let Err(e) = pty_manager.kill(&req_id).await {
                warn!("Failed to kill process: {}", e);
            }
        }
        "INVALID_REQ_ID" => {
            warn!("Server reported invalid req_id: {}", req_id);
            // Kill the process if it exists
            let _ = pty_manager.kill(&req_id).await;
        }
        _ => {
            warn!("Unknown command type: {}", msg.msg_type);
        }
    }
    
    Ok(())
}

/// Escape control characters for PTY mode output
fn escape_control_chars(data: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(data.len() * 2);
    let mut i = 0;
    
    while i < data.len() {
        let b = data[i];
        
        if b == 0x1b {
            // ESC character - escape it
            result.extend_from_slice(b"\\x1b");
            i += 1;
        } else if b < 0x20 && b != b'\n' && b != b'\r' && b != b'\t' {
            // Other control characters (except newline, carriage return, tab)
            result.extend_from_slice(format!("\\x{:02x}", b).as_bytes());
            i += 1;
        } else {
            result.push(b);
            i += 1;
        }
    }
    
    result
}

/// Convert \r to \r\n and \n to \r\n for Windows PTY compatibility
/// Only used on Windows target
#[cfg(target_os = "windows")]
fn convert_line_endings(data: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(data.len() * 2);
    let mut i = 0;
    
    while i < data.len() {
        let b = data[i];
        if b == b'\r' {
            // Check if next char is already \n
            if i + 1 < data.len() && data[i + 1] == b'\n' {
                // Already \r\n, keep as is
                result.push(b'\r');
                result.push(b'\n');
                i += 2;
            } else {
                // Just \r, convert to \r\n
                result.push(b'\r');
                result.push(b'\n');
                i += 1;
            }
        } else if b == b'\n' {
            // Check if previous char was \r (already handled above)
            // This case means standalone \n, convert to \r\n
            result.push(b'\r');
            result.push(b'\n');
            i += 1;
        } else {
            result.push(b);
            i += 1;
        }
    }
    
    result
}
