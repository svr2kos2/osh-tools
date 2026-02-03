//! Relay Service core implementation
//!
//! Handles:
//! - Device connections and pairing
//! - OSH CLI request routing
//! - Message forwarding between osh CLI and clients
//! - Heartbeat monitoring

#![allow(dead_code)]

use anyhow::Result;
use chrono::Utc;
use futures_util::stream::SplitStream;
use futures_util::StreamExt;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, RwLock};
use tokio::time::interval;
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};
use tracing::{debug, error, info, warn};

use crate::device_store::{DeviceInfo, DeviceStatus, DeviceStore};
use crate::protocol::{ClientMessage, OshMessage, RelayToClientMessage, RelayToOshMessage};

/// Heartbeat interval in seconds
const HEARTBEAT_INTERVAL_SECS: u64 = 1;
/// Heartbeat timeout in seconds
const HEARTBEAT_TIMEOUT_SECS: u64 = 2;

/// Connected client information
struct ConnectedClient {
    device_id: String,
    sender: mpsc::UnboundedSender<Message>,
    last_activity: Instant,
    approved: bool,
}

/// OSH CLI session information
struct OshSession {
    req_id: String,
    device_id: String,
    osh_addr: SocketAddr,  // To identify which osh-cli owns this session
    sender: mpsc::UnboundedSender<Message>,
    last_activity: Instant,
}

/// Main relay service
pub struct RelayService {
    /// Device store
    device_store: Arc<DeviceStore>,
    /// Connected clients: device_id -> client
    clients: RwLock<HashMap<String, ConnectedClient>>,
    /// OSH sessions: req_id -> session
    osh_sessions: RwLock<HashMap<String, OshSession>>,
    /// OSH connections (for admin reload notifications): addr -> sender
    osh_connections: RwLock<HashMap<SocketAddr, mpsc::UnboundedSender<Message>>>,
}

impl RelayService {
    /// Create a new relay service
    pub async fn new(device_store: Arc<DeviceStore>) -> Self {
        Self {
            device_store,
            clients: RwLock::new(HashMap::new()),
            osh_sessions: RwLock::new(HashMap::new()),
            osh_connections: RwLock::new(HashMap::new()),
        }
    }

    /// Handle a client (Local Daemon) connection
    pub async fn handle_client_connection(
        &self,
        addr: SocketAddr,
        sender: mpsc::UnboundedSender<Message>,
        mut receiver: SplitStream<WebSocketStream<TcpStream>>,
    ) {
        let mut device_id: Option<String> = None;
        let mut last_activity = Instant::now();
        let mut heartbeat_interval = interval(Duration::from_secs(HEARTBEAT_INTERVAL_SECS));

        loop {
            tokio::select! {
                // Heartbeat tick
                _ = heartbeat_interval.tick() => {
                    // Check timeout
                    if last_activity.elapsed() > Duration::from_secs(HEARTBEAT_TIMEOUT_SECS) {
                        warn!("Client {} timed out", addr);
                        break;
                    }

                    // Send ping
                    let ping = RelayToClientMessage::PING { ts: Utc::now().timestamp_millis() };
                    if let Err(e) = self.send_to_client_raw(&sender, &ping) {
                        warn!("Failed to send ping to {}: {}", addr, e);
                        break;
                    }
                }

                // Receive message
                msg = receiver.next() => {
                    match msg {
                        Some(Ok(Message::Binary(data))) => {
                            last_activity = Instant::now();

                            // Update last activity in clients map
                            if let Some(ref did) = device_id {
                                let mut clients = self.clients.write().await;
                                if let Some(client) = clients.get_mut(did) {
                                    client.last_activity = last_activity;
                                }
                            }

                            match serde_json::from_slice::<ClientMessage>(&data) {
                                Ok(client_msg) => {
                                    match self.handle_client_message(addr, &sender, &mut device_id, client_msg).await {
                                        Ok(should_continue) => {
                                            if !should_continue {
                                                break;
                                            }
                                        }
                                        Err(e) => {
                                            error!("Error handling client message from {}: {}", addr, e);
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!("Failed to parse client message from {}: {}", addr, e);
                                }
                            }
                        }
                        Some(Ok(Message::Close(_))) => {
                            info!("Client {} sent close frame", addr);
                            break;
                        }
                        Some(Ok(_)) => {
                            // Ignore text and other message types
                        }
                        Some(Err(e)) => {
                            warn!("WebSocket error from client {}: {}", addr, e);
                            break;
                        }
                        None => {
                            info!("Client {} disconnected", addr);
                            break;
                        }
                    }
                }
            }
        }

        // Cleanup on disconnect
        if let Some(did) = device_id {
            self.on_client_disconnect(&did).await;
        }
    }

    /// Handle a message from client
    async fn handle_client_message(
        &self,
        addr: SocketAddr,
        sender: &mpsc::UnboundedSender<Message>,
        device_id: &mut Option<String>,
        msg: ClientMessage,
    ) -> Result<bool> {
        match msg {
            ClientMessage::PING { ts } => {
                // Reply with PONG
                let pong = RelayToClientMessage::PONG { ts };
                self.send_to_client_raw(sender, &pong)?;
            }
            ClientMessage::PONG { .. } => {
                // Just update activity time (already done above)
            }
            ClientMessage::PAIR_REQUEST {
                device_id: did,
                device_name,
                platform,
                shell,
                secret_key,
            } => {
                info!(
                    "Pairing request from {} (device_id: {}, name: {})",
                    addr, did, device_name
                );

                // Check secret key
                let expected_key = self.device_store.get_secret_key().await;
                if secret_key != expected_key {
                    warn!("Auth failed for device {}: invalid secret key", did);
                    let response = RelayToClientMessage::PAIR_AUTH_FAILED {
                        message: Some("Invalid secret key".to_string()),
                    };
                    self.send_to_client_raw(sender, &response)?;
                    return Ok(false); // Disconnect
                }

                // Check device limit
                if !self.device_store.device_exists(&did).await {
                    let current_count = self.device_store.get_device_count().await;
                    let max_devices = self.device_store.get_max_devices().await;
                    if current_count >= max_devices {
                        warn!("Device limit reached, rejecting {}", did);
                        let response = RelayToClientMessage::PAIR_LIMIT_EXCEEDED {
                            message: Some(format!(
                                "Device limit reached ({}/{})",
                                current_count, max_devices
                            )),
                        };
                        self.send_to_client_raw(sender, &response)?;
                        return Ok(false); // Disconnect
                    }
                }

                // Check if device already exists
                let existing = self.device_store.get_device_by_id(&did).await;
                let (status, response) = match existing {
                    Some(device) => {
                        // Update device info
                        let updated = DeviceInfo {
                            device_id: did.clone(),
                            alias: device.alias,
                            device_name,
                            platform,
                            shell,
                            status: device.status,
                            created_at: device.created_at,
                            last_seen: Utc::now(),
                        };
                        self.device_store.upsert_device(updated).await?;

                        match device.status {
                            DeviceStatus::Approved => (
                                true,
                                RelayToClientMessage::PAIR_APPROVED {
                                    message: Some("Welcome back".to_string()),
                                },
                            ),
                            DeviceStatus::Pending => (
                                false,
                                RelayToClientMessage::PAIR_PENDING {
                                    message: Some("Waiting for admin approval".to_string()),
                                },
                            ),
                            DeviceStatus::Rejected => (
                                false,
                                RelayToClientMessage::PAIR_REJECTED {
                                    message: Some("Device was rejected".to_string()),
                                },
                            ),
                        }
                    }
                    None => {
                        // New device - create with pending status
                        let new_device = DeviceInfo {
                            device_id: did.clone(),
                            alias: None,
                            device_name,
                            platform,
                            shell,
                            status: DeviceStatus::Pending,
                            created_at: Utc::now(),
                            last_seen: Utc::now(),
                        };
                        self.device_store.upsert_device(new_device).await?;
                        info!("New device registered: {}", did);
                        (
                            false,
                            RelayToClientMessage::PAIR_PENDING {
                                message: Some("Device registered, waiting for admin approval".to_string()),
                            },
                        )
                    }
                };

                self.send_to_client_raw(sender, &response)?;

                // Store device_id and register in clients map
                *device_id = Some(did.clone());
                let mut clients = self.clients.write().await;
                clients.insert(
                    did.clone(),
                    ConnectedClient {
                        device_id: did,
                        sender: sender.clone(),
                        last_activity: Instant::now(),
                        approved: status,
                    },
                );

                if matches!(response, RelayToClientMessage::PAIR_REJECTED { .. }) {
                    return Ok(false); // Disconnect rejected devices
                }
            }
            ClientMessage::DATA { req_id, payload } => {
                // Forward to OSH session
                let sessions = self.osh_sessions.read().await;
                if let Some(session) = sessions.get(&req_id) {
                    let msg = RelayToOshMessage::DATA { req_id, payload };
                    if let Err(e) = self.send_to_osh_raw(&session.sender, &msg) {
                        warn!("Failed to forward data to OSH: {}", e);
                    }
                } else {
                    debug!("Unknown req_id in DATA: {}", req_id);
                }
            }
            ClientMessage::EXIT { req_id, exit_code } => {
                info!("Process exited: req_id={}, exit_code={}", req_id, exit_code);
                // Forward to OSH session
                let sessions = self.osh_sessions.read().await;
                if let Some(session) = sessions.get(&req_id) {
                    let msg = RelayToOshMessage::EXIT { req_id: req_id.clone(), exit_code };
                    if let Err(e) = self.send_to_osh_raw(&session.sender, &msg) {
                        warn!("Failed to forward exit to OSH: {}", e);
                    }
                }
                // Clean up session
                drop(sessions);
                self.osh_sessions.write().await.remove(&req_id);
            }
            ClientMessage::ERROR { req_id, error } => {
                warn!("Client error for req_id {}: {:?}", req_id, error);
                // Forward to OSH session
                let sessions = self.osh_sessions.read().await;
                if let Some(session) = sessions.get(&req_id) {
                    let msg = RelayToOshMessage::ERROR {
                        req_id: req_id.clone(),
                        message: error.unwrap_or_else(|| "Unknown error".to_string()),
                    };
                    if let Err(e) = self.send_to_osh_raw(&session.sender, &msg) {
                        warn!("Failed to forward error to OSH: {}", e);
                    }
                }
                // Clean up session
                drop(sessions);
                self.osh_sessions.write().await.remove(&req_id);
            }
        }

        Ok(true)
    }

    /// Handle client disconnect
    async fn on_client_disconnect(&self, device_id: &str) {
        info!("Client disconnected: {}", device_id);

        // Remove from clients map
        self.clients.write().await.remove(device_id);

        // Find all sessions for this device and notify OSH
        let mut to_remove = Vec::new();
        {
            let sessions = self.osh_sessions.read().await;
            for (req_id, session) in sessions.iter() {
                if session.device_id == device_id {
                    let msg = RelayToOshMessage::DEVICE_OFFLINE {
                        req_id: req_id.clone(),
                        message: "Device disconnected".to_string(),
                    };
                    let _ = self.send_to_osh_raw(&session.sender, &msg);
                    to_remove.push(req_id.clone());
                }
            }
        }

        // Remove sessions
        let mut sessions = self.osh_sessions.write().await;
        for req_id in to_remove {
            sessions.remove(&req_id);
        }
    }

    /// Handle an OSH CLI or osh-admin connection
    pub async fn handle_osh_connection(
        &self,
        addr: SocketAddr,
        sender: mpsc::UnboundedSender<Message>,
        mut receiver: SplitStream<WebSocketStream<TcpStream>>,
    ) {
        // Register connection
        self.osh_connections
            .write()
            .await
            .insert(addr, sender.clone());

        let mut last_activity = Instant::now();
        let mut heartbeat_interval = interval(Duration::from_secs(HEARTBEAT_INTERVAL_SECS));

        loop {
            tokio::select! {
                // Heartbeat tick
                _ = heartbeat_interval.tick() => {
                    // Check timeout
                    if last_activity.elapsed() > Duration::from_secs(HEARTBEAT_TIMEOUT_SECS) {
                        warn!("OSH connection {} timed out", addr);
                        break;
                    }

                    // Send ping
                    let ping = RelayToOshMessage::PING { ts: Utc::now().timestamp_millis() };
                    if let Err(e) = self.send_to_osh_raw(&sender, &ping) {
                        warn!("Failed to send ping to OSH {}: {}", addr, e);
                        break;
                    }
                }

                // Receive message
                msg = receiver.next() => {
                    match msg {
                        Some(Ok(Message::Binary(data))) => {
                            last_activity = Instant::now();

                            match serde_json::from_slice::<OshMessage>(&data) {
                                Ok(osh_msg) => {
                                    if let Err(e) = self.handle_osh_message(addr, &sender, osh_msg).await {
                                        error!("Error handling OSH message from {}: {}", addr, e);
                                    }
                                }
                                Err(e) => {
                                    warn!("Failed to parse OSH message from {}: {}", addr, e);
                                }
                            }
                        }
                        Some(Ok(Message::Close(_))) => {
                            info!("OSH {} sent close frame", addr);
                            break;
                        }
                        Some(Ok(_)) => {
                            // Ignore text and other message types
                        }
                        Some(Err(e)) => {
                            warn!("WebSocket error from OSH {}: {}", addr, e);
                            break;
                        }
                        None => {
                            info!("OSH {} disconnected", addr);
                            break;
                        }
                    }
                }
            }
        }

        // Cleanup on disconnect
        self.osh_connections.write().await.remove(&addr);

        // Clean up any sessions owned by this connection and send KILL to daemons
        self.cleanup_osh_sessions(addr).await;
    }

    /// Clean up sessions for a disconnected OSH CLI and notify daemons to kill processes
    async fn cleanup_osh_sessions(&self, osh_addr: SocketAddr) {
        let mut sessions_to_remove = Vec::new();
        
        // Find all sessions owned by this osh-cli
        {
            let sessions = self.osh_sessions.read().await;
            for (req_id, session) in sessions.iter() {
                if session.osh_addr == osh_addr {
                    sessions_to_remove.push((req_id.clone(), session.device_id.clone()));
                }
            }
        }
        
        if sessions_to_remove.is_empty() {
            return;
        }
        
        info!("Cleaning up {} sessions for disconnected OSH {}", sessions_to_remove.len(), osh_addr);
        
        // Send KILL to each daemon and remove sessions
        let clients = self.clients.read().await;
        for (req_id, device_id) in &sessions_to_remove {
            if let Some(client) = clients.get(device_id) {
                let kill_msg = RelayToClientMessage::KILL { req_id: req_id.clone() };
                if let Err(e) = self.send_to_client_raw(&client.sender, &kill_msg) {
                    warn!("Failed to send KILL to device {}: {}", device_id, e);
                } else {
                    info!("Sent KILL for req_id={} to device {}", req_id, device_id);
                }
            }
        }
        drop(clients);
        
        // Remove sessions
        let mut sessions = self.osh_sessions.write().await;
        for (req_id, _) in sessions_to_remove {
            sessions.remove(&req_id);
        }
    }

    /// Handle a message from OSH CLI
    async fn handle_osh_message(
        &self,
        addr: SocketAddr,
        sender: &mpsc::UnboundedSender<Message>,
        msg: OshMessage,
    ) -> Result<()> {
        match msg {
            OshMessage::PING { ts } => {
                let pong = RelayToOshMessage::PONG { ts };
                self.send_to_osh_raw(sender, &pong)?;
            }
            OshMessage::PONG { .. } => {
                // Just update activity time (already done above)
            }
            OshMessage::EXEC {
                device,
                req_id,
                cmd,
                cols,
                rows,
                pty,
            } => {
                info!(
                    "EXEC request: device={}, req_id={}, cmd={}",
                    device, req_id, cmd
                );

                // Find device
                let device_info = match self.device_store.find_device(&device).await {
                    Some(d) => d,
                    None => {
                        let msg = RelayToOshMessage::DEVICE_NOT_FOUND {
                            req_id,
                            message: format!("Device '{}' not found", device),
                        };
                        self.send_to_osh_raw(sender, &msg)?;
                        return Ok(());
                    }
                };

                // Check if device is approved
                if device_info.status != DeviceStatus::Approved {
                    let msg = RelayToOshMessage::DEVICE_NOT_APPROVED {
                        req_id,
                        message: format!("Device '{}' is not approved", device),
                    };
                    self.send_to_osh_raw(sender, &msg)?;
                    return Ok(());
                }

                // Check if device is online
                let clients = self.clients.read().await;
                let client = match clients.get(&device_info.device_id) {
                    Some(c) => c,
                    None => {
                        drop(clients);
                        let msg = RelayToOshMessage::DEVICE_OFFLINE {
                            req_id,
                            message: format!("Device '{}' is offline", device),
                        };
                        self.send_to_osh_raw(sender, &msg)?;
                        return Ok(());
                    }
                };

                // Check if client is approved
                if !client.approved {
                    drop(clients);
                    let msg = RelayToOshMessage::DEVICE_NOT_APPROVED {
                        req_id,
                        message: format!("Device '{}' is not approved", device),
                    };
                    self.send_to_osh_raw(sender, &msg)?;
                    return Ok(());
                }

                // Register session
                self.osh_sessions.write().await.insert(
                    req_id.clone(),
                    OshSession {
                        req_id: req_id.clone(),
                        device_id: device_info.device_id.clone(),
                        osh_addr: addr,
                        sender: sender.clone(),
                        last_activity: Instant::now(),
                    },
                );

                // Forward SPAWN to client
                let spawn_msg = RelayToClientMessage::SPAWN {
                    req_id,
                    payload: Some(base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        cmd.as_bytes(),
                    )),
                    cols,
                    rows,
                    pty,
                };
                self.send_to_client_raw(&client.sender, &spawn_msg)?;
            }
            OshMessage::INPUT { req_id, payload } => {
                // Find session and forward to client
                let sessions = self.osh_sessions.read().await;
                if let Some(session) = sessions.get(&req_id) {
                    let device_id = session.device_id.clone();
                    drop(sessions);

                    let clients = self.clients.read().await;
                    if let Some(client) = clients.get(&device_id) {
                        let msg = RelayToClientMessage::DATA { req_id, payload };
                        self.send_to_client_raw(&client.sender, &msg)?;
                    }
                } else {
                    let msg = RelayToOshMessage::INVALID_REQ_ID {
                        req_id,
                        message: "Unknown request ID".to_string(),
                    };
                    self.send_to_osh_raw(sender, &msg)?;
                }
            }
            OshMessage::RESIZE { req_id, cols, rows } => {
                // Find session and forward to client
                let sessions = self.osh_sessions.read().await;
                if let Some(session) = sessions.get(&req_id) {
                    let device_id = session.device_id.clone();
                    drop(sessions);

                    let clients = self.clients.read().await;
                    if let Some(client) = clients.get(&device_id) {
                        let msg = RelayToClientMessage::RESIZE { req_id, cols, rows };
                        self.send_to_client_raw(&client.sender, &msg)?;
                    }
                }
            }
            OshMessage::ADMIN_RELOAD => {
                info!("Admin reload request from {}", addr);
                match self.reload_config().await {
                    Ok(count) => {
                        let msg = RelayToOshMessage::ADMIN_RELOAD_OK {
                            message: format!("Reloaded {} devices", count),
                        };
                        self.send_to_osh_raw(sender, &msg)?;
                    }
                    Err(e) => {
                        let msg = RelayToOshMessage::ADMIN_RELOAD_ERROR {
                            message: format!("Failed to reload: {}", e),
                        };
                        self.send_to_osh_raw(sender, &msg)?;
                    }
                }
            }
        }

        Ok(())
    }

    /// Reload configuration and notify affected clients
    async fn reload_config(&self) -> Result<usize> {
        // Reload device store
        let count = self.device_store.reload().await?;

        // Update connected clients' approval status
        let mut clients = self.clients.write().await;
        for (device_id, client) in clients.iter_mut() {
            let was_approved = client.approved;
            let is_approved = self.device_store.is_device_approved(device_id).await;

            if !was_approved && is_approved {
                // Newly approved - send notification
                info!("Device {} is now approved", device_id);
                let msg = RelayToClientMessage::PAIR_APPROVED {
                    message: Some("Device approved".to_string()),
                };
                let _ = self.send_to_client_raw(&client.sender, &msg);
                client.approved = true;
            } else if was_approved && !is_approved {
                // Revoked approval
                info!("Device {} approval revoked", device_id);
                let msg = RelayToClientMessage::PAIR_REJECTED {
                    message: Some("Approval revoked".to_string()),
                };
                let _ = self.send_to_client_raw(&client.sender, &msg);
                client.approved = false;
            }
        }

        Ok(count)
    }

    /// Send a message to a client
    fn send_to_client_raw<T: serde::Serialize>(
        &self,
        sender: &mpsc::UnboundedSender<Message>,
        msg: &T,
    ) -> Result<()> {
        let json = serde_json::to_vec(msg)?;
        sender.send(Message::Binary(json.into()))?;
        Ok(())
    }

    /// Send a message to an OSH connection
    fn send_to_osh_raw<T: serde::Serialize>(
        &self,
        sender: &mpsc::UnboundedSender<Message>,
        msg: &T,
    ) -> Result<()> {
        let json = serde_json::to_vec(msg)?;
        sender.send(Message::Binary(json.into()))?;
        Ok(())
    }

    /// Get connected device count
    pub async fn get_connected_count(&self) -> usize {
        self.clients.read().await.len()
    }

    /// Get active session count
    pub async fn get_session_count(&self) -> usize {
        self.osh_sessions.read().await.len()
    }
}
