//! OSH Daemon GUI
//!
//! System tray application for OSH daemon with auto-start functionality.
//! 
//! Features:
//! - System tray icon with menu
//! - Auto-start on Windows
//! - Run daemon in background
//! - Configure settings via GUI
//! - Monitor daemon connection status

#![windows_subsystem = "windows"]

use anyhow::Result;
use std::sync::{Arc, Mutex};
use tokio::runtime::Runtime;
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, CheckMenuItem},
    TrayIconBuilder,
};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;
use winit::event_loop::{ControlFlow, EventLoopBuilder};

/// Connection status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectionStatus {
    /// Daemon not running
    Disconnected,
    /// Daemon running but not yet approved
    Connecting,
    /// Daemon connected and approved
    Connected,
}

/// Application state
struct AppState {
    daemon_running: bool,
    daemon_handle: Option<tokio::task::JoinHandle<()>>,
    shutdown_tx: Option<tokio::sync::mpsc::Sender<()>>,
    connection_status: ConnectionStatus,
    runtime: Arc<Runtime>,
}

fn main() -> Result<()> {
    // Initialize logging to current directory
    let log_dir = std::env::current_dir().unwrap().join("logs");
    std::fs::create_dir_all(&log_dir)?;
    
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join("gui.log"))?;
    
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("osh_daemon_gui=debug".parse()?)
                .add_directive("osh_daemon=info".parse()?)
        )
        .with_writer(Arc::new(log_file))
        .with_ansi(false)
        .with_thread_ids(true)
        .with_line_number(true)
        .init();
    
    info!("Starting OSH Daemon GUI...");
    info!("Log directory: {}", log_dir.display());
    
    // Create tokio runtime for async operations
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
    );
    
    // Create application state
    let app_state = Arc::new(Mutex::new(AppState {
        daemon_running: false,
        daemon_handle: None,
        shutdown_tx: None,
        connection_status: ConnectionStatus::Disconnected,
        runtime: Arc::clone(&runtime),
    }));
    
    // Create event loop
    let event_loop = EventLoopBuilder::new().build()?;
    
    // Create tray menu
    let tray_menu = Menu::new();
    let start_item = MenuItem::new("启动守护进程", true, None);
    let stop_item = MenuItem::new("停止守护进程", false, None);
    let separator = tray_icon::menu::PredefinedMenuItem::separator();
    let status_item = MenuItem::new("状态: 未连接", false, None);
    let separator2 = tray_icon::menu::PredefinedMenuItem::separator();
    let config_item = MenuItem::new("配置", true, None);
    let autostart_item = CheckMenuItem::new("开机自启", false, true, None);
    let separator3 = tray_icon::menu::PredefinedMenuItem::separator();
    let quit_item = MenuItem::new("退出", true, None);
    
    tray_menu.append(&start_item)?;
    tray_menu.append(&stop_item)?;
    tray_menu.append(&separator)?;
    tray_menu.append(&status_item)?;
    tray_menu.append(&separator2)?;
    tray_menu.append(&config_item)?;
    tray_menu.append(&autostart_item)?;
    tray_menu.append(&separator3)?;
    tray_menu.append(&quit_item)?;
    
    // Check and set autostart status (ignore errors)
    if let Ok(true) = is_autostart_enabled() {
        autostart_item.set_checked(true);
    }
    
    // Ensure autostart button is enabled
    autostart_item.set_enabled(true);
    
    // Create tray icon
    let tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(tray_menu))
        .with_tooltip("OSH Daemon - 未连接")
        .with_icon(create_icon(ConnectionStatus::Disconnected))
        .build()?;
    
    info!("Tray icon created");
    
    // Handle menu events
    let menu_channel = MenuEvent::receiver();
    let app_state_clone = Arc::clone(&app_state);
    let tray_icon_rc = std::rc::Rc::new(tray_icon);
    
    // Update status periodically - this will run in the event loop
    let app_state_monitor = Arc::clone(&app_state);
    let status_item_clone = status_item.clone();
    let tray_icon_clone = tray_icon_rc.clone();
    
    // Pre-spawn the monitor thread before event loop (non-Windows fallback)
    #[cfg(not(target_os = "windows"))]
    let _monitor_handle = {
        let state = Arc::clone(&app_state);
        std::thread::spawn(move || {
            monitor_daemon_status(state);
        })
    };

    let mut last_status = ConnectionStatus::Disconnected;
    
    event_loop.run(move |_event, event_loop| {
        event_loop.set_control_flow(ControlFlow::Wait);
        
        // Update status display only when status changes
        let state = app_state_monitor.lock().unwrap();
        if state.connection_status != last_status {
            let status_text = match state.connection_status {
                ConnectionStatus::Disconnected => "状态: 未连接",
                ConnectionStatus::Connecting => "状态: 连接中...",
                ConnectionStatus::Connected => "状态: 已连接",
            };
            
            let tooltip = match state.connection_status {
                ConnectionStatus::Disconnected => "OSH Daemon - 未连接",
                ConnectionStatus::Connecting => "OSH Daemon - 连接中...",
                ConnectionStatus::Connected => "OSH Daemon - 已连接",
            };
            
            let _ = status_item_clone.set_text(status_text);
            let _ = tray_icon_clone.set_tooltip(Some(tooltip.to_string()));
            let _ = tray_icon_clone.set_icon(Some(create_icon(state.connection_status)));
            last_status = state.connection_status;
        }
        drop(state); // Release lock before handling events
        
        // Check for menu events
        if let Ok(event) = menu_channel.try_recv() {
            if event.id == start_item.id() {
                info!("Start daemon requested");
                let state_clone = Arc::clone(&app_state_clone);
                std::thread::spawn(move || {
                    if let Err(e) = start_daemon_thread(state_clone) {
                        error!("Failed to start daemon: {}", e);
                    }
                });
                start_item.set_enabled(false);
                stop_item.set_enabled(true);
            } else if event.id == stop_item.id() {
                info!("Stop daemon requested");
                if let Err(e) = stop_daemon(Arc::clone(&app_state_clone)) {
                    error!("Failed to stop daemon: {}", e);
                }
                start_item.set_enabled(true);
                stop_item.set_enabled(false);
            } else if event.id == config_item.id() {
                info!("Config requested");
                open_config();
            } else if event.id == autostart_item.id() {
                info!("Toggle autostart");
                if let Err(e) = toggle_autostart() {
                    error!("Failed to toggle autostart: {}", e);
                } else {
                    // Update checkbox state
                    if let Ok(enabled) = is_autostart_enabled() {
                        autostart_item.set_checked(enabled);
                    }
                }
            } else if event.id == quit_item.id() {
                info!("Quit requested");
                event_loop.exit();
            }
        }
    })?;
    
    info!("Application exiting");
    Ok(())
}

/// Create tray icon based on connection status
fn create_icon(status: ConnectionStatus) -> tray_icon::Icon {
    // Create a 16x16 icon with different colors based on status
    let width = 16;
    let height = 16;
    let mut rgba = Vec::with_capacity(width * height * 4);
    
    let (r, g, b) = match status {
        ConnectionStatus::Disconnected => (255, 64, 64),   // Red
        ConnectionStatus::Connecting => (255, 165, 0),     // Orange
        ConnectionStatus::Connected => (100, 200, 100),    // Green
    };
    
    for _y in 0..height {
        for _x in 0..width {
            rgba.push(r);  // R
            rgba.push(g);  // G
            rgba.push(b);  // B
            rgba.push(255); // A
        }
    }
    
    tray_icon::Icon::from_rgba(rgba, width as u32, height as u32)
        .expect("Failed to create icon")
}

/// Start the daemon
fn start_daemon_thread(state: Arc<Mutex<AppState>>) -> Result<()> {
    let runtime = {
        let state = state.lock().unwrap();
        Arc::clone(&state.runtime)
    };
    
    let (shutdown_tx, shutdown_rx) = tokio::sync::mpsc::channel::<()>(1);
    let state_clone = Arc::clone(&state);
    
    // Update status to Connecting
    {
        let mut state_guard = state.lock().unwrap();
        state_guard.connection_status = ConnectionStatus::Connecting;
    }
    
    // Generate a unique pipe name
    let pipe_name = format!("osh_status_{}", std::process::id());
    let pipe_name_clone = pipe_name.clone();
    
    // Spawn the daemon task with status pipe
    let handle = runtime.spawn(async move {
        info!("Starting daemon with status pipe: {}", pipe_name_clone);
        match osh_daemon::run_with_shutdown(shutdown_rx, Some(pipe_name_clone)).await {
            Ok(_) => info!("Daemon stopped normally"),
            Err(e) => error!("Daemon error: {:#}", e),
        }
        
        // Update state
        let mut state = state_clone.lock().unwrap();
        state.daemon_running = false;
        state.daemon_handle = None;
        state.shutdown_tx = None;
        state.connection_status = ConnectionStatus::Disconnected;
    });
    
    let mut state_guard = state.lock().unwrap();
    state_guard.daemon_running = true;
    state_guard.daemon_handle = Some(handle);
    state_guard.shutdown_tx = Some(shutdown_tx);
    drop(state_guard); // Release the lock before calling listen_status_pipe
    
    // Spawn the IPC pipe listener
    listen_status_pipe(Arc::clone(&state), pipe_name);
    
    Ok(())
}

/// Listen on IPC named pipe for status updates from daemon
#[cfg(target_os = "windows")]
fn listen_status_pipe(state: Arc<Mutex<AppState>>, pipe_name: String) {
    let runtime = {
        let state = state.lock().unwrap();
        Arc::clone(&state.runtime)
    };

    runtime.spawn(async move {
        use tokio::io::{AsyncBufReadExt, BufReader};
        use tokio::net::windows::named_pipe::ServerOptions;
        use std::time::Duration;

        info!("Status pipe server starting for pipe: {}", pipe_name);
        let pipe_path = format!(r"\\.\pipe\{}", pipe_name);

        loop {
            let server = match ServerOptions::new().create(&pipe_path) {
                Ok(server) => server,
                Err(e) => {
                    error!("Failed to create status pipe server: {}", e);
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    continue;
                }
            };

            info!("Waiting for daemon to connect to status pipe: {}", pipe_name);
            if let Err(e) = server.connect().await {
                warn!("Failed to accept status pipe connection: {}", e);
                continue;
            }

            info!("Daemon connected to status pipe: {}", pipe_name);

            let mut reader = BufReader::new(server);
            let mut line = String::new();

            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => {
                        info!("Status pipe disconnected");
                        break;
                    }
                    Ok(_) => {
                        let status = line.trim();

                        let new_status = match status {
                            "CONNECTED" => {
                                info!("Received CONNECTED status");
                                ConnectionStatus::Connected
                            }
                            "CONNECTING" | "WAITING" => {
                                info!("Received {} status", status);
                                ConnectionStatus::Connecting
                            }
                            "RECONNECTING" => {
                                info!("Received RECONNECTING status");
                                ConnectionStatus::Connecting
                            }
                            "STOPPED" => {
                                info!("Received STOPPED status");
                                ConnectionStatus::Disconnected
                            }
                            "REJECTED" | "AUTH_FAILED" | "LIMIT_EXCEEDED" => {
                                warn!("Received error status: {}", status);
                                ConnectionStatus::Disconnected
                            }
                            _ => {
                                warn!("Unknown status: {}", status);
                                continue;
                            }
                        };

                        let mut state = state.lock().unwrap();
                        if state.daemon_running {
                            state.connection_status = new_status;
                        } else {
                            info!("Daemon not running, exiting status pipe listener");
                            return;
                        }
                    }
                    Err(e) => {
                        warn!("Failed reading status pipe: {}", e);
                        break;
                    }
                }
            }

            let state = state.lock().unwrap();
            if !state.daemon_running {
                info!("Daemon stopped, exiting status pipe listener");
                break;
            }
        }

        info!("Status pipe listener task exited");
    });
}

/// Listen on IPC named pipe for status updates from daemon (no-op on non-Windows)
#[cfg(not(target_os = "windows"))]
fn listen_status_pipe(_state: Arc<Mutex<AppState>>, _pipe_name: String) {
    // Fall back to log file monitoring on non-Windows platforms
}

/// Monitor daemon status by reading log file
#[cfg(not(target_os = "windows"))]
fn monitor_daemon_status(state: Arc<Mutex<AppState>>) {
    let log_path = std::env::current_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .join("logs")
        .join("gui.log");
    
    let mut last_modified = std::time::SystemTime::now();
    let mut connected = false;
    
    loop {
        std::thread::sleep(std::time::Duration::from_millis(500));
        
        // Check if daemon is still running
        {
            let state = state.lock().unwrap();
            if !state.daemon_running {
                connected = false;
                break;
            }
        }
        
        // Try to read log file
        if let Ok(metadata) = std::fs::metadata(&log_path) {
            if let Ok(modified) = metadata.modified() {
                if modified > last_modified {
                    last_modified = modified;
                    
                    // Read file and look for status indicators
                    if let Ok(content) = std::fs::read_to_string(&log_path) {
                        let lines: Vec<&str> = content.lines().collect();
                        
                        // Scan from the end for the most recent status
                        for line in lines.iter().rev().take(200) {
                            // Connected indicators - check these first (more recent = more relevant)
                            if line.contains("Pairing approved") {
                                if !connected {
                                    connected = true;
                                    let mut state = state.lock().unwrap();
                                    state.connection_status = ConnectionStatus::Connected;
                                }
                                break;
                            }
                            // Pending/Connecting indicators
                            if line.contains("Pairing pending") || line.contains("Waiting for admin approval") {
                                if connected {
                                    connected = false;
                                    let mut state = state.lock().unwrap();
                                    state.connection_status = ConnectionStatus::Connecting;
                                }
                                break;
                            }
                            // Reconnecting indicators
                            if line.contains("Reconnecting") || line.contains("Heartbeat timeout") {
                                if connected {
                                    connected = false;
                                    let mut state = state.lock().unwrap();
                                    state.connection_status = ConnectionStatus::Connecting;
                                }
                                break;
                            }
                            // Sent pairing request (initial connection)
                            if line.contains("Sent pairing request") {
                                if !connected && !matches!(state.lock().unwrap().connection_status, ConnectionStatus::Connected) {
                                    let mut state = state.lock().unwrap();
                                    state.connection_status = ConnectionStatus::Connecting;
                                }
                                break;
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Stop the daemon
fn stop_daemon(state: Arc<Mutex<AppState>>) -> Result<()> {
    let mut state = state.lock().unwrap();
    
    if let Some(shutdown_tx) = &state.shutdown_tx {
        // Send shutdown signal
        let tx = shutdown_tx.clone();
        info!("Sending shutdown signal to daemon");
        
        // Spawn task to send signal (non-blocking)
        std::thread::spawn(move || {
            // Try to send shutdown signal a few times
            for attempt in 1..=5 {
                if tx.blocking_send(()).is_ok() {
                    info!("Shutdown signal sent successfully");
                    return;
                }
                if attempt < 5 {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
            error!("Failed to send shutdown signal after 5 attempts");
        });
    } else {
        warn!("Daemon not running or shutdown channel not available");
    }
    
    state.daemon_running = false;
    
    Ok(())
}

/// Open configuration
fn open_config() {
    info!("Opening config...");
    
    // Get config path
    let config_path = dirs::config_dir()
        .unwrap_or_else(|| std::env::current_dir().unwrap())
        .join("osh-daemon")
        .join("daemon.toml");
    
    // Open in default editor
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("notepad")
            .arg(&config_path)
            .spawn();
    }
    
    #[cfg(not(target_os = "windows"))]
    {
        let _ = std::process::Command::new("xdg-open")
            .arg(&config_path)
            .spawn();
    }
}

/// Check if autostart is enabled
fn is_autostart_enabled() -> Result<bool> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        
        let app_name = "OSHDaemonGUI";
        
        let output = std::process::Command::new("reg")
            .args(&[
                "query",
                "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run",
                "/v",
                app_name,
            ])
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .output()?;
        
        Ok(output.status.success())
    }
    
    #[cfg(not(target_os = "windows"))]
    {
        Ok(false)
    }
}

/// Toggle autostart
fn toggle_autostart() -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        
        let exe_path = std::env::current_exe()?;
        let app_name = "OSHDaemonGUI";
        
        // Check if already registered
        let output = std::process::Command::new("reg")
            .args(&[
                "query",
                "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run",
                "/v",
                app_name,
            ])
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .output()?;
        
        if output.status.success() {
            // Remove from autostart
            info!("Removing from autostart");
            std::process::Command::new("reg")
                .args(&[
                    "delete",
                    "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run",
                    "/v",
                    app_name,
                    "/f",
                ])
                .creation_flags(0x08000000)
                .output()?;
        } else {
            // Add to autostart
            info!("Adding to autostart");
            std::process::Command::new("reg")
                .args(&[
                    "add",
                    "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run",
                    "/v",
                    app_name,
                    "/t",
                    "REG_SZ",
                    "/d",
                    &exe_path.to_string_lossy(),
                    "/f",
                ])
                .creation_flags(0x08000000)
                .output()?;
        }
    }
    
    Ok(())
}
