use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use parking_lot::RwLock;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;
use rustls::ServerConfig;
use rustls::pki_types::CertificateDer;
use thiserror::Error;

use crate::client::SharedClientRegistry;
use crate::handler::{handle_client, ShellEventSender};
use crate::logging::SharedLogStore;
use crate::settings::SharedSettings;
use crate::websocket::{detect_websocket_upgrade, accept_websocket, UpgradeResult};

#[derive(Error, Debug)]
pub enum ListenerError {
    #[error("Failed to bind to port {0}: {1}")]
    BindError(u16, String),
    #[error("TLS configuration error: {0}")]
    TlsError(String),
    #[error("Listener not found for port {0}")]
    NotFound(u16),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Clone)]
pub struct PortStatus {
    pub port: u16,
    pub enabled: bool,
    pub connections: u32,
}

struct ActiveListener {
    shutdown_tx: mpsc::Sender<()>,
    connections: Arc<RwLock<u32>>,
}

pub struct ListenerManager {
    listeners: Arc<RwLock<HashMap<u16, ActiveListener>>>,
    tls_config: Arc<RwLock<Option<Arc<ServerConfig>>>>,
    client_registry: SharedClientRegistry,
    shell_event_tx: Arc<RwLock<Option<ShellEventSender>>>,
    settings: SharedSettings,
    log_store: SharedLogStore,
}

impl ListenerManager {
    pub fn new(client_registry: SharedClientRegistry, settings: SharedSettings, log_store: SharedLogStore) -> Self {
        Self {
            listeners: Arc::new(RwLock::new(HashMap::new())),
            tls_config: Arc::new(RwLock::new(None)),
            client_registry,
            shell_event_tx: Arc::new(RwLock::new(None)),
            settings,
            log_store,
        }
    }

    /// Set the shell event sender for forwarding shell output to frontend
    pub fn set_shell_event_sender(&self, tx: ShellEventSender) {
        *self.shell_event_tx.write() = Some(tx);
    }

    /// Get a reference to the client registry
    pub fn client_registry(&self) -> &SharedClientRegistry {
        &self.client_registry
    }

    /// Configure TLS with certificate and key PEM data
    pub fn configure_tls(&self, cert_pem: &str, key_pem: &str) -> Result<(), ListenerError> {
        // Parse certificate
        let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_pem.as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| ListenerError::TlsError(e.to_string()))?;

        // Parse private key
        let key = rustls_pemfile::private_key(&mut key_pem.as_bytes())
            .map_err(|e| ListenerError::TlsError(e.to_string()))?
            .ok_or_else(|| ListenerError::TlsError("No private key found".to_string()))?;

        // Build server config
        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .map_err(|e| ListenerError::TlsError(e.to_string()))?;

        *self.tls_config.write() = Some(Arc::new(config));
        Ok(())
    }

    /// Start listening on a specific port
    pub async fn start_listener(&self, port: u16) -> Result<(), ListenerError> {
        // Check if already listening
        if self.listeners.read().contains_key(&port) {
            return Ok(());
        }

        // Get TLS config
        let tls_config = self.tls_config.read().clone()
            .ok_or_else(|| ListenerError::TlsError("TLS not configured".to_string()))?;

        let acceptor = TlsAcceptor::from(tls_config);

        // Bind to port
        let addr: SocketAddr = format!("0.0.0.0:{}", port).parse().unwrap();
        let tcp_listener = TcpListener::bind(addr).await
            .map_err(|e| ListenerError::BindError(port, e.to_string()))?;

        // Create shutdown channel
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);
        let connections = Arc::new(RwLock::new(0u32));
        let connections_clone = connections.clone();

        // Spawn listener task
        let listeners = self.listeners.clone();
        let registry = self.client_registry.clone();
        let shell_tx = self.shell_event_tx.clone();
        let settings = self.settings.clone();
        let log_store = self.log_store.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    accept_result = tcp_listener.accept() => {
                        match accept_result {
                            Ok((stream, peer_addr)) => {
                                let acceptor = acceptor.clone();
                                let conns = connections_clone.clone();
                                let registry = registry.clone();
                                let shell_event_tx = shell_tx.read().clone();
                                let settings = settings.clone();
                                let log_store = log_store.clone();

                                // Increment connection count
                                *conns.write() += 1;

                                tokio::spawn(async move {
                                    match acceptor.accept(stream).await {
                                        Ok(tls_stream) => {
                                            // Handle the TLS connection with protocol handler
                                            handle_client(tls_stream, peer_addr, port, registry, shell_event_tx, settings, log_store).await;
                                        }
                                        Err(e) => {
                                            eprintln!("TLS handshake failed from {}: {}", peer_addr, e);
                                        }
                                    }

                                    // Decrement connection count
                                    let mut count = conns.write();
                                    *count = count.saturating_sub(1);
                                });
                            }
                            Err(e) => {
                                eprintln!("Accept error on port {}: {}", port, e);
                            }
                        }
                    }
                    _ = shutdown_rx.recv() => {
                        println!("Shutting down listener on port {}", port);
                        break;
                    }
                }
            }

            // Remove from active listeners
            listeners.write().remove(&port);
        });

        // Store listener info
        self.listeners.write().insert(port, ActiveListener {
            shutdown_tx,
            connections,
        });

        println!("Started listener on port {}", port);
        Ok(())
    }

    /// Stop listening on a specific port and disconnect all clients on that port
    pub async fn stop_listener(&self, port: u16) -> Result<(), ListenerError> {
        let listener = self.listeners.write().remove(&port);

        if let Some(listener) = listener {
            // Disconnect all clients connected on this port
            let disconnected = self.client_registry.disconnect_clients_on_port(port);
            if disconnected > 0 {
                println!("Disconnected {} clients on port {}", disconnected, port);
            }

            let _ = listener.shutdown_tx.send(()).await;
            Ok(())
        } else {
            Err(ListenerError::NotFound(port))
        }
    }

    /// Get status of all configured ports
    pub fn get_port_statuses(&self, configured_ports: &[u16]) -> Vec<PortStatus> {
        let listeners = self.listeners.read();

        configured_ports.iter().map(|&port| {
            if let Some(listener) = listeners.get(&port) {
                PortStatus {
                    port,
                    enabled: true,
                    connections: *listener.connections.read(),
                }
            } else {
                PortStatus {
                    port,
                    enabled: false,
                    connections: 0,
                }
            }
        }).collect()
    }

    /// Check if a port is currently listening
    pub fn is_listening(&self, port: u16) -> bool {
        self.listeners.read().contains_key(&port)
    }

    /// Get connection count for a port
    pub fn get_connections(&self, port: u16) -> u32 {
        self.listeners.read()
            .get(&port)
            .map(|l| *l.connections.read())
            .unwrap_or(0)
    }
}

