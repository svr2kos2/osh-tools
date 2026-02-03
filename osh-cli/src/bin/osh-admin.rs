//! OSH Admin CLI
//!
//! Device management tool for OSH Relay Service.
//!
//! Commands:
//! - list: List all devices and their status
//! - approve: Approve a pending device
//! - reject: Reject a device
//! - alias: Set device alias
//! - remove: Remove a device
//! - reload: Notify Relay Service to reload config

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use futures_util::{SinkExt, StreamExt};
use std::path::PathBuf;
use std::time::Duration;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, Level};
use tracing_subscriber::FmtSubscriber;

use osh_cli::config::{default_config_path, load_config, save_config, format_device_list};
use osh_cli::protocol::{DeviceStatus, AdminRequest, RelayResponse};

/// OSH Admin CLI - Device management tool
#[derive(Parser)]
#[command(name = "osh-admin")]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Path to devices.json config file
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,
    
    /// Relay service WebSocket URL for reload command
    #[arg(long, default_value = "ws://127.0.0.1:8081")]
    relay_url: String,
    
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List all devices and their status
    List,
    
    /// Approve a pending device
    Approve {
        /// Device ID or alias (supports prefix match)
        device: String,
    },
    
    /// Reject a device
    Reject {
        /// Device ID or alias (supports prefix match)
        device: String,
    },
    
    /// Set device alias
    Alias {
        /// Device ID (supports prefix match)
        device_id: String,
        /// New alias to set
        alias: String,
    },
    
    /// Remove a device from the list
    Remove {
        /// Device ID or alias (supports prefix match)
        device: String,
    },
    
    /// Notify Relay Service to reload configuration
    Reload,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    let _subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .with_target(false)
        .without_time()
        .init();
    
    let cli = Cli::parse();
    let config_path = cli.config.unwrap_or_else(default_config_path);
    
    match cli.command {
        Commands::List => cmd_list(&config_path)?,
        Commands::Approve { device } => cmd_approve(&config_path, &device)?,
        Commands::Reject { device } => cmd_reject(&config_path, &device)?,
        Commands::Alias { device_id, alias } => cmd_alias(&config_path, &device_id, &alias)?,
        Commands::Remove { device } => cmd_remove(&config_path, &device)?,
        Commands::Reload => cmd_reload(&cli.relay_url).await?,
    }
    
    Ok(())
}

/// List all devices
fn cmd_list(config_path: &PathBuf) -> Result<()> {
    let config = load_config(config_path)?;
    println!("{}", format_device_list(&config));
    Ok(())
}

/// Approve a device
fn cmd_approve(config_path: &PathBuf, device: &str) -> Result<()> {
    let mut config = load_config(config_path)?;
    
    let device_entry = config.find_device_mut(device)
        .with_context(|| format!("Device not found: {}", device))?;
    
    if device_entry.status == DeviceStatus::Approved {
        println!("Device '{}' is already approved.", device_entry.device_id);
        return Ok(());
    }
    
    let device_id = device_entry.device_id.clone();
    device_entry.status = DeviceStatus::Approved;
    
    save_config(config_path, &config)?;
    println!("✓ Device '{}' approved.", device_id);
    println!("Run 'osh-admin reload' to apply changes.");
    
    Ok(())
}

/// Reject a device
fn cmd_reject(config_path: &PathBuf, device: &str) -> Result<()> {
    let mut config = load_config(config_path)?;
    
    let device_entry = config.find_device_mut(device)
        .with_context(|| format!("Device not found: {}", device))?;
    
    if device_entry.status == DeviceStatus::Rejected {
        println!("Device '{}' is already rejected.", device_entry.device_id);
        return Ok(());
    }
    
    let device_id = device_entry.device_id.clone();
    device_entry.status = DeviceStatus::Rejected;
    
    save_config(config_path, &config)?;
    println!("✗ Device '{}' rejected.", device_id);
    println!("Run 'osh-admin reload' to apply changes.");
    
    Ok(())
}

/// Set device alias
fn cmd_alias(config_path: &PathBuf, device_id: &str, alias: &str) -> Result<()> {
    let mut config = load_config(config_path)?;
    
    // Check if alias is already in use by another device
    if let Some(existing) = config.devices.iter().find(|d| {
        d.alias.as_ref().map(|a| a == alias).unwrap_or(false)
    }) {
        if !existing.device_id.starts_with(device_id) {
            bail!("Alias '{}' is already in use by device '{}'", alias, existing.device_id);
        }
    }
    
    let device_entry = config.find_device_mut(device_id)
        .with_context(|| format!("Device not found: {}", device_id))?;
    
    let full_device_id = device_entry.device_id.clone();
    device_entry.alias = Some(alias.to_string());
    
    save_config(config_path, &config)?;
    println!("✓ Device '{}' alias set to '{}'.", full_device_id, alias);
    
    Ok(())
}

/// Remove a device
fn cmd_remove(config_path: &PathBuf, device: &str) -> Result<()> {
    let mut config = load_config(config_path)?;
    
    let removed = config.remove_device(device)
        .with_context(|| format!("Device not found: {}", device))?;
    
    save_config(config_path, &config)?;
    println!("✓ Device '{}' removed.", removed.device_id);
    println!("Run 'osh-admin reload' to apply changes.");
    
    Ok(())
}

/// Notify Relay Service to reload configuration
async fn cmd_reload(relay_url: &str) -> Result<()> {
    println!("Connecting to Relay Service at {}...", relay_url);
    
    let (ws_stream, _) = tokio::time::timeout(
        Duration::from_secs(5),
        connect_async(relay_url)
    )
    .await
    .context("Connection timeout")?
    .context("Failed to connect to Relay Service")?;
    
    let (mut write, mut read) = ws_stream.split();
    
    // Send reload request
    let request = AdminRequest::ADMIN_RELOAD;
    let json = serde_json::to_string(&request)?;
    write.send(Message::Binary(json.into_bytes().into())).await?;
    
    // Wait for response with timeout
    let response = tokio::time::timeout(Duration::from_secs(5), read.next())
        .await
        .context("Response timeout")?
        .context("Connection closed")?;
    
    match response {
        Ok(Message::Binary(data)) => {
            let response: RelayResponse = serde_json::from_slice(&data)
                .context("Failed to parse response")?;
            
            match response {
                RelayResponse::ADMIN_RELOAD_OK { message } => {
                    println!("✓ Reload successful: {}", message);
                }
                RelayResponse::ADMIN_RELOAD_ERROR { message } => {
                    error!("Reload failed: {}", message);
                    bail!("Reload failed: {}", message);
                }
                _ => {
                    bail!("Unexpected response from Relay Service");
                }
            }
        }
        Ok(Message::Text(text)) => {
            // Also handle text messages for compatibility
            let response: RelayResponse = serde_json::from_str(&text)
                .context("Failed to parse response")?;
            
            match response {
                RelayResponse::ADMIN_RELOAD_OK { message } => {
                    println!("✓ Reload successful: {}", message);
                }
                RelayResponse::ADMIN_RELOAD_ERROR { message } => {
                    error!("Reload failed: {}", message);
                    bail!("Reload failed: {}", message);
                }
                _ => {
                    bail!("Unexpected response from Relay Service");
                }
            }
        }
        Ok(_) => bail!("Unexpected message type from Relay Service"),
        Err(e) => bail!("WebSocket error: {}", e),
    }
    
    Ok(())
}
