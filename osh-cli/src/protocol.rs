//! OSH Protocol Definitions
//!
//! Shared message types for communication between osh/osh-admin and Relay Service.

use serde::{Deserialize, Serialize};

/// Device status in the system
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DeviceStatus {
    Pending,
    Approved,
    Rejected,
}

impl std::fmt::Display for DeviceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeviceStatus::Pending => write!(f, "pending"),
            DeviceStatus::Approved => write!(f, "approved"),
            DeviceStatus::Rejected => write!(f, "rejected"),
        }
    }
}

/// Device information stored in devices.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub device_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    pub device_name: String,
    pub platform: String,
    pub shell: String,
    pub status: DeviceStatus,
    pub created_at: String,
    pub last_seen: String,
}

/// Root structure of devices.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevicesConfig {
    pub max_devices: u32,
    pub secret_key: String,
    pub devices: Vec<Device>,
}

impl DevicesConfig {
    /// Find a device by device_id or alias (supports prefix matching)
    pub fn find_device(&self, identifier: &str) -> Option<&Device> {
        // First try exact match on alias
        if let Some(device) = self.devices.iter().find(|d| {
            d.alias.as_ref().map(|a| a == identifier).unwrap_or(false)
        }) {
            return Some(device);
        }
        
        // Then try exact match on device_id
        if let Some(device) = self.devices.iter().find(|d| d.device_id == identifier) {
            return Some(device);
        }
        
        // Finally try prefix match on device_id (must be unique)
        let matches: Vec<_> = self.devices.iter()
            .filter(|d| d.device_id.starts_with(identifier))
            .collect();
        
        if matches.len() == 1 {
            return Some(matches[0]);
        }
        
        None
    }
    
    /// Find a device by device_id or alias (mutable, supports prefix matching)
    pub fn find_device_mut(&mut self, identifier: &str) -> Option<&mut Device> {
        // First try exact match on alias
        let alias_idx = self.devices.iter().position(|d| {
            d.alias.as_ref().map(|a| a == identifier).unwrap_or(false)
        });
        if let Some(idx) = alias_idx {
            return Some(&mut self.devices[idx]);
        }
        
        // Then try exact match on device_id
        let exact_idx = self.devices.iter().position(|d| d.device_id == identifier);
        if let Some(idx) = exact_idx {
            return Some(&mut self.devices[idx]);
        }
        
        // Finally try prefix match on device_id (must be unique)
        let matches: Vec<_> = self.devices.iter()
            .enumerate()
            .filter(|(_, d)| d.device_id.starts_with(identifier))
            .map(|(i, _)| i)
            .collect();
        
        if matches.len() == 1 {
            return Some(&mut self.devices[matches[0]]);
        }
        
        None
    }
    
    /// Remove a device by device_id or alias
    pub fn remove_device(&mut self, identifier: &str) -> Option<Device> {
        // First try exact match on alias
        let alias_idx = self.devices.iter().position(|d| {
            d.alias.as_ref().map(|a| a == identifier).unwrap_or(false)
        });
        if let Some(idx) = alias_idx {
            return Some(self.devices.remove(idx));
        }
        
        // Then try exact match on device_id
        let exact_idx = self.devices.iter().position(|d| d.device_id == identifier);
        if let Some(idx) = exact_idx {
            return Some(self.devices.remove(idx));
        }
        
        // Finally try prefix match on device_id (must be unique)
        let matches: Vec<_> = self.devices.iter()
            .enumerate()
            .filter(|(_, d)| d.device_id.starts_with(identifier))
            .map(|(i, _)| i)
            .collect();
        
        if matches.len() == 1 {
            return Some(self.devices.remove(matches[0]));
        }
        
        None
    }
    
    /// Check if an alias is already in use
    pub fn is_alias_used(&self, alias: &str) -> bool {
        self.devices.iter().any(|d| {
            d.alias.as_ref().map(|a| a == alias).unwrap_or(false)
        })
    }
}

// ============================================================================
// WebSocket Protocol Messages
// ============================================================================

/// Heartbeat messages
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum HeartbeatMessage {
    PING { ts: i64 },
    PONG { ts: i64 },
}

/// Admin command messages (osh-admin -> Relay)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AdminRequest {
    ADMIN_RELOAD,
}

/// Admin response messages (Relay -> osh-admin)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AdminResponse {
    ADMIN_RELOAD_OK { message: String },
    ADMIN_RELOAD_ERROR { message: String },
}

/// Exec request message (osh -> Relay)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecRequest {
    #[serde(rename = "type")]
    pub msg_type: String,  // "EXEC"
    pub device: String,
    pub req_id: String,
    pub cmd: String,
    pub cols: u16,
    pub rows: u16,
    #[serde(default)]
    pub pty: bool,
}

impl ExecRequest {
    pub fn new(device: String, req_id: String, cmd: String, cols: u16, rows: u16, pty: bool) -> Self {
        Self {
            msg_type: "EXEC".to_string(),
            device,
            req_id,
            cmd,
            cols,
            rows,
            pty,
        }
    }
}

/// Input message (osh -> Relay)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputMessage {
    #[serde(rename = "type")]
    pub msg_type: String,  // "INPUT"
    pub req_id: String,
    pub payload: String,  // base64 encoded
}

impl InputMessage {
    pub fn new(req_id: String, data: &[u8]) -> Self {
        use base64::Engine;
        Self {
            msg_type: "INPUT".to_string(),
            req_id,
            payload: base64::engine::general_purpose::STANDARD.encode(data),
        }
    }
}

/// Resize message (osh -> Relay)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResizeMessage {
    #[serde(rename = "type")]
    pub msg_type: String,  // "RESIZE"
    pub req_id: String,
    pub cols: u16,
    pub rows: u16,
}

impl ResizeMessage {
    pub fn new(req_id: String, cols: u16, rows: u16) -> Self {
        Self {
            msg_type: "RESIZE".to_string(),
            req_id,
            cols,
            rows,
        }
    }
}

/// Response messages from Relay to osh
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RelayResponse {
    DATA {
        req_id: String,
        payload: String,  // base64 encoded
    },
    EXIT {
        req_id: String,
        exit_code: i32,
    },
    ERROR {
        req_id: String,
        message: String,
    },
    DEVICE_OFFLINE {
        req_id: String,
        message: String,
    },
    DEVICE_NOT_APPROVED {
        req_id: String,
        message: String,
    },
    DEVICE_NOT_FOUND {
        req_id: String,
        message: String,
    },
    INVALID_REQ_ID {
        req_id: String,
        message: String,
    },
    // Heartbeat
    PING { ts: i64 },
    PONG { ts: i64 },
    // Admin responses
    ADMIN_RELOAD_OK { message: String },
    ADMIN_RELOAD_ERROR { message: String },
}

/// Helper to decode base64 payload
pub fn decode_payload(payload: &str) -> anyhow::Result<Vec<u8>> {
    use base64::Engine;
    Ok(base64::engine::general_purpose::STANDARD.decode(payload)?)
}

/// Helper to decode base64 payload and filter problematic escape sequences
/// from Windows Terminal / PowerShell that break Linux terminals
pub fn decode_and_filter_payload(payload: &str) -> anyhow::Result<Vec<u8>> {
    use base64::Engine;
    let data = base64::engine::general_purpose::STANDARD.decode(payload)?;
    Ok(filter_terminal_sequences(&data))
}

/// Filter out problematic terminal escape sequences
fn filter_terminal_sequences(data: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(data.len());
    let mut i = 0;
    
    while i < data.len() {
        // Check for ESC (0x1b)
        if data[i] == 0x1b {
            if let Some(seq_len) = try_filter_escape_sequence(&data[i..]) {
                // Skip this sequence entirely
                i += seq_len;
                continue;
            }
        }
        
        result.push(data[i]);
        i += 1;
    }
    
    result
}

/// Try to identify and filter a problematic escape sequence
/// Returns Some(length) if sequence should be filtered, None otherwise
fn try_filter_escape_sequence(data: &[u8]) -> Option<usize> {
    if data.len() < 2 || data[0] != 0x1b {
        return None;
    }
    
    match data[1] {
        b'[' => try_filter_csi(data),
        b']' => try_filter_osc(data),
        _ => None,
    }
}

/// Try to filter CSI sequences (ESC [ ...)
fn try_filter_csi(data: &[u8]) -> Option<usize> {
    if data.len() < 3 {
        return None;
    }
    
    let mut i = 2;
    let has_question = data[i] == b'?';
    let has_gt = data[i] == b'>';
    let has_eq = data[i] == b'=';
    
    if has_question || has_gt || has_eq {
        i += 1;
    }
    
    // Collect the sequence until we find the final byte (0x40-0x7e)
    let start = i;
    while i < data.len() && (data[i].is_ascii_digit() || data[i] == b';') {
        i += 1;
    }
    
    if i >= data.len() {
        return None;
    }
    
    let final_byte = data[i];
    if final_byte < 0x40 || final_byte > 0x7e {
        return None;
    }
    
    let seq_len = i + 1;
    let params_str = std::str::from_utf8(&data[start..i]).unwrap_or("");
    
    // Decide what to filter
    if has_question {
        // DEC private modes
        match final_byte {
            b'h' | b'l' => {
                // Filter problematic modes
                for param in params_str.split(';') {
                    match param {
                        "9001" |  // Win32 Input Mode
                        "1004" |  // Focus Reporting
                        "1049" |  // Alternate screen buffer
                        "2004" |  // Bracketed paste
                        "25"      // Cursor visibility (let PowerShell control mess)
                            => return Some(seq_len),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    } else if has_gt {
        // xterm modifyOtherKeys etc - filter all
        return Some(seq_len);
    } else if has_eq {
        // Kitty keyboard protocol - filter all
        return Some(seq_len);
    } else {
        // Standard CSI sequences
        match final_byte {
            b'J' => {
                // ED - Erase in Display
                // Filter \x1b[2J (clear screen) and \x1b[3J (clear scrollback)
                if params_str == "2" || params_str == "3" || params_str == "" {
                    return Some(seq_len);
                }
            }
            b'H' => {
                // CUP - Cursor Position
                // Filter \x1b[H (cursor to home) - often used with clear
                if params_str.is_empty() || params_str == "1;1" {
                    return Some(seq_len);
                }
            }
            _ => {}
        }
    }
    
    None
}

/// Try to filter OSC sequences (ESC ] ... BEL/ST)
fn try_filter_osc(data: &[u8]) -> Option<usize> {
    if data.len() < 3 {
        return None;
    }
    
    // Find terminator: BEL (0x07) or ST (ESC \)
    for i in 2..data.len() {
        if data[i] == 0x07 {
            return Some(i + 1);
        }
        if data[i] == 0x1b && i + 1 < data.len() && data[i + 1] == b'\\' {
            return Some(i + 2);
        }
    }
    
    None
}

/// Helper to encode data to base64
pub fn encode_payload(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

/// Get current timestamp in milliseconds
pub fn timestamp_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
