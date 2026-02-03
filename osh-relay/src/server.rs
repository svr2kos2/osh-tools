//! WebSocket server implementation
//!
//! Provides two WebSocket endpoints:
//! - 0.0.0.0:8080 for Client (Local Daemon) connections
//! - 127.0.0.1:8081 for osh CLI and osh-admin connections

use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::{accept_async, tungstenite::Message};
use tracing::{error, info, warn};

use crate::relay::RelayService;

/// Start the Client WebSocket server (0.0.0.0:8080)
pub async fn start_client_server(relay: Arc<RelayService>, port: u16) -> Result<()> {
    let addr = format!("0.0.0.0:{}", port);
    let listener = TcpListener::bind(&addr).await?;
    info!("Client WebSocket server listening on {}", addr);

    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                let relay = relay.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_client_connection(stream, addr, relay).await {
                        error!("Client connection error from {}: {}", addr, e);
                    }
                });
            }
            Err(e) => {
                error!("Failed to accept client connection: {}", e);
            }
        }
    }
}

/// Start the OSH/Admin WebSocket server (127.0.0.1:8081)
pub async fn start_osh_server(relay: Arc<RelayService>, port: u16) -> Result<()> {
    let addr = format!("127.0.0.1:{}", port);
    let listener = TcpListener::bind(&addr).await?;
    info!("OSH/Admin WebSocket server listening on {}", addr);

    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                let relay = relay.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_osh_connection(stream, addr, relay).await {
                        error!("OSH connection error from {}: {}", addr, e);
                    }
                });
            }
            Err(e) => {
                error!("Failed to accept OSH connection: {}", e);
            }
        }
    }
}

/// Handle a Client (Local Daemon) connection
async fn handle_client_connection(
    stream: TcpStream,
    addr: SocketAddr,
    relay: Arc<RelayService>,
) -> Result<()> {
    info!("New client connection from {}", addr);

    let ws_stream = accept_async(stream).await?;
    let (ws_sender, ws_receiver) = ws_stream.split();

    // Create channels for sending messages to this connection
    let (tx, rx) = mpsc::unbounded_channel::<Message>();

    // Spawn writer task
    let write_task = tokio::spawn(write_messages(rx, ws_sender));

    // Handle the connection in relay service
    relay.handle_client_connection(addr, tx, ws_receiver).await;

    // Clean up
    write_task.abort();

    info!("Client connection closed: {}", addr);
    Ok(())
}

/// Handle an OSH CLI or osh-admin connection
async fn handle_osh_connection(
    stream: TcpStream,
    addr: SocketAddr,
    relay: Arc<RelayService>,
) -> Result<()> {
    info!("New OSH connection from {}", addr);

    let ws_stream = accept_async(stream).await?;
    let (ws_sender, ws_receiver) = ws_stream.split();

    // Create channels for sending messages to this connection
    let (tx, rx) = mpsc::unbounded_channel::<Message>();

    // Spawn writer task
    let write_task = tokio::spawn(write_messages(rx, ws_sender));

    // Handle the connection in relay service
    relay.handle_osh_connection(addr, tx, ws_receiver).await;

    // Clean up
    write_task.abort();

    info!("OSH connection closed: {}", addr);
    Ok(())
}

/// Write messages from channel to WebSocket
async fn write_messages(
    mut rx: mpsc::UnboundedReceiver<Message>,
    mut ws_sender: futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<TcpStream>,
        Message,
    >,
) {
    while let Some(msg) = rx.recv().await {
        if let Err(e) = ws_sender.send(msg).await {
            warn!("Failed to send WebSocket message: {}", e);
            break;
        }
    }
}
