//! Device configuration management
//!
//! Provides functions to read and write devices.json file.

use std::path::PathBuf;
use anyhow::{Context, Result, bail};
use crate::protocol::{DevicesConfig, DeviceStatus};

/// Default path for devices.json
pub fn default_config_path() -> PathBuf {
    // Look for config in standard locations
    if let Ok(path) = std::env::var("OSH_DEVICES_CONFIG") {
        return PathBuf::from(path);
    }
    
    // Check current directory
    let local_path = PathBuf::from("devices.json");
    if local_path.exists() {
        return local_path;
    }
    
    // Default to /etc/osh/devices.json on Unix, or current dir on Windows
    #[cfg(unix)]
    {
        let etc_path = PathBuf::from("/etc/osh/devices.json");
        if etc_path.exists() {
            return etc_path;
        }
    }
    
    // Default to current directory
    PathBuf::from("devices.json")
}

/// Load devices configuration from file
pub fn load_config(path: &PathBuf) -> Result<DevicesConfig> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read config file: {}", path.display()))?;
    
    let config: DevicesConfig = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse config file: {}", path.display()))?;
    
    // Validate: check for duplicate device_ids
    let mut seen_ids = std::collections::HashSet::new();
    for device in &config.devices {
        if !seen_ids.insert(&device.device_id) {
            bail!("Duplicate device_id found: {}", device.device_id);
        }
    }
    
    // Validate: check for duplicate aliases
    let mut seen_aliases = std::collections::HashSet::new();
    for device in &config.devices {
        if let Some(alias) = &device.alias {
            if !seen_aliases.insert(alias) {
                bail!("Duplicate alias found: {}", alias);
            }
        }
    }
    
    Ok(config)
}

/// Save devices configuration to file
pub fn save_config(path: &PathBuf, config: &DevicesConfig) -> Result<()> {
    let content = serde_json::to_string_pretty(config)
        .context("Failed to serialize config")?;
    
    std::fs::write(path, content)
        .with_context(|| format!("Failed to write config file: {}", path.display()))?;
    
    Ok(())
}

/// Format device list as table for display
pub fn format_device_list(config: &DevicesConfig) -> String {
    use std::fmt::Write;
    
    let mut output = String::new();
    
    writeln!(output, "Max devices: {}", config.max_devices).unwrap();
    writeln!(output, "Total devices: {}\n", config.devices.len()).unwrap();
    
    if config.devices.is_empty() {
        writeln!(output, "No devices registered.").unwrap();
        return output;
    }
    
    // Header
    writeln!(output, "{:<36}  {:<12}  {:<10}  {:<10}  {:<10}  {}",
        "Device ID", "Alias", "Name", "Platform", "Status", "Last Seen").unwrap();
    writeln!(output, "{}", "-".repeat(100)).unwrap();
    
    // Sort by status (pending first), then by created_at
    let mut devices: Vec<_> = config.devices.iter().collect();
    devices.sort_by(|a, b| {
        match (&a.status, &b.status) {
            (DeviceStatus::Pending, DeviceStatus::Pending) => a.created_at.cmp(&b.created_at),
            (DeviceStatus::Pending, _) => std::cmp::Ordering::Less,
            (_, DeviceStatus::Pending) => std::cmp::Ordering::Greater,
            _ => a.created_at.cmp(&b.created_at),
        }
    });
    
    for device in devices {
        let alias = device.alias.as_deref().unwrap_or("-");
        let device_id_short = if device.device_id.len() > 36 {
            &device.device_id[..36]
        } else {
            &device.device_id
        };
        let name_short = if device.device_name.len() > 10 {
            format!("{}...", &device.device_name[..7])
        } else {
            device.device_name.clone()
        };
        
        // Format status with color indicators
        let status_str = match device.status {
            DeviceStatus::Pending => "⏳ pending",
            DeviceStatus::Approved => "✓ approved",
            DeviceStatus::Rejected => "✗ rejected",
        };
        
        // Format last_seen to be more readable
        let last_seen = &device.last_seen;
        let last_seen_short = if last_seen.len() > 19 {
            &last_seen[..19]
        } else {
            last_seen
        };
        
        writeln!(output, "{:<36}  {:<12}  {:<10}  {:<10}  {:<10}  {}",
            device_id_short,
            alias,
            name_short,
            device.platform,
            status_str,
            last_seen_short
        ).unwrap();
    }
    
    output
}
