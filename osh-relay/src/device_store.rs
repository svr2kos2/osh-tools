//! Device storage and management
//!
//! Handles reading/writing devices.json and device state management.

#![allow(dead_code)]

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tokio::fs;
use tokio::sync::RwLock;
use tracing::info;

/// Device status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceStatus {
    Pending,
    Approved,
    Rejected,
}

/// Device information stored in devices.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub device_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    pub device_name: String,
    pub platform: String,
    pub shell: String,
    pub status: DeviceStatus,
    pub created_at: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
}

/// Configuration file structure (devices.json)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevicesConfig {
    pub max_devices: usize,
    pub secret_key: String,
    pub devices: Vec<DeviceInfo>,
}

impl Default for DevicesConfig {
    fn default() -> Self {
        Self {
            max_devices: 10,
            secret_key: "change-me-in-production".to_string(),
            devices: Vec::new(),
        }
    }
}

/// Device store with thread-safe access
pub struct DeviceStore {
    config_path: String,
    config: RwLock<DevicesConfig>,
}

impl DeviceStore {
    /// Create a new device store, loading from the specified path
    pub async fn new(config_path: &str) -> Result<Self> {
        let config = Self::load_config(config_path).await?;
        Ok(Self {
            config_path: config_path.to_string(),
            config: RwLock::new(config),
        })
    }

    /// Load configuration from file
    async fn load_config(path: &str) -> Result<DevicesConfig> {
        if !Path::new(path).exists() {
            info!("Config file not found, creating default: {}", path);
            let config = DevicesConfig::default();
            let json = serde_json::to_string_pretty(&config)?;
            fs::write(path, json).await?;
            return Ok(config);
        }

        let content = fs::read_to_string(path)
            .await
            .with_context(|| format!("Failed to read {}", path))?;

        let config: DevicesConfig = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse {}", path))?;

        // Validate config
        Self::validate_config(&config)?;

        info!("Loaded {} devices from {}", config.devices.len(), path);
        Ok(config)
    }

    /// Validate configuration for duplicates
    fn validate_config(config: &DevicesConfig) -> Result<()> {
        let mut device_ids = HashMap::new();
        let mut aliases = HashMap::new();

        for device in &config.devices {
            // Check device_id duplicates
            if let Some(existing) = device_ids.insert(&device.device_id, &device.device_name) {
                return Err(anyhow!(
                    "Duplicate device_id '{}': found in '{}' and '{}'",
                    device.device_id,
                    existing,
                    device.device_name
                ));
            }

            // Check alias duplicates
            if let Some(alias) = &device.alias {
                if let Some(existing) = aliases.insert(alias, &device.device_name) {
                    return Err(anyhow!(
                        "Duplicate alias '{}': found in '{}' and '{}'",
                        alias,
                        existing,
                        device.device_name
                    ));
                }
            }
        }

        Ok(())
    }

    /// Reload configuration from file
    pub async fn reload(&self) -> Result<usize> {
        let config = Self::load_config(&self.config_path).await?;
        let count = config.devices.len();
        *self.config.write().await = config;
        Ok(count)
    }

    /// Get the secret key
    pub async fn get_secret_key(&self) -> String {
        self.config.read().await.secret_key.clone()
    }

    /// Get max devices limit
    pub async fn get_max_devices(&self) -> usize {
        self.config.read().await.max_devices
    }

    /// Get current device count
    pub async fn get_device_count(&self) -> usize {
        self.config.read().await.devices.len()
    }

    /// Find device by ID or alias
    pub async fn find_device(&self, id_or_alias: &str) -> Option<DeviceInfo> {
        let config = self.config.read().await;

        // Try exact match first
        if let Some(device) = config.devices.iter().find(|d| d.device_id == id_or_alias) {
            return Some(device.clone());
        }

        // Try alias match
        if let Some(device) = config
            .devices
            .iter()
            .find(|d| d.alias.as_deref() == Some(id_or_alias))
        {
            return Some(device.clone());
        }

        // Try prefix match for device_id
        let matches: Vec<_> = config
            .devices
            .iter()
            .filter(|d| d.device_id.starts_with(id_or_alias))
            .collect();

        if matches.len() == 1 {
            return Some(matches[0].clone());
        }

        None
    }

    /// Get device by exact ID
    pub async fn get_device_by_id(&self, device_id: &str) -> Option<DeviceInfo> {
        self.config
            .read()
            .await
            .devices
            .iter()
            .find(|d| d.device_id == device_id)
            .cloned()
    }

    /// Check if a device exists
    pub async fn device_exists(&self, device_id: &str) -> bool {
        self.config
            .read()
            .await
            .devices
            .iter()
            .any(|d| d.device_id == device_id)
    }

    /// Add or update a device (for pairing)
    pub async fn upsert_device(&self, device: DeviceInfo) -> Result<()> {
        let mut config = self.config.write().await;

        // Find and update or insert
        if let Some(existing) = config
            .devices
            .iter_mut()
            .find(|d| d.device_id == device.device_id)
        {
            // Update existing device
            existing.device_name = device.device_name;
            existing.platform = device.platform;
            existing.shell = device.shell;
            existing.last_seen = device.last_seen;
            // Don't change status on update - preserve approved/rejected state
        } else {
            // Check device limit
            if config.devices.len() >= config.max_devices {
                return Err(anyhow!("Device limit reached"));
            }
            config.devices.push(device);
        }

        // Save to file
        self.save_config_internal(&config).await?;
        Ok(())
    }

    /// Update device's last_seen timestamp
    pub async fn update_last_seen(&self, device_id: &str) -> Result<()> {
        let mut config = self.config.write().await;

        if let Some(device) = config
            .devices
            .iter_mut()
            .find(|d| d.device_id == device_id)
        {
            device.last_seen = Utc::now();
            self.save_config_internal(&config).await?;
        }

        Ok(())
    }

    /// Save configuration to file (internal, requires holding write lock)
    async fn save_config_internal(&self, config: &DevicesConfig) -> Result<()> {
        let json = serde_json::to_string_pretty(config)?;
        fs::write(&self.config_path, json)
            .await
            .with_context(|| format!("Failed to write {}", self.config_path))?;
        Ok(())
    }

    /// Get all devices
    pub async fn get_all_devices(&self) -> Vec<DeviceInfo> {
        self.config.read().await.devices.clone()
    }

    /// Check if device is approved
    pub async fn is_device_approved(&self, device_id: &str) -> bool {
        self.config
            .read()
            .await
            .devices
            .iter()
            .find(|d| d.device_id == device_id)
            .map(|d| d.status == DeviceStatus::Approved)
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_status_serialization() {
        let json = serde_json::to_string(&DeviceStatus::Approved).unwrap();
        assert_eq!(json, "\"approved\"");

        let status: DeviceStatus = serde_json::from_str("\"pending\"").unwrap();
        assert_eq!(status, DeviceStatus::Pending);
    }

    #[test]
    fn test_validate_config_duplicate_id() {
        let config = DevicesConfig {
            max_devices: 10,
            secret_key: "test".to_string(),
            devices: vec![
                DeviceInfo {
                    device_id: "same-id".to_string(),
                    alias: None,
                    device_name: "Device 1".to_string(),
                    platform: "windows".to_string(),
                    shell: "powershell".to_string(),
                    status: DeviceStatus::Approved,
                    created_at: Utc::now(),
                    last_seen: Utc::now(),
                },
                DeviceInfo {
                    device_id: "same-id".to_string(),
                    alias: None,
                    device_name: "Device 2".to_string(),
                    platform: "linux".to_string(),
                    shell: "bash".to_string(),
                    status: DeviceStatus::Pending,
                    created_at: Utc::now(),
                    last_seen: Utc::now(),
                },
            ],
        };

        let result = DeviceStore::validate_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Duplicate device_id"));
    }

    #[test]
    fn test_validate_config_duplicate_alias() {
        let config = DevicesConfig {
            max_devices: 10,
            secret_key: "test".to_string(),
            devices: vec![
                DeviceInfo {
                    device_id: "id-1".to_string(),
                    alias: Some("mypc".to_string()),
                    device_name: "Device 1".to_string(),
                    platform: "windows".to_string(),
                    shell: "powershell".to_string(),
                    status: DeviceStatus::Approved,
                    created_at: Utc::now(),
                    last_seen: Utc::now(),
                },
                DeviceInfo {
                    device_id: "id-2".to_string(),
                    alias: Some("mypc".to_string()),
                    device_name: "Device 2".to_string(),
                    platform: "linux".to_string(),
                    shell: "bash".to_string(),
                    status: DeviceStatus::Pending,
                    created_at: Utc::now(),
                    last_seen: Utc::now(),
                },
            ],
        };

        let result = DeviceStore::validate_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Duplicate alias"));
    }
}
