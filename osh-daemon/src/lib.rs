//! OSH Daemon Library
//!
//! Core functionality for OSH Daemon that can be reused by both CLI and GUI versions.
//!
//! This library provides:
//! - Configuration management
//! - WebSocket connection and pairing
//! - Command execution (PTY and non-PTY modes)
//! - Heartbeat and state management
//! - Logging utilities

pub mod config;
pub mod protocol;
pub mod pty_manager;
pub mod cmd_executor;
pub mod daemon;
pub mod logger;

// Re-export commonly used items
pub use config::{DaemonConfig, init_config, load_config};
pub use daemon::{run, run_with_shutdown};
pub use logger::init_logger;
