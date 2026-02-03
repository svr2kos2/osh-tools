//! Protocol message definitions for OSH Relay Service
//!
//! All messages are serialized as JSON and transmitted over WebSocket Binary Frames.

#![allow(non_camel_case_types)]
#![allow(dead_code)]

use serde::{Deserialize, Serialize};

// ============================================================================
// Heartbeat Messages
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum HeartbeatMessage {
    PING { ts: i64 },
    PONG { ts: i64 },
}

// ============================================================================
// Pairing Messages (Client <-> Relay)
// ============================================================================

/// Client -> Relay: Pairing request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairRequest {
    pub device_id: String,
    pub device_name: String,
    pub platform: String,
    pub shell: String,
    pub secret_key: String,
}

/// Pairing response types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PairResponseType {
    PAIR_APPROVED,
    PAIR_REJECTED,
    PAIR_PENDING,
    PAIR_AUTH_FAILED,
    PAIR_LIMIT_EXCEEDED,
}

/// Relay -> Client: Pairing response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairResponse {
    #[serde(rename = "type")]
    pub response_type: PairResponseType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

// ============================================================================
// Command Messages (VPS -> Client)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientCommand {
    /// Spawn a new process
    SPAWN {
        req_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        payload: Option<String>, // base64 encoded command
        #[serde(skip_serializing_if = "Option::is_none")]
        cols: Option<u16>,
        #[serde(skip_serializing_if = "Option::is_none")]
        rows: Option<u16>,
        #[serde(default)]
        pty: bool,
    },
    /// Send data to process stdin
    DATA {
        req_id: String,
        payload: String, // base64 encoded
    },
    /// Resize terminal
    RESIZE {
        req_id: String,
        cols: u16,
        rows: u16,
    },
    /// Kill the process
    KILL { req_id: String },
}

// ============================================================================
// Client Events (Client -> VPS)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientEvent {
    /// Output data from process
    DATA {
        req_id: String,
        payload: String, // base64 encoded
    },
    /// Process exited
    EXIT {
        req_id: String,
        exit_code: i32,
    },
    /// Error occurred
    ERROR {
        req_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
}

// ============================================================================
// OSH CLI Messages (osh <-> Relay)
// ============================================================================

/// osh -> Relay: Execute command request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecRequest {
    pub device: String, // device_id or alias
    pub req_id: String,
    pub cmd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cols: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows: Option<u16>,
    #[serde(default)]
    pub pty: bool,
}

/// osh -> Relay: Input data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputData {
    pub req_id: String,
    pub payload: String, // base64 encoded
}

/// osh -> Relay: Resize terminal
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResizeRequest {
    pub req_id: String,
    pub cols: u16,
    pub rows: u16,
}

// ============================================================================
// Admin Messages (osh-admin <-> Relay)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AdminMessage {
    /// Reload configuration request
    ADMIN_RELOAD,
    /// Reload success response
    ADMIN_RELOAD_OK { message: String },
    /// Reload error response
    ADMIN_RELOAD_ERROR { message: String },
}

// ============================================================================
// Error Messages
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ErrorMessage {
    INVALID_REQ_ID {
        req_id: String,
        message: String,
    },
    DEVICE_NOT_APPROVED {
        req_id: String,
        message: String,
    },
    DEVICE_OFFLINE {
        req_id: String,
        message: String,
    },
    DEVICE_NOT_FOUND {
        req_id: String,
        message: String,
    },
    ERROR {
        req_id: String,
        message: String,
    },
}

// ============================================================================
// Unified Message Types
// ============================================================================

/// Messages from Client (Local Daemon)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMessage {
    // Heartbeat
    PING { ts: i64 },
    PONG { ts: i64 },
    // Pairing
    PAIR_REQUEST {
        device_id: String,
        device_name: String,
        platform: String,
        shell: String,
        secret_key: String,
    },
    // Events
    DATA { req_id: String, payload: String },
    EXIT { req_id: String, exit_code: i32 },
    ERROR { req_id: String, error: Option<String> },
}

/// Messages to Client (Local Daemon)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RelayToClientMessage {
    // Heartbeat
    PING { ts: i64 },
    PONG { ts: i64 },
    // Pairing responses
    PAIR_APPROVED { message: Option<String> },
    PAIR_REJECTED { message: Option<String> },
    PAIR_PENDING { message: Option<String> },
    PAIR_AUTH_FAILED { message: Option<String> },
    PAIR_LIMIT_EXCEEDED { message: Option<String> },
    // Commands
    SPAWN {
        req_id: String,
        payload: Option<String>,
        cols: Option<u16>,
        rows: Option<u16>,
        #[serde(default)]
        pty: bool,
    },
    DATA { req_id: String, payload: String },
    RESIZE { req_id: String, cols: u16, rows: u16 },
    KILL { req_id: String },
    // Errors
    INVALID_REQ_ID { req_id: String, message: String },
}

/// Messages from OSH CLI
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum OshMessage {
    // Heartbeat
    PING { ts: i64 },
    PONG { ts: i64 },
    // Commands
    EXEC {
        device: String,
        req_id: String,
        cmd: String,
        cols: Option<u16>,
        rows: Option<u16>,
        #[serde(default)]
        pty: bool,
    },
    INPUT { req_id: String, payload: String },
    RESIZE { req_id: String, cols: u16, rows: u16 },
    // Admin
    ADMIN_RELOAD,
}

/// Messages to OSH CLI
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RelayToOshMessage {
    // Heartbeat
    PING { ts: i64 },
    PONG { ts: i64 },
    // Data from client
    DATA { req_id: String, payload: String },
    EXIT { req_id: String, exit_code: i32 },
    // Errors
    ERROR { req_id: String, message: String },
    DEVICE_OFFLINE { req_id: String, message: String },
    DEVICE_NOT_APPROVED { req_id: String, message: String },
    DEVICE_NOT_FOUND { req_id: String, message: String },
    INVALID_REQ_ID { req_id: String, message: String },
    // Admin responses
    ADMIN_RELOAD_OK { message: String },
    ADMIN_RELOAD_ERROR { message: String },
}
