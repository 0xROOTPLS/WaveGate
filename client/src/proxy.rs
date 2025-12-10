//! Reverse proxy client-side handler.
//!
//! Manages connections requested by the server and relays data.
//! Supports TCP connections, local named pipes, and remote SMB pipes.

use std::collections::HashMap;
use std::sync::Arc;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use parking_lot::RwLock;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use wavegate_shared::{
    ClientMessageType, ProxyConnectMessage, ProxyConnectResultMessage,
    ProxyDataMessage, ProxyCloseMessage, ProxyClosedMessage, ProxyTarget,
};

use windows::core::{PCWSTR, HSTRING};
use windows::Win32::Foundation::{HANDLE, CloseHandle, GetLastError};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FlushFileBuffers,
    FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_NONE,
    OPEN_EXISTING, FILE_FLAG_OVERLAPPED, FILE_ATTRIBUTE_NORMAL,
};
use windows::Win32::Security::{
    LogonUserW, ImpersonateLoggedOnUser, RevertToSelf,
    LOGON32_LOGON_NEW_CREDENTIALS, LOGON32_PROVIDER_WINNT50,
};

/// Active proxy connection
struct ProxyConnection {
    /// Channel to send data to this connection's write task
    data_tx: mpsc::Sender<Vec<u8>>,
    /// Handle to the connection task (for cleanup)
    _task_handle: tokio::task::JoinHandle<()>,
}

/// Inner proxy manager state
struct ProxyManagerInner {
    /// Active connections keyed by conn_id
    connections: RwLock<HashMap<u32, ProxyConnection>>,
    /// Channel to send messages back to the server
    server_tx: mpsc::Sender<(ClientMessageType, Vec<u8>)>,
}

/// Proxy manager for handling server-requested connections
#[derive(Clone)]
pub struct ProxyManager {
    inner: Arc<ProxyManagerInner>,
}

impl ProxyManager {
    pub fn new(server_tx: mpsc::Sender<(ClientMessageType, Vec<u8>)>) -> Self {
        Self {
            inner: Arc::new(ProxyManagerInner {
                connections: RwLock::new(HashMap::new()),
                server_tx,
            }),
        }
    }

    /// Handle a connect request from the server (unified handler)
    pub async fn handle_connect(&self, msg: ProxyConnectMessage) {
        let conn_id = msg.conn_id;

        match &msg.target {
            ProxyTarget::Tcp { host, port } => {
                self.handle_tcp_connect(conn_id, host.clone(), *port).await;
            }
            ProxyTarget::LocalPipe { pipe_name } => {
                self.handle_local_pipe_connect(conn_id, pipe_name.clone()).await;
            }
            ProxyTarget::RemotePipe { server, pipe_name, username, password, domain } => {
                self.handle_remote_pipe_connect(
                    conn_id,
                    server.clone(),
                    pipe_name.clone(),
                    username.clone(),
                    password.clone(),
                    domain.clone(),
                ).await;
            }
        }
    }

    /// Handle TCP connection (existing SOCKS5 behavior)
    async fn handle_tcp_connect(&self, conn_id: u32, host: String, port: u16) {
        let addr = format!("{}:{}", host, port);
        let connect_result = TcpStream::connect(&addr).await;

        match connect_result {
            Ok(stream) => {
                let local_addr = stream.local_addr().ok();
                let bound_addr = local_addr.map(|a| a.ip().to_string());
                let bound_port = local_addr.map(|a| a.port());

                // Send success response
                let response = ProxyConnectResultMessage {
                    conn_id,
                    success: true,
                    error: None,
                    bound_addr,
                    bound_port,
                };
                let payload = serde_json::to_vec(&response).unwrap();
                let _ = self.inner.server_tx.send((ClientMessageType::ProxyConnectResult, payload)).await;

                // Split stream and start relay
                let (reader, writer) = stream.into_split();
                self.start_tcp_relay_tasks(conn_id, reader, writer).await;
            }
            Err(e) => {
                self.send_connect_error(conn_id, e.to_string()).await;
            }
        }
    }

    /// Handle local named pipe connection (\\.\pipe\name)
    async fn handle_local_pipe_connect(&self, conn_id: u32, pipe_name: String) {
        let pipe_path = format!(r"\\.\pipe\{}", pipe_name);

        // Open pipe synchronously (blocking, but quick)
        let result = tokio::task::spawn_blocking(move || {
            open_named_pipe_sync(&pipe_path)
        }).await;

        match result {
            Ok(Ok(handle)) => {
                // Send success response
                let response = ProxyConnectResultMessage {
                    conn_id,
                    success: true,
                    error: None,
                    bound_addr: Some(format!(r"\\.\pipe\{}", pipe_name)),
                    bound_port: None,
                };
                let payload = serde_json::to_vec(&response).unwrap();
                let _ = self.inner.server_tx.send((ClientMessageType::ProxyConnectResult, payload)).await;

                // Start relay using the pipe
                self.start_pipe_relay_tasks(conn_id, handle).await;
            }
            Ok(Err(e)) => {
                self.send_connect_error(conn_id, e).await;
            }
            Err(e) => {
                self.send_connect_error(conn_id, format!("Task error: {}", e)).await;
            }
        }
    }

    /// Handle remote named pipe connection via SMB (\\server\pipe\name)
    async fn handle_remote_pipe_connect(
        &self,
        conn_id: u32,
        server: String,
        pipe_name: String,
        username: Option<String>,
        password: Option<String>,
        domain: Option<String>,
    ) {
        let pipe_path = format!(r"\\{}\pipe\{}", server, pipe_name);
        let pipe_path_clone = pipe_path.clone();

        // Do all the work in spawn_blocking to avoid Send issues with HANDLE
        let result = tokio::task::spawn_blocking(move || {
            // If credentials provided, impersonate before connecting
            let mut token_handle: Option<HANDLE> = None;

            if let (Some(user), Some(pass)) = (&username, &password) {
                match logon_and_impersonate_sync(user, pass, domain.as_deref()) {
                    Ok(handle) => {
                        token_handle = Some(handle);
                    }
                    Err(e) => {
                        return Err(format!("Logon failed: {}", e));
                    }
                }
            }

            // Connect to the remote pipe
            let pipe_result = open_named_pipe_sync(&pipe_path_clone);

            // Revert impersonation regardless of result
            if token_handle.is_some() {
                unsafe { let _ = RevertToSelf(); }
            }

            // Clean up logon handle
            if let Some(handle) = token_handle {
                unsafe { let _ = CloseHandle(handle); }
            }

            pipe_result
        }).await;

        match result {
            Ok(Ok(handle)) => {
                let response = ProxyConnectResultMessage {
                    conn_id,
                    success: true,
                    error: None,
                    bound_addr: Some(pipe_path),
                    bound_port: None,
                };
                let payload = serde_json::to_vec(&response).unwrap();
                let _ = self.inner.server_tx.send((ClientMessageType::ProxyConnectResult, payload)).await;

                self.start_pipe_relay_tasks(conn_id, handle).await;
            }
            Ok(Err(e)) => {
                self.send_connect_error(conn_id, e).await;
            }
            Err(e) => {
                self.send_connect_error(conn_id, format!("Task error: {}", e)).await;
            }
        }
    }

    /// Start relay tasks for TCP stream
    async fn start_tcp_relay_tasks<R, W>(&self, conn_id: u32, mut reader: R, mut writer: W)
    where
        R: AsyncReadExt + Unpin + Send + 'static,
        W: AsyncWriteExt + Unpin + Send + 'static,
    {
        let (data_tx, mut data_rx) = mpsc::channel::<Vec<u8>>(64);

        // Write task
        let write_task = tokio::spawn(async move {
            while let Some(data) = data_rx.recv().await {
                if writer.write_all(&data).await.is_err() {
                    break;
                }
            }
        });

        // Read task
        let server_tx = self.inner.server_tx.clone();
        let inner = self.inner.clone();
        let read_task = tokio::spawn(async move {
            let mut buf = vec![0u8; 8192];
            loop {
                match reader.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        let data_msg = ProxyDataMessage {
                            conn_id,
                            data: BASE64.encode(&buf[..n]),
                        };
                        let payload = serde_json::to_vec(&data_msg).unwrap();
                        if server_tx.send((ClientMessageType::ProxyData, payload)).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }

            let closed_msg = ProxyClosedMessage {
                conn_id,
                reason: None,
            };
            let payload = serde_json::to_vec(&closed_msg).unwrap();
            let _ = server_tx.send((ClientMessageType::ProxyClosed, payload)).await;
            inner.connections.write().remove(&conn_id);
        });

        let combined_task = tokio::spawn(async move {
            tokio::select! {
                _ = write_task => {}
                _ = read_task => {}
            }
        });

        self.inner.connections.write().insert(conn_id, ProxyConnection {
            data_tx,
            _task_handle: combined_task,
        });
    }

    /// Start relay tasks for named pipe using blocking I/O in separate threads
    async fn start_pipe_relay_tasks(&self, conn_id: u32, handle: PipeHandle) {
        let (data_tx, mut data_rx) = mpsc::channel::<Vec<u8>>(64);

        // PipeHandle is Copy + Send, so we can share it between tasks
        let write_handle = handle;
        let read_handle = handle;

        // Write task - runs in blocking thread
        let write_task = tokio::spawn(async move {
            while let Some(data) = data_rx.recv().await {
                let h = write_handle;
                let result = tokio::task::spawn_blocking(move || {
                    pipe_write_sync(h, &data)
                }).await;

                if result.is_err() || result.unwrap().is_err() {
                    break;
                }
            }
        });

        // Read task - runs in blocking thread
        let server_tx = self.inner.server_tx.clone();
        let inner = self.inner.clone();
        let read_task = tokio::spawn(async move {
            loop {
                let h = read_handle;
                let result = tokio::task::spawn_blocking(move || {
                    pipe_read_sync(h)
                }).await;

                match result {
                    Ok(Ok(data)) if !data.is_empty() => {
                        let data_msg = ProxyDataMessage {
                            conn_id,
                            data: BASE64.encode(&data),
                        };
                        let payload = serde_json::to_vec(&data_msg).unwrap();
                        if server_tx.send((ClientMessageType::ProxyData, payload)).await.is_err() {
                            break;
                        }
                    }
                    Ok(Ok(_)) => break, // Empty read = EOF
                    Ok(Err(_)) => break,
                    Err(_) => break,
                }
            }

            let closed_msg = ProxyClosedMessage {
                conn_id,
                reason: None,
            };
            let payload = serde_json::to_vec(&closed_msg).unwrap();
            let _ = server_tx.send((ClientMessageType::ProxyClosed, payload)).await;
            inner.connections.write().remove(&conn_id);

            // Close the handle
            unsafe { let _ = CloseHandle(HANDLE(read_handle.0)); }
        });

        let combined_task = tokio::spawn(async move {
            tokio::select! {
                _ = write_task => {}
                _ = read_task => {}
            }
        });

        self.inner.connections.write().insert(conn_id, ProxyConnection {
            data_tx,
            _task_handle: combined_task,
        });
    }

    /// Send connection error response
    async fn send_connect_error(&self, conn_id: u32, error: String) {
        let response = ProxyConnectResultMessage {
            conn_id,
            success: false,
            error: Some(error),
            bound_addr: None,
            bound_port: None,
        };
        let payload = serde_json::to_vec(&response).unwrap();
        let _ = self.inner.server_tx.send((ClientMessageType::ProxyConnectResult, payload)).await;
    }

    /// Handle data from server to send to target
    pub async fn handle_data(&self, msg: ProxyDataMessage) {
        let data_tx = {
            let conns = self.inner.connections.read();
            conns.get(&msg.conn_id).map(|c| c.data_tx.clone())
        };

        if let Some(tx) = data_tx {
            if let Ok(data) = BASE64.decode(&msg.data) {
                let _ = tx.send(data).await;
            }
        }
    }

    /// Handle close request from server
    pub fn handle_close(&self, msg: ProxyCloseMessage) {
        self.inner.connections.write().remove(&msg.conn_id);
    }

    /// Get number of active connections
    pub fn active_connections(&self) -> usize {
        self.inner.connections.read().len()
    }
}

// =============================================================================
// Synchronous Named Pipe Operations (run in spawn_blocking)
// =============================================================================

/// Wrapper for raw handle pointer (Send-safe for spawn_blocking)
#[derive(Clone, Copy)]
struct PipeHandle(*mut std::ffi::c_void);
unsafe impl Send for PipeHandle {}
unsafe impl Sync for PipeHandle {}

/// Open a named pipe synchronously
fn open_named_pipe_sync(pipe_path: &str) -> Result<PipeHandle, String> {
    // Try to connect with retries (pipe might be busy)
    let max_retries = 3;
    let mut last_error = String::new();

    for attempt in 0..max_retries {
        unsafe {
            let path_w = HSTRING::from(pipe_path);

            let handle = CreateFileW(
                PCWSTR(path_w.as_ptr()),
                (FILE_GENERIC_READ | FILE_GENERIC_WRITE).0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OVERLAPPED,
                None,
            );

            match handle {
                Ok(h) if !h.is_invalid() => {
                    return Ok(PipeHandle(h.0));
                }
                Ok(_) => {
                    let err = GetLastError();
                    last_error = format!("Invalid handle: {:?}", err);
                }
                Err(e) => {
                    last_error = format!("CreateFileW failed: {}", e);
                }
            }
        }

        if attempt < max_retries - 1 {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    Err(last_error)
}

/// Perform LogonUser and ImpersonateLoggedOnUser synchronously
fn logon_and_impersonate_sync(
    username: &str,
    password: &str,
    domain: Option<&str>,
) -> Result<HANDLE, String> {
    unsafe {
        let mut token_handle = HANDLE::default();

        let username_w = HSTRING::from(username);
        let password_w = HSTRING::from(password);
        let domain_w = domain.map(HSTRING::from);

        let domain_ptr = match &domain_w {
            Some(d) => PCWSTR(d.as_ptr()),
            None => PCWSTR::null(),
        };

        let result = LogonUserW(
            PCWSTR(username_w.as_ptr()),
            domain_ptr,
            PCWSTR(password_w.as_ptr()),
            LOGON32_LOGON_NEW_CREDENTIALS,
            LOGON32_PROVIDER_WINNT50,
            &mut token_handle,
        );

        if result.is_err() {
            let err = GetLastError();
            return Err(format!("LogonUserW failed: {:?}", err));
        }

        let result = ImpersonateLoggedOnUser(token_handle);
        if result.is_err() {
            CloseHandle(token_handle).ok();
            let err = GetLastError();
            return Err(format!("ImpersonateLoggedOnUser failed: {:?}", err));
        }

        Ok(token_handle)
    }
}

/// Read from pipe synchronously
fn pipe_read_sync(handle: PipeHandle) -> Result<Vec<u8>, String> {
    unsafe {
        let mut buf = vec![0u8; 8192];
        let mut bytes_read = 0u32;

        let result = ReadFile(
            HANDLE(handle.0),
            Some(&mut buf),
            Some(&mut bytes_read),
            None,
        );

        if result.is_ok() {
            buf.truncate(bytes_read as usize);
            Ok(buf)
        } else {
            let err = GetLastError();
            if err.0 == 109 { // ERROR_BROKEN_PIPE
                Ok(vec![]) // EOF
            } else {
                Err(format!("ReadFile failed: {:?}", err))
            }
        }
    }
}

/// Write to pipe synchronously
fn pipe_write_sync(handle: PipeHandle, data: &[u8]) -> Result<(), String> {
    unsafe {
        let mut bytes_written = 0u32;

        let result = WriteFile(
            HANDLE(handle.0),
            Some(data),
            Some(&mut bytes_written),
            None,
        );

        if result.is_ok() {
            // Flush
            let _ = FlushFileBuffers(HANDLE(handle.0));
            Ok(())
        } else {
            let err = GetLastError();
            Err(format!("WriteFile failed: {:?}", err))
        }
    }
}
