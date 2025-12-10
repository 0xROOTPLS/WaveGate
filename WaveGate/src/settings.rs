//! Settings persistence.
//!
//! Saves and loads application settings to/from disk.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::crypto::get_data_dir;

const SETTINGS_FILE: &str = "settings.json";

/// Application settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    /// Configured listener ports
    #[serde(default = "default_ports")]
    pub ports: Vec<u16>,

    /// Minimize to tray when minimizing
    #[serde(default)]
    pub minimize_to_tray: bool,

    /// Close to tray instead of exiting
    #[serde(default)]
    pub close_to_tray: bool,

    /// Open output folder after build
    #[serde(default = "default_true")]
    pub open_output_folder: bool,

    /// Restore window state on load
    #[serde(default = "default_true")]
    pub restore_window_state: bool,

    /// Autorun type: "cmd" or "powershell"
    #[serde(default = "default_autorun_type")]
    pub autorun_type: String,

    /// Autorun commands/script
    #[serde(default)]
    pub autorun_commands: String,

    /// Play sound on new client
    #[serde(default = "default_true")]
    pub sound_new_client: bool,

    /// Play sound on lost client
    #[serde(default)]
    pub sound_lost_client: bool,

    /// Log connection events
    #[serde(default = "default_true")]
    pub log_connection_events: bool,

    /// Show client connection notifications
    #[serde(default = "default_true")]
    pub notify_connect: bool,

    /// Show client disconnection notifications
    #[serde(default = "default_true")]
    pub notify_disconnect: bool,

    // Advanced settings
    /// Log internal server exceptions
    #[serde(default)]
    pub log_exceptions: bool,

    /// Filter clients with duplicate UID
    #[serde(default = "default_true")]
    pub filter_dup_uid: bool,

    /// Filter clients with duplicate IP
    #[serde(default)]
    pub filter_dup_ip: bool,

    /// Filter clients with duplicate LAN IP
    #[serde(default)]
    pub filter_dup_lan: bool,

    /// Timeout interval in milliseconds
    #[serde(default = "default_timeout_interval")]
    pub timeout_interval: u32,

    /// Keepalive timeout in milliseconds
    #[serde(default = "default_keepalive_timeout")]
    pub keepalive_timeout: u32,

    /// Identify timeout in milliseconds
    #[serde(default = "default_identify_timeout")]
    pub identify_timeout: u32,

    /// Pipe timeout in milliseconds
    #[serde(default = "default_pipe_timeout")]
    pub pipe_timeout: u32,

    /// Maximum number of clients
    #[serde(default = "default_max_clients")]
    pub max_clients: u32,

    /// Maximum number of connections
    #[serde(default = "default_max_connections")]
    pub max_connections: u32,

    /// Buffer size in bytes
    #[serde(default = "default_buffer_size")]
    pub buffer_size: u32,

    /// Maximum packet size in bytes
    #[serde(default = "default_max_packet_size")]
    pub max_packet_size: u32,

    /// GC threshold in MB
    #[serde(default = "default_gc_threshold")]
    pub gc_threshold: u32,
}

// Default value functions
fn default_ports() -> Vec<u16> {
    vec![4444, 8080]
}

fn default_true() -> bool {
    true
}

fn default_autorun_type() -> String {
    "cmd".to_string()
}

fn default_timeout_interval() -> u32 {
    30000
}

fn default_keepalive_timeout() -> u32 {
    60000
}

fn default_identify_timeout() -> u32 {
    10000
}

fn default_pipe_timeout() -> u32 {
    5000
}

fn default_max_clients() -> u32 {
    1000
}

fn default_max_connections() -> u32 {
    5000
}

fn default_buffer_size() -> u32 {
    65536
}

fn default_max_packet_size() -> u32 {
    10485760
}

fn default_gc_threshold() -> u32 {
    100
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            ports: default_ports(),
            minimize_to_tray: false,
            close_to_tray: false,
            open_output_folder: true,
            restore_window_state: true,
            autorun_type: default_autorun_type(),
            autorun_commands: String::new(),
            sound_new_client: true,
            sound_lost_client: false,
            log_connection_events: true,
            notify_connect: true,
            notify_disconnect: true,
            log_exceptions: false,
            filter_dup_uid: true,
            filter_dup_ip: false,
            filter_dup_lan: false,
            timeout_interval: default_timeout_interval(),
            keepalive_timeout: default_keepalive_timeout(),
            identify_timeout: default_identify_timeout(),
            pipe_timeout: default_pipe_timeout(),
            max_clients: default_max_clients(),
            max_connections: default_max_connections(),
            buffer_size: default_buffer_size(),
            max_packet_size: default_max_packet_size(),
            gc_threshold: default_gc_threshold(),
        }
    }
}

/// Result of settings load
pub struct SettingsLoadResult {
    pub settings: AppSettings,
    pub was_loaded: bool,
    pub path: PathBuf,
}

impl AppSettings {
    /// Get the settings file path
    fn get_path() -> Result<PathBuf, SettingsError> {
        let data_dir = get_data_dir().map_err(|e| SettingsError::Io(e.to_string()))?;
        Ok(data_dir.join(SETTINGS_FILE))
    }

    /// Load settings from disk, or return defaults if not found
    /// Returns the settings and whether they were loaded from disk
    pub fn load() -> SettingsLoadResult {
        let path = Self::get_path().unwrap_or_else(|_| PathBuf::from("."));

        match Self::try_load() {
            Ok(settings) => SettingsLoadResult {
                settings,
                was_loaded: true,
                path,
            },
            Err(_) => SettingsLoadResult {
                settings: Self::default(),
                was_loaded: false,
                path,
            },
        }
    }

    /// Try to load settings from disk
    fn try_load() -> Result<Self, SettingsError> {
        let path = Self::get_path()?;

        if !path.exists() {
            return Err(SettingsError::NotFound);
        }

        let contents = fs::read_to_string(&path)
            .map_err(|e| SettingsError::Io(e.to_string()))?;

        let settings: Self = serde_json::from_str(&contents)
            .map_err(|e| SettingsError::Parse(e.to_string()))?;

        Ok(settings)
    }

    /// Save settings to disk
    pub fn save(&self) -> Result<(), SettingsError> {
        let path = Self::get_path()?;

        let contents = serde_json::to_string_pretty(self)
            .map_err(|e| SettingsError::Parse(e.to_string()))?;

        fs::write(&path, contents)
            .map_err(|e| SettingsError::Io(e.to_string()))?;

        println!("Settings saved to {:?}", path);
        Ok(())
    }
}

/// Settings errors
#[derive(Debug)]
pub enum SettingsError {
    NotFound,
    Io(String),
    Parse(String),
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "Settings file not found"),
            Self::Io(e) => write!(f, "IO error: {}", e),
            Self::Parse(e) => write!(f, "Parse error: {}", e),
        }
    }
}

impl std::error::Error for SettingsError {}

/// Shared settings reference type
pub type SharedSettings = Arc<RwLock<AppSettings>>;

use std::sync::Arc;
use parking_lot::RwLock;
