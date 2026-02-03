//! OSH Relay Service
//!
//! WebSocket router for remote command execution.
//!
//! Responsibilities:
//! - WebSocket Server (0.0.0.0:8080) for Client connections
//! - WebSocket Server (127.0.0.1:8081) for osh/osh-admin connections
//! - Device pairing and authentication
//! - Message routing between osh CLI and remote clients
//! - Heartbeat monitoring (1s ping, 2s timeout)

mod device_store;
mod protocol;
mod relay;
mod server;

use anyhow::Result;
use clap::Parser;
use std::sync::Arc;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

use device_store::DeviceStore;
use relay::RelayService;

/// OSH Relay Service - WebSocket router for remote command execution
#[derive(Parser, Debug)]
#[command(name = "osh-relay")]
#[command(version, about, long_about = None)]
struct Args {
    /// Port for Client (Local Daemon) connections
    #[arg(long, default_value = "8080")]
    client_port: u16,

    /// Port for osh CLI and osh-admin connections (localhost only)
    #[arg(long, default_value = "8081")]
    osh_port: u16,

    /// Path to devices.json configuration file
    #[arg(long, default_value = "devices.json")]
    config: String,

    /// Log level (trace, debug, info, warn, error)
    #[arg(long, default_value = "info")]
    log_level: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize logging
    let level = match args.log_level.to_lowercase().as_str() {
        "trace" => Level::TRACE,
        "debug" => Level::DEBUG,
        "info" => Level::INFO,
        "warn" => Level::WARN,
        "error" => Level::ERROR,
        _ => Level::INFO,
    };

    let subscriber = FmtSubscriber::builder()
        .with_max_level(level)
        .with_target(true)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false)
        .finish();

    tracing::subscriber::set_global_default(subscriber)?;

    info!("OSH Relay Service starting...");
    info!("Client port: {}", args.client_port);
    info!("OSH/Admin port: {} (localhost only)", args.osh_port);
    info!("Config file: {}", args.config);

    // Load device store
    let device_store = Arc::new(DeviceStore::new(&args.config).await?);
    info!(
        "Loaded {} devices from config",
        device_store.get_device_count().await
    );

    // Create relay service
    let relay = Arc::new(RelayService::new(device_store).await);

    // Start servers
    let relay_client = relay.clone();
    let relay_osh = relay.clone();

    let client_server = tokio::spawn(async move {
        if let Err(e) = server::start_client_server(relay_client, args.client_port).await {
            tracing::error!("Client server error: {}", e);
        }
    });

    let osh_server = tokio::spawn(async move {
        if let Err(e) = server::start_osh_server(relay_osh, args.osh_port).await {
            tracing::error!("OSH server error: {}", e);
        }
    });

    // Status reporter
    let relay_status = relay.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            info!(
                "Status: {} clients connected, {} active sessions",
                relay_status.get_connected_count().await,
                relay_status.get_session_count().await
            );
        }
    });

    info!("OSH Relay Service is running");

    // Wait for servers
    tokio::select! {
        _ = client_server => {
            tracing::error!("Client server terminated unexpectedly");
        }
        _ = osh_server => {
            tracing::error!("OSH server terminated unexpectedly");
        }
        _ = tokio::signal::ctrl_c() => {
            info!("Received Ctrl+C, shutting down...");
        }
    }

    info!("OSH Relay Service stopped");
    Ok(())
}
