//! VM detection via WMI fan query.
//!
//! Checks for CPU fan presence
use std::collections::HashMap;
use wmi::{COMLibrary, WMIConnection, Variant};
use windows::core::PCWSTR;
use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_OK, MB_ICONERROR};

/// Returns true if fan found, false if VM detected.
pub fn check_environment() -> bool {
    match detect_fan() {
        Ok(has_fan) => {
            if has_fan {
                true // Bare metal, continue
            } else {
                show_wmi_error();
                false // VM detected
            }
        }
        Err(_) => {
            // WMI query failed - could be VM or broken WMI
            show_wmi_error();
            false
        }
    }
}

/// Query WMI for Win32_Fan instances
fn detect_fan() -> Result<bool, wmi::WMIError> {
    let com_lib = COMLibrary::new()?;
    let wmi_con = WMIConnection::new(com_lib)?;
    let results: Vec<HashMap<String, Variant>> = wmi_con.raw_query("SELECT * FROM Win32_Fan")?;
    Ok(!results.is_empty())
}

/// Show error messagebox
fn show_wmi_error() {
    let title: Vec<u16> = "System Error\0".encode_utf16().collect();
    let message: Vec<u16> = "WMI is broken or misconfigured on your machine.\0".encode_utf16().collect();
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(message.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}
