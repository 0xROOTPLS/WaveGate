//! Registry Manager module for Windows registry operations.

use wavegate_shared::{CommandResponseData, RegistryKeyInfo, RegistryValueInfo};
use winreg::enums::*;
use winreg::RegKey;

/// Parse a registry path like "HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft"
/// Returns (root_key, subpath)
fn parse_registry_path(path: &str) -> Result<(RegKey, String), String> {
    let parts: Vec<&str> = path.splitn(2, '\\').collect();
    let root_name = parts[0].to_uppercase();
    let subpath = if parts.len() > 1 { parts[1].to_string() } else { String::new() };

    let root = match root_name.as_str() {
        "HKEY_LOCAL_MACHINE" | "HKLM" => RegKey::predef(HKEY_LOCAL_MACHINE),
        "HKEY_CURRENT_USER" | "HKCU" => RegKey::predef(HKEY_CURRENT_USER),
        "HKEY_CLASSES_ROOT" | "HKCR" => RegKey::predef(HKEY_CLASSES_ROOT),
        "HKEY_USERS" | "HKU" => RegKey::predef(HKEY_USERS),
        "HKEY_CURRENT_CONFIG" | "HKCC" => RegKey::predef(HKEY_CURRENT_CONFIG),
        _ => return Err(format!("Unknown registry root: {}", root_name)),
    };

    Ok((root, subpath))
}

/// List subkeys under a registry path
pub fn list_keys(path: &str) -> (bool, CommandResponseData) {
    let (root, subpath) = match parse_registry_path(path) {
        Ok(r) => r,
        Err(e) => return (false, CommandResponseData::Error { message: e }),
    };

    let key = if subpath.is_empty() {
        root
    } else {
        match root.open_subkey(&subpath) {
            Ok(k) => k,
            Err(e) => return (false, CommandResponseData::Error {
                message: format!("Failed to open key: {}", e)
            }),
        }
    };

    let mut keys = Vec::new();
    for name in key.enum_keys().filter_map(|r| r.ok()) {
        // Try to get subkey info
        let (subkey_count, value_count) = if let Ok(subkey) = key.open_subkey(&name) {
            let sk = subkey.enum_keys().count() as u32;
            let vk = subkey.enum_values().count() as u32;
            (sk, vk)
        } else {
            (0, 0)
        };

        keys.push(RegistryKeyInfo {
            name,
            subkey_count,
            value_count,
        });
    }

    keys.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    (true, CommandResponseData::RegistryKeys { keys })
}

/// List values in a registry key
pub fn list_values(path: &str) -> (bool, CommandResponseData) {
    let (root, subpath) = match parse_registry_path(path) {
        Ok(r) => r,
        Err(e) => return (false, CommandResponseData::Error { message: e }),
    };

    let key = if subpath.is_empty() {
        root
    } else {
        match root.open_subkey(&subpath) {
            Ok(k) => k,
            Err(e) => return (false, CommandResponseData::Error {
                message: format!("Failed to open key: {}", e)
            }),
        }
    };

    let mut values = Vec::new();
    for (name, value) in key.enum_values().filter_map(|r| r.ok()) {
        let (value_type, data) = format_reg_value(&value);
        values.push(RegistryValueInfo {
            name: if name.is_empty() { "(Default)".to_string() } else { name },
            value_type,
            data,
        });
    }

    // Sort with (Default) first, then alphabetically
    values.sort_by(|a, b| {
        if a.name == "(Default)" { std::cmp::Ordering::Less }
        else if b.name == "(Default)" { std::cmp::Ordering::Greater }
        else { a.name.to_lowercase().cmp(&b.name.to_lowercase()) }
    });

    (true, CommandResponseData::RegistryValues { values })
}

/// Format a registry value to (type_string, data_string)
fn format_reg_value(value: &winreg::RegValue) -> (String, String) {
    use winreg::enums::RegType;

    let type_str = match value.vtype {
        RegType::REG_SZ => "String",
        RegType::REG_EXPAND_SZ => "ExpandString",
        RegType::REG_MULTI_SZ => "MultiString",
        RegType::REG_DWORD => "DWord",
        RegType::REG_QWORD => "QWord",
        RegType::REG_BINARY => "Binary",
        RegType::REG_NONE => "None",
        _ => "Unknown",
    }.to_string();

    let data_str = match value.vtype {
        RegType::REG_SZ | RegType::REG_EXPAND_SZ => {
            String::from_utf16_lossy(
                &value.bytes.chunks(2)
                    .map(|c| u16::from_le_bytes([c[0], *c.get(1).unwrap_or(&0)]))
                    .collect::<Vec<_>>()
            ).trim_end_matches('\0').to_string()
        }
        RegType::REG_MULTI_SZ => {
            let wide: Vec<u16> = value.bytes.chunks(2)
                .map(|c| u16::from_le_bytes([c[0], *c.get(1).unwrap_or(&0)]))
                .collect();
            String::from_utf16_lossy(&wide)
                .trim_end_matches('\0')
                .split('\0')
                .collect::<Vec<_>>()
                .join("; ")
        }
        RegType::REG_DWORD => {
            if value.bytes.len() >= 4 {
                let val = u32::from_le_bytes([value.bytes[0], value.bytes[1], value.bytes[2], value.bytes[3]]);
                format!("0x{:08X} ({})", val, val)
            } else {
                "(invalid)".to_string()
            }
        }
        RegType::REG_QWORD => {
            if value.bytes.len() >= 8 {
                let val = u64::from_le_bytes([
                    value.bytes[0], value.bytes[1], value.bytes[2], value.bytes[3],
                    value.bytes[4], value.bytes[5], value.bytes[6], value.bytes[7],
                ]);
                format!("0x{:016X} ({})", val, val)
            } else {
                "(invalid)".to_string()
            }
        }
        RegType::REG_BINARY | RegType::REG_NONE | _ => {
            if value.bytes.len() <= 64 {
                value.bytes.iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" ")
            } else {
                format!("{} ({} bytes)",
                    value.bytes.iter().take(32).map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" "),
                    value.bytes.len()
                )
            }
        }
    };

    (type_str, data_str)
}

/// Get a specific registry value
pub fn get_value(path: &str, name: &str) -> (bool, CommandResponseData) {
    let (root, subpath) = match parse_registry_path(path) {
        Ok(r) => r,
        Err(e) => return (false, CommandResponseData::Error { message: e }),
    };

    let key = if subpath.is_empty() {
        root
    } else {
        match root.open_subkey(&subpath) {
            Ok(k) => k,
            Err(e) => return (false, CommandResponseData::Error {
                message: format!("Failed to open key: {}", e)
            }),
        }
    };

    let value_name = if name == "(Default)" { "" } else { name };
    match key.get_raw_value(value_name) {
        Ok(value) => {
            let (value_type, data) = format_reg_value(&value);
            (true, CommandResponseData::RegistryValues {
                values: vec![RegistryValueInfo {
                    name: name.to_string(),
                    value_type,
                    data,
                }]
            })
        }
        Err(e) => (false, CommandResponseData::Error {
            message: format!("Failed to get value: {}", e)
        }),
    }
}

/// Set/create a registry value
pub fn set_value(path: &str, name: &str, value_type: &str, data: &str) -> (bool, CommandResponseData) {
    let (root, subpath) = match parse_registry_path(path) {
        Ok(r) => r,
        Err(e) => return (false, CommandResponseData::Error { message: e }),
    };

    let key = if subpath.is_empty() {
        root
    } else {
        match root.open_subkey_with_flags(&subpath, KEY_WRITE | KEY_READ) {
            Ok(k) => k,
            Err(e) => return (false, CommandResponseData::Error {
                message: format!("Failed to open key for writing: {}", e)
            }),
        }
    };

    let value_name = if name == "(Default)" { "" } else { name };

    let result = match value_type {
        "String" => key.set_value(value_name, &data),
        "ExpandString" => {
            use winreg::RegValue;
            let mut bytes: Vec<u8> = data.encode_utf16()
                .flat_map(|c| c.to_le_bytes())
                .collect();
            bytes.extend_from_slice(&[0, 0]); // null terminator
            key.set_raw_value(value_name, &RegValue {
                vtype: winreg::enums::RegType::REG_EXPAND_SZ,
                bytes
            })
        }
        "MultiString" => {
            use winreg::RegValue;
            let strings: Vec<&str> = data.split(';').map(|s| s.trim()).collect();
            let mut bytes: Vec<u8> = Vec::new();
            for s in strings {
                bytes.extend(s.encode_utf16().flat_map(|c| c.to_le_bytes()));
                bytes.extend_from_slice(&[0, 0]); // null terminator for each string
            }
            bytes.extend_from_slice(&[0, 0]); // double null terminator
            key.set_raw_value(value_name, &RegValue {
                vtype: winreg::enums::RegType::REG_MULTI_SZ,
                bytes
            })
        }
        "DWord" => {
            // Parse as hex (0x...) or decimal
            let val: u32 = if data.starts_with("0x") || data.starts_with("0X") {
                u32::from_str_radix(&data[2..], 16).unwrap_or(0)
            } else {
                data.parse().unwrap_or(0)
            };
            key.set_value(value_name, &val)
        }
        "QWord" => {
            let val: u64 = if data.starts_with("0x") || data.starts_with("0X") {
                u64::from_str_radix(&data[2..], 16).unwrap_or(0)
            } else {
                data.parse().unwrap_or(0)
            };
            key.set_value(value_name, &val)
        }
        "Binary" => {
            use winreg::RegValue;
            // Parse hex string like "00 1A 2B 3C" or "001A2B3C"
            let hex = data.replace(" ", "");
            let bytes: Vec<u8> = (0..hex.len())
                .step_by(2)
                .filter_map(|i| u8::from_str_radix(&hex[i..i+2], 16).ok())
                .collect();
            key.set_raw_value(value_name, &RegValue {
                vtype: winreg::enums::RegType::REG_BINARY,
                bytes
            })
        }
        _ => return (false, CommandResponseData::Error {
            message: format!("Unknown value type: {}", value_type)
        }),
    };

    match result {
        Ok(_) => (true, CommandResponseData::RegistryResult {
            success: true,
            message: "Value set successfully".to_string()
        }),
        Err(e) => (false, CommandResponseData::Error {
            message: format!("Failed to set value: {}", e)
        }),
    }
}

/// Delete a registry value
pub fn delete_value(path: &str, name: &str) -> (bool, CommandResponseData) {
    let (root, subpath) = match parse_registry_path(path) {
        Ok(r) => r,
        Err(e) => return (false, CommandResponseData::Error { message: e }),
    };

    let key = match root.open_subkey_with_flags(&subpath, KEY_WRITE) {
        Ok(k) => k,
        Err(e) => return (false, CommandResponseData::Error {
            message: format!("Failed to open key for writing: {}", e)
        }),
    };

    let value_name = if name == "(Default)" { "" } else { name };
    match key.delete_value(value_name) {
        Ok(_) => (true, CommandResponseData::RegistryResult {
            success: true,
            message: "Value deleted successfully".to_string()
        }),
        Err(e) => (false, CommandResponseData::Error {
            message: format!("Failed to delete value: {}", e)
        }),
    }
}

/// Create a registry key
pub fn create_key(path: &str) -> (bool, CommandResponseData) {
    let (root, subpath) = match parse_registry_path(path) {
        Ok(r) => r,
        Err(e) => return (false, CommandResponseData::Error { message: e }),
    };

    if subpath.is_empty() {
        return (false, CommandResponseData::Error {
            message: "Cannot create root key".to_string()
        });
    }

    match root.create_subkey(&subpath) {
        Ok(_) => (true, CommandResponseData::RegistryResult {
            success: true,
            message: "Key created successfully".to_string()
        }),
        Err(e) => (false, CommandResponseData::Error {
            message: format!("Failed to create key: {}", e)
        }),
    }
}

/// Delete a registry key
pub fn delete_key(path: &str, recursive: bool) -> (bool, CommandResponseData) {
    let (root, subpath) = match parse_registry_path(path) {
        Ok(r) => r,
        Err(e) => return (false, CommandResponseData::Error { message: e }),
    };

    if subpath.is_empty() {
        return (false, CommandResponseData::Error {
            message: "Cannot delete root key".to_string()
        });
    }

    let result = if recursive {
        root.delete_subkey_all(&subpath)
    } else {
        root.delete_subkey(&subpath)
    };

    match result {
        Ok(_) => (true, CommandResponseData::RegistryResult {
            success: true,
            message: "Key deleted successfully".to_string()
        }),
        Err(e) => (false, CommandResponseData::Error {
            message: format!("Failed to delete key: {}", e)
        }),
    }
}
