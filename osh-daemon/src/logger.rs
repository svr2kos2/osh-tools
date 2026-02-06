//! Logging utilities for OSH Daemon
//! 
//! Provides file-based logging in addition to console output

use tracing::Level;
use tracing_appender::non_blocking::WorkerGuard;
use std::path::PathBuf;

/// Initialize logger with both console and file output
/// 
/// # Arguments
/// * `enable_file_logging` - Whether to enable file logging to current directory
/// 
/// Returns a WorkerGuard that must be kept alive for the duration of the program
pub fn init_logger(enable_file_logging: bool) -> Option<WorkerGuard> {
    let mut guard = None;
    
    if enable_file_logging {
        // Create logs directory in current working directory
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let log_dir = cwd.join("logs");
        
        if let Err(e) = std::fs::create_dir_all(&log_dir) {
            eprintln!("Failed to create logs directory: {}", e);
            return None;
        }
        
        // Create a file appender
        let file_appender = tracing_appender::rolling::daily(&log_dir, "daemon.log");
        let (non_blocking, worker_guard) = tracing_appender::non_blocking(file_appender);
        
        // Set up the subscriber with both console and file output
        let subscriber = tracing_subscriber::fmt()
            .with_writer(non_blocking)
            .with_ansi(false)
            .with_max_level(Level::INFO)
            .with_file(true)
            .with_line_number(true)
            .finish();
        
        let _ = tracing::subscriber::set_default(subscriber);
        guard = Some(worker_guard);
    } else {
        // Only console output
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(Level::INFO)
            .with_file(true)
            .with_line_number(true)
            .finish();
        
        let _ = tracing::subscriber::set_default(subscriber);
    }
    
    guard
}
