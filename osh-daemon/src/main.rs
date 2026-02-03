//! OSH Daemon
//!
//! Local client for remote command execution.
//!
//! Commands:
//! - osh-daemon init [--server <url>] [--secret <key>] - Initialize config
//! - osh-daemon run - Start the daemon
//!
//! Responsibilities:
//! - Connect to Relay Service via WebSocket
//! - Handle device pairing with PSK authentication
//! - Execute commands using portable-pty
//! - Heartbeat (1s ping, 2s timeout)

mod config;
mod protocol;
mod pty_manager;
mod cmd_executor;
mod daemon;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "osh-daemon")]
#[command(about = "OSH Local Daemon - Remote command execution client")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize configuration file
    Init {
        /// Server WebSocket URL (e.g., wss://example.com/bridge)
        #[arg(short, long)]
        server: Option<String>,
        /// Pre-shared secret key
        #[arg(short = 'k', long)]
        secret: Option<String>,
    },
    /// Start the daemon
    Run,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("osh_daemon=info".parse()?)
        )
        .init();
    
    let cli = Cli::parse();
    
    match cli.command {
        Commands::Init { server, secret } => {
            config::init_config(server, secret)?;
        }
        Commands::Run => {
            daemon::run().await?;
        }
    }
    
    Ok(())
}
