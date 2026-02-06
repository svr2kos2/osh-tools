//! OSH Daemon CLI Entry Point
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

use anyhow::Result;
use clap::{Parser, Subcommand};
use osh_daemon::{init_config, run, init_logger};
use tracing::info;
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
    Run {
        /// Status pipe name for IPC (e.g., osh_status_<pid>)
        #[arg(long)]
        status_pipe: Option<String>,
        /// Enable logging to logs/daemon.log in current directory
        #[arg(long)]
        log: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    
    // Initialize logging based on --log flag
    let _guard = match &cli.command {
        Commands::Run { log, .. } => init_logger(*log),
        _ => None,
    };
    
    // Initialize console tracing (file logging is handled by init_logger above)
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("osh_daemon=info".parse()?)
        )
        .try_init();
    
    info!("OSH Daemon started");
    
    match cli.command {
        Commands::Init { server, secret } => {
            init_config(server, secret)?;
        }
        Commands::Run { status_pipe, .. } => {
            info!("Starting daemon with status_pipe: {:?}", status_pipe);
            run(status_pipe).await?;
        }
    }
    
    Ok(())
}
