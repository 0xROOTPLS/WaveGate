//! Startup manager operations for the client.
//!
//! Enumerates and manages startup entries from:
//! - Registry: HKCU\Software\Microsoft\Windows\CurrentVersion\Run
//! - Registry: HKLM\Software\Microsoft\Windows\CurrentVersion\Run
//! - Registry: HKCU\Software\Microsoft\Windows\CurrentVersion\RunOnce
//! - Registry: HKLM\Software\Microsoft\Windows\CurrentVersion\RunOnce
//! - Startup folder: %APPDATA%\Microsoft\Windows\Start Menu\Programs\Startup
//! - Common startup folder: %PROGRAMDATA%\Microsoft\Windows\Start Menu\Programs\Startup

use wavegate_shared::{CommandResponseData, StartupEntry};

/// List all startup entries
pub fn list_startup_entries() -> (bool, CommandResponseData) {
    let mut entries = Vec::new();

    // Registry locations
    entries.extend(get_registry_entries(
        "HKCU",
        r"Software\Microsoft\Windows\CurrentVersion\Run",
        "Registry (Current User)",
    ));
    entries.extend(get_registry_entries(
        "HKLM",
        r"Software\Microsoft\Windows\CurrentVersion\Run",
        "Registry (Local Machine)",
    ));
    entries.extend(get_registry_entries(
        "HKCU",
        r"Software\Microsoft\Windows\CurrentVersion\RunOnce",
        "Registry RunOnce (Current User)",
    ));
    entries.extend(get_registry_entries(
        "HKLM",
        r"Software\Microsoft\Windows\CurrentVersion\RunOnce",
        "Registry RunOnce (Local Machine)",
    ));

    // Startup folders
    entries.extend(get_startup_folder_entries(false));
    entries.extend(get_startup_folder_entries(true));

    // Sort by name
    entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    (true, CommandResponseData::StartupList { entries })
}

/// Get entries from a registry key
fn get_registry_entries(root: &str, path: &str, location: &str) -> Vec<StartupEntry> {
    use winreg::enums::*;
    use winreg::RegKey;

    let mut entries = Vec::new();

    let hkey = match root {
        "HKCU" => RegKey::predef(HKEY_CURRENT_USER),
        "HKLM" => RegKey::predef(HKEY_LOCAL_MACHINE),
        _ => return entries,
    };

    if let Ok(key) = hkey.open_subkey(path) {
        for value_name in key.enum_values().filter_map(|v| v.ok()) {
            let (name, reg_value) = value_name;

            // Get the command/path from the registry value
            let command = match reg_value {
                winreg::RegValue { bytes, .. } => {
                    // Try to interpret as string (REG_SZ or REG_EXPAND_SZ)
                    String::from_utf16_lossy(
                        &bytes.chunks_exact(2)
                            .map(|c| u16::from_le_bytes([c[0], c[1]]))
                            .take_while(|&c| c != 0)
                            .collect::<Vec<u16>>()
                    )
                }
            };

            entries.push(StartupEntry {
                name: name.clone(),
                command,
                location: location.to_string(),
                entry_type: "Registry".to_string(),
                enabled: true, // Registry entries are always enabled if present
                registry_key: Some(format!("{}\\{}", root, path)),
                registry_value: Some(name),
                file_path: None,
            });
        }
    }

    entries
}

/// Get entries from startup folder
fn get_startup_folder_entries(common: bool) -> Vec<StartupEntry> {
    use std::fs;
    use std::path::PathBuf;

    let mut entries = Vec::new();

    let folder_path = if common {
        // Common startup folder: C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Startup
        std::env::var("PROGRAMDATA")
            .ok()
            .map(|pd| PathBuf::from(pd).join(r"Microsoft\Windows\Start Menu\Programs\Startup"))
    } else {
        // User startup folder: %APPDATA%\Microsoft\Windows\Start Menu\Programs\Startup
        std::env::var("APPDATA")
            .ok()
            .map(|appdata| PathBuf::from(appdata).join(r"Microsoft\Windows\Start Menu\Programs\Startup"))
    };

    let location = if common {
        "Startup Folder (All Users)"
    } else {
        "Startup Folder (Current User)"
    };

    if let Some(folder) = folder_path {
        if let Ok(read_dir) = fs::read_dir(&folder) {
            for entry in read_dir.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();

                // Get the target for shortcuts, or the path itself for other files
                let command = if path.extension().map(|e| e == "lnk").unwrap_or(false) {
                    // For .lnk files, try to resolve the target
                    resolve_shortcut(&path).unwrap_or_else(|| path.to_string_lossy().to_string())
                } else {
                    path.to_string_lossy().to_string()
                };

                entries.push(StartupEntry {
                    name,
                    command,
                    location: location.to_string(),
                    entry_type: "Folder".to_string(),
                    enabled: true,
                    registry_key: None,
                    registry_value: None,
                    file_path: Some(path.to_string_lossy().to_string()),
                });
            }
        }
    }

    entries
}

/// Resolve a Windows shortcut (.lnk) to get its target path
/// Uses COM IShellLink interface
fn resolve_shortcut(path: &std::path::Path) -> Option<String> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows::core::{Interface, PCWSTR};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize,
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, STGM, IPersistFile,
    };
    use windows::Win32::UI::Shell::IShellLinkW;

    // CLSID for ShellLink
    const CLSID_SHELL_LINK: windows::core::GUID = windows::core::GUID::from_u128(
        0x00021401_0000_0000_C000_000000000046
    );

    unsafe {
        // Initialize COM
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        let shell_link: IShellLinkW = match CoCreateInstance(&CLSID_SHELL_LINK, None, CLSCTX_INPROC_SERVER) {
            Ok(sl) => sl,
            Err(_) => {
                CoUninitialize();
                return None;
            }
        };

        let persist_file: IPersistFile = match shell_link.cast() {
            Ok(pf) => pf,
            Err(_) => {
                CoUninitialize();
                return None;
            }
        };

        // Convert path to wide string
        let path_wide: Vec<u16> = OsStr::new(path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        // Load the shortcut (STGM(0) = STGM_READ)
        if persist_file.Load(PCWSTR(path_wide.as_ptr()), STGM(0)).is_err() {
            CoUninitialize();
            return None;
        }

        // Get the target path
        let mut target_buf = [0u16; 260];
        if shell_link.GetPath(&mut target_buf, std::ptr::null_mut(), 0).is_err() {
            CoUninitialize();
            return None;
        }

        let target_len = target_buf.iter().position(|&c| c == 0).unwrap_or(target_buf.len());
        let target = String::from_utf16_lossy(&target_buf[..target_len]);

        CoUninitialize();

        if target.is_empty() {
            None
        } else {
            Some(target)
        }
    }
}

/// Remove a startup entry
pub fn remove_startup_entry(
    entry_type: &str,
    registry_key: Option<&str>,
    registry_value: Option<&str>,
    file_path: Option<&str>,
) -> (bool, CommandResponseData) {
    match entry_type {
        "Registry" => {
            if let (Some(key_path), Some(value_name)) = (registry_key, registry_value) {
                remove_registry_entry(key_path, value_name)
            } else {
                (false, CommandResponseData::Error {
                    message: "Missing registry key or value name".to_string(),
                })
            }
        }
        "Folder" => {
            if let Some(path) = file_path {
                remove_folder_entry(path)
            } else {
                (false, CommandResponseData::Error {
                    message: "Missing file path".to_string(),
                })
            }
        }
        _ => (false, CommandResponseData::Error {
            message: format!("Unknown entry type: {}", entry_type),
        }),
    }
}

/// Remove a registry startup entry
fn remove_registry_entry(key_path: &str, value_name: &str) -> (bool, CommandResponseData) {
    use winreg::enums::*;
    use winreg::RegKey;

    // Parse key path like "HKCU\Software\Microsoft\Windows\CurrentVersion\Run"
    let parts: Vec<&str> = key_path.splitn(2, '\\').collect();
    if parts.len() != 2 {
        return (false, CommandResponseData::Error {
            message: format!("Invalid registry key path: {}", key_path),
        });
    }

    let (root, subkey) = (parts[0], parts[1]);

    let hkey = match root {
        "HKCU" => RegKey::predef(HKEY_CURRENT_USER),
        "HKLM" => RegKey::predef(HKEY_LOCAL_MACHINE),
        _ => {
            return (false, CommandResponseData::Error {
                message: format!("Unknown registry root: {}", root),
            });
        }
    };

    match hkey.open_subkey_with_flags(subkey, KEY_WRITE) {
        Ok(key) => {
            match key.delete_value(value_name) {
                Ok(_) => (true, CommandResponseData::StartupResult {
                    success: true,
                    message: format!("Removed '{}' from {}", value_name, key_path),
                }),
                Err(e) => (false, CommandResponseData::StartupResult {
                    success: false,
                    message: format!("Failed to delete registry value: {}", e),
                }),
            }
        }
        Err(e) => (false, CommandResponseData::StartupResult {
            success: false,
            message: format!("Failed to open registry key: {}", e),
        }),
    }
}

/// Remove a startup folder entry
fn remove_folder_entry(file_path: &str) -> (bool, CommandResponseData) {
    use std::fs;
    use std::path::Path;

    let path = Path::new(file_path);

    if !path.exists() {
        return (false, CommandResponseData::StartupResult {
            success: false,
            message: format!("File not found: {}", file_path),
        });
    }

    match fs::remove_file(path) {
        Ok(_) => (true, CommandResponseData::StartupResult {
            success: true,
            message: format!("Removed '{}'", file_path),
        }),
        Err(e) => (false, CommandResponseData::StartupResult {
            success: false,
            message: format!("Failed to delete file: {}", e),
        }),
    }
}
