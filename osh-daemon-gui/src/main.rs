//! OSH Daemon GUI
//!
//! System tray application for OSH daemon with auto-start functionality.
//! 
//! Features:
//! - System tray icon with menu
//! - Auto-start on Windows
//! - Run daemon in background
//! - Configure settings via GUI

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

/// Application state
struct AppState {
    daemon_running: bool,
    daemon_handle: Option<tokio::task::JoinHandle<()>>,
    shutdown_tx: Option<tokio::sync::mpsc::Sender<()>>,
    runtime: Arc<Runtime>,
}

fn main() -> Result<()> {
    // Initialize logging to file
    let log_dir = dirs::config_dir()
        .unwrap_or_else(|| std::env::current_dir().unwrap())
        .join("osh-daemon");
    std::fs::create_dir_all(&log_dir)?;
    
    let log_file = std::fs::File::create(log_dir.join("gui.log"))?;
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("osh_daemon_gui=info".parse()?)
                .add_directive("osh_daemon=info".parse()?)
        )
        .with_writer(Arc::new(log_file))
        .with_ansi(false)
        .init();
    
    info!("Starting OSH Daemon GUI...");
    
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
        runtime: Arc::clone(&runtime),
    }));
    
    // Create event loop
    let event_loop = EventLoopBuilder::new().build()?;
    
    // Load tray icon
    let icon = load_icon();
    
    // Create tray menu
    let tray_menu = Menu::new();
    let start_item = MenuItem::new("启动守护进程", true, None);
    let stop_item = MenuItem::new("停止守护进程", false, None);
    let separator = tray_icon::menu::PredefinedMenuItem::separator();
    let config_item = MenuItem::new("配置", true, None);
    let autostart_item = CheckMenuItem::new("开机自启", false, true, None);
    let separator2 = tray_icon::menu::PredefinedMenuItem::separator();
    let quit_item = MenuItem::new("退出", true, None);
    
    tray_menu.append(&start_item)?;
    tray_menu.append(&stop_item)?;
    tray_menu.append(&separator)?;
    tray_menu.append(&config_item)?;
    tray_menu.append(&autostart_item)?;
    tray_menu.append(&separator2)?;
    tray_menu.append(&quit_item)?;
    
    // Check and set autostart status (ignore errors)
    if let Ok(true) = is_autostart_enabled() {
        autostart_item.set_checked(true);
    }
    
    // Ensure autostart button is enabled
    autostart_item.set_enabled(true);
    
    // Create tray icon
    let _tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(tray_menu))
        .with_tooltip("OSH Daemon")
        .with_icon(icon)
        .build()?;
    
    info!("Tray icon created");
    
    // Handle menu events
    let menu_channel = MenuEvent::receiver();
    let app_state_clone = Arc::clone(&app_state);
    
    event_loop.run(move |_event, event_loop| {
        event_loop.set_control_flow(ControlFlow::Wait);
        
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

/// Load tray icon
fn load_icon() -> tray_icon::Icon {
    // Create a simple icon (16x16 red square for now)
    // TODO: Replace with actual icon
    let width = 16;
    let height = 16;
    let mut rgba = Vec::with_capacity(width * height * 4);
    
    for _y in 0..height {
        for _x in 0..width {
            rgba.push(255); // R
            rgba.push(64);  // G
            rgba.push(64);  // B
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
    
    // Spawn the daemon task
    let handle = runtime.spawn(async move {
        info!("Starting daemon...");
        match osh_daemon::run_with_shutdown(shutdown_rx).await {
            Ok(_) => info!("Daemon stopped normally"),
            Err(e) => error!("Daemon error: {:#}", e),
        }
        
        // Update state
        let mut state = state_clone.lock().unwrap();
        state.daemon_running = false;
        state.daemon_handle = None;
        state.shutdown_tx = None;
    });
    
    let mut state = state.lock().unwrap();
    state.daemon_running = true;
    state.daemon_handle = Some(handle);
    state.shutdown_tx = Some(shutdown_tx);
    
    Ok(())
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
