//! Protocol definitions for osh-daemon
//!
//! Defines message types for communication between Local Daemon and Relay Service.

use serde::{Deserialize, Serialize};

// ============================================================================
// Heartbeat Messages
// ============================================================================

/// Heartbeat PING message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PingMessage {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub ts: i64,
}

impl PingMessage {
    pub fn new() -> Self {
        Self {
            msg_type: "PING".to_string(),
            ts: timestamp_ms(),
        }
    }
}

/// Heartbeat PONG message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PongMessage {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub ts: i64,
}

impl PongMessage {
    pub fn new(ts: i64) -> Self {
        Self {
            msg_type: "PONG".to_string(),
            ts,
        }
    }
}

// ============================================================================
// Pairing Messages
// ============================================================================

/// Pair request message (Client -> VPS)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairRequest {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub device_id: String,
    pub device_name: String,
    pub platform: String,
    pub shell: String,
    pub secret_key: String,
}

impl PairRequest {
    pub fn new(
        device_id: String,
        device_name: String,
        platform: String,
        shell: String,
        secret_key: String,
    ) -> Self {
        Self {
            msg_type: "PAIR_REQUEST".to_string(),
            device_id,
            device_name,
            platform,
            shell,
            secret_key,
        }
    }
}

// Note: These types are defined for documentation and potential future use
// but are currently handled through GenericMessage for flexibility
#[allow(dead_code)]
/// Pair response types
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairResponseType {
    Approved,
    Pending,
    Rejected,
    AuthFailed,
    LimitExceeded,
}

/// Pair response message (VPS -> Client)
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairResponse {
    #[serde(rename = "type")]
    pub msg_type: String,
    #[serde(default)]
    pub message: Option<String>,
}

#[allow(dead_code)]
impl PairResponse {
    pub fn response_type(&self) -> Option<PairResponseType> {
        match self.msg_type.as_str() {
            "PAIR_APPROVED" => Some(PairResponseType::Approved),
            "PAIR_PENDING" => Some(PairResponseType::Pending),
            "PAIR_REJECTED" => Some(PairResponseType::Rejected),
            "PAIR_AUTH_FAILED" => Some(PairResponseType::AuthFailed),
            "PAIR_LIMIT_EXCEEDED" => Some(PairResponseType::LimitExceeded),
            _ => None,
        }
    }
}

// ============================================================================
// Command Messages (VPS -> Client)
// These are documented types; actual parsing uses GenericMessage
// ============================================================================

/// Spawn command message
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnCommand {
    pub req_id: String,
    #[serde(rename = "type")]
    pub msg_type: String,
    pub payload: String,  // base64 encoded command
    #[serde(default)]
    pub cols: Option<u16>,
    #[serde(default)]
    pub rows: Option<u16>,
}

/// Data input message
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataCommand {
    pub req_id: String,
    #[serde(rename = "type")]
    pub msg_type: String,
    pub payload: String,  // base64 encoded data
}

/// Resize command message
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResizeCommand {
    pub req_id: String,
    #[serde(rename = "type")]
    pub msg_type: String,
    pub cols: u16,
    pub rows: u16,
}

/// Kill command message
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KillCommand {
    pub req_id: String,
    #[serde(rename = "type")]
    pub msg_type: String,
}

/// Unified command message from VPS
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VpsCommand {
    pub req_id: String,
    #[serde(rename = "type")]
    pub msg_type: String,
    #[serde(default)]
    pub payload: Option<String>,
    #[serde(default)]
    pub cols: Option<u16>,
    #[serde(default)]
    pub rows: Option<u16>,
}

#[allow(dead_code)]
impl VpsCommand {
    pub fn command_type(&self) -> CommandType {
        match self.msg_type.as_str() {
            "SPAWN" => CommandType::Spawn,
            "DATA" => CommandType::Data,
            "RESIZE" => CommandType::Resize,
            "KILL" => CommandType::Kill,
            "INVALID_REQ_ID" => CommandType::InvalidReqId,
            _ => CommandType::Unknown,
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandType {
    Spawn,
    Data,
    Resize,
    Kill,
    InvalidReqId,
    Unknown,
}

// ============================================================================
// Response Messages (Client -> VPS)
// ============================================================================

/// Data output message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataResponse {
    pub req_id: String,
    #[serde(rename = "type")]
    pub msg_type: String,
    pub payload: String,  // base64 encoded
}

impl DataResponse {
    pub fn new(req_id: String, data: &[u8]) -> Self {
        Self {
            req_id,
            msg_type: "DATA".to_string(),
            payload: encode_payload(data),
        }
    }
}

/// Exit message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExitResponse {
    pub req_id: String,
    #[serde(rename = "type")]
    pub msg_type: String,
    pub exit_code: i32,
}

impl ExitResponse {
    pub fn new(req_id: String, exit_code: i32) -> Self {
        Self {
            req_id,
            msg_type: "EXIT".to_string(),
            exit_code,
        }
    }
}

/// Error message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub req_id: String,
    #[serde(rename = "type")]
    pub msg_type: String,
    pub error: String,
}

impl ErrorResponse {
    pub fn new(req_id: String, error: String) -> Self {
        Self {
            req_id,
            msg_type: "ERROR".to_string(),
            error,
        }
    }
}

// ============================================================================
// Generic Message for Parsing
// ============================================================================

/// Generic message for initial parsing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenericMessage {
    #[serde(rename = "type")]
    pub msg_type: String,
    #[serde(default)]
    pub req_id: Option<String>,
    #[serde(default)]
    pub ts: Option<i64>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub payload: Option<String>,
    #[serde(default)]
    pub cols: Option<u16>,
    #[serde(default)]
    pub rows: Option<u16>,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub pty: Option<bool>,
}

impl GenericMessage {
    pub fn is_ping(&self) -> bool {
        self.msg_type == "PING"
    }
    
    pub fn is_pong(&self) -> bool {
        self.msg_type == "PONG"
    }
    
    pub fn is_pair_response(&self) -> bool {
        matches!(
            self.msg_type.as_str(),
            "PAIR_APPROVED" | "PAIR_PENDING" | "PAIR_REJECTED" | "PAIR_AUTH_FAILED" | "PAIR_LIMIT_EXCEEDED"
        )
    }
    
    pub fn is_command(&self) -> bool {
        matches!(
            self.msg_type.as_str(),
            "SPAWN" | "DATA" | "RESIZE" | "KILL" | "INVALID_REQ_ID"
        )
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Get current timestamp in milliseconds
pub fn timestamp_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// Encode data to base64
pub fn encode_payload(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

/// Decode base64 payload
pub fn decode_payload(payload: &str) -> anyhow::Result<Vec<u8>> {
    use base64::Engine;
    Ok(base64::engine::general_purpose::STANDARD.decode(payload)?)
}
