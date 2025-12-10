//! Local security enumeration module.
//!
//! Implements local group enumeration and remote access rights checking.

use wavegate_shared::{
    LocalGroupInfo, LocalGroupMember, RemoteAccessInfo, CommandResponseData,
};

use windows::core::PCWSTR;
use windows::Win32::NetworkManagement::NetManagement::{
    NetLocalGroupEnum, NetLocalGroupGetMembers, NetApiBufferFree,
    LOCALGROUP_INFO_1, LOCALGROUP_MEMBERS_INFO_2,
};
use windows::Win32::Security::SID_NAME_USE;
use windows::Win32::System::Services::{
    OpenSCManagerW, OpenServiceW, QueryServiceStatus,
    SC_MANAGER_CONNECT, SERVICE_QUERY_STATUS, SERVICE_STATUS,
    SERVICE_RUNNING,
};
use windows::Win32::System::Registry::{
    RegOpenKeyExW, RegQueryValueExW, RegCloseKey,
    HKEY_LOCAL_MACHINE, KEY_READ, REG_DWORD, REG_VALUE_TYPE,
};

use std::ptr::null_mut;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;

/// Convert wide string pointer to Rust String
fn wide_to_string(wide: *const u16) -> String {
    if wide.is_null() {
        return String::new();
    }
    unsafe {
        let len = (0..).take_while(|&i| *wide.add(i) != 0).count();
        let slice = std::slice::from_raw_parts(wide, len);
        OsString::from_wide(slice).to_string_lossy().into_owned()
    }
}

/// Convert Rust string to wide string (null-terminated)
fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Key local groups to enumerate
const INTERESTING_GROUPS: &[&str] = &[
    "Administrators",
    "Remote Desktop Users",
    "Remote Management Users",
    "Distributed COM Users",
    "Backup Operators",
    "Power Users",
    "Hyper-V Administrators",
    "Event Log Readers",
    "Network Configuration Operators",
];

/// Enumerate local groups and their members
pub fn enum_local_groups() -> (bool, CommandResponseData) {
    let mut groups = Vec::new();

    // First, get all local groups
    let all_groups = match get_all_local_groups() {
        Ok(g) => g,
        Err(e) => return (false, CommandResponseData::Error {
            message: format!("Failed to enumerate local groups: {}", e)
        }),
    };

    // Filter to interesting groups and get members
    for group_name in all_groups {
        // Check if it's an interesting group
        let is_interesting = INTERESTING_GROUPS.iter()
            .any(|&ig| group_name.eq_ignore_ascii_case(ig));

        if !is_interesting {
            continue;
        }

        // Get group members
        let members = get_local_group_members(&group_name).unwrap_or_default();

        groups.push(LocalGroupInfo {
            name: group_name,
            comment: None,
            members,
        });
    }

    (true, CommandResponseData::LocalGroupList { groups })
}

/// Get all local group names
fn get_all_local_groups() -> Result<Vec<String>, String> {
    unsafe {
        let mut buffer: *mut u8 = null_mut();
        let mut entries_read: u32 = 0;
        let mut total_entries: u32 = 0;

        let result = NetLocalGroupEnum(
            PCWSTR::null(), // Local computer
            1,              // Level 1 = LOCALGROUP_INFO_1
            &mut buffer,
            u32::MAX,       // Prefer max
            &mut entries_read,
            &mut total_entries,
            None,
        );

        if result != 0 || buffer.is_null() {
            return Err(format!("NetLocalGroupEnum failed: {}", result));
        }

        let mut groups = Vec::new();
        let info_array = buffer as *const LOCALGROUP_INFO_1;

        for i in 0..entries_read as isize {
            let info = &*info_array.offset(i);
            let name = wide_to_string(info.lgrpi1_name.0);
            if !name.is_empty() {
                groups.push(name);
            }
        }

        let _ = NetApiBufferFree(Some(buffer as *const _));
        Ok(groups)
    }
}

/// Get members of a specific local group
fn get_local_group_members(group_name: &str) -> Result<Vec<LocalGroupMember>, String> {
    unsafe {
        let group_wide = to_wide(group_name);
        let mut buffer: *mut u8 = null_mut();
        let mut entries_read: u32 = 0;
        let mut total_entries: u32 = 0;

        let result = NetLocalGroupGetMembers(
            PCWSTR::null(),
            PCWSTR::from_raw(group_wide.as_ptr()),
            2, // Level 2 = LOCALGROUP_MEMBERS_INFO_2 (includes SID and domain)
            &mut buffer,
            u32::MAX,
            &mut entries_read,
            &mut total_entries,
            None,
        );

        if result != 0 || buffer.is_null() {
            // Group might be empty or access denied
            return Ok(Vec::new());
        }

        let mut members = Vec::new();
        let info_array = buffer as *const LOCALGROUP_MEMBERS_INFO_2;

        for i in 0..entries_read as isize {
            let info = &*info_array.offset(i);

            let name = wide_to_string(info.lgrmi2_domainandname.0);
            let domain = if name.contains('\\') {
                Some(name.split('\\').next().unwrap_or("").to_string())
            } else {
                None
            };

            // Convert SID to string
            let sid_string = sid_to_string(info.lgrmi2_sid);

            // Map sidusage to member type
            // SID_NAME_USE values: 1=User, 2=Group, 5=WellKnownGroup, 4=Alias, 6=DeletedAccount, 8=Unknown
            let member_type = match info.lgrmi2_sidusage.0 {
                1 => "User",
                2 => "Group",
                5 => "WellKnownGroup",
                4 => "Alias",
                6 => "DeletedAccount",
                8 => "Unknown",
                _ => "Other",
            };

            members.push(LocalGroupMember {
                name,
                sid: sid_string,
                member_type: member_type.to_string(),
                domain,
            });
        }

        let _ = NetApiBufferFree(Some(buffer as *const _));
        Ok(members)
    }
}

/// Convert PSID to string representation
fn sid_to_string(sid: windows::Win32::Security::PSID) -> String {
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows::Win32::Security::IsValidSid;
    use windows::Win32::Foundation::LocalFree;

    if sid.is_invalid() {
        return String::new();
    }

    unsafe {
        if !IsValidSid(sid).as_bool() {
            return String::new();
        }

        let mut string_sid: windows::core::PWSTR = windows::core::PWSTR::null();
        if ConvertSidToStringSidW(sid, &mut string_sid).is_ok() {
            let result = wide_to_string(string_sid.0);
            let _ = LocalFree(Some(windows::Win32::Foundation::HLOCAL(string_sid.0 as *mut _)));
            result
        } else {
            String::new()
        }
    }
}

/// Check remote access rights (RDP, WinRM, DCOM)
pub fn enum_remote_access_rights() -> (bool, CommandResponseData) {
    let mut rdp_access = Vec::new();
    let mut winrm_access = Vec::new();
    let mut dcom_access = Vec::new();

    // Get members of Remote Desktop Users
    if let Ok(members) = get_local_group_members("Remote Desktop Users") {
        for m in members {
            rdp_access.push(m.name);
        }
    }

    // Get members of Administrators (they have all remote access)
    if let Ok(members) = get_local_group_members("Administrators") {
        for m in &members {
            if !rdp_access.contains(&m.name) {
                rdp_access.push(format!("{} (Admin)", m.name));
            }
            winrm_access.push(format!("{} (Admin)", m.name.clone()));
            dcom_access.push(format!("{} (Admin)", m.name.clone()));
        }
    }

    // Get members of Remote Management Users
    if let Ok(members) = get_local_group_members("Remote Management Users") {
        for m in members {
            if !winrm_access.iter().any(|x| x.starts_with(&m.name)) {
                winrm_access.push(m.name);
            }
        }
    }

    // Get members of Distributed COM Users
    if let Ok(members) = get_local_group_members("Distributed COM Users") {
        for m in members {
            if !dcom_access.iter().any(|x| x.starts_with(&m.name)) {
                dcom_access.push(m.name);
            }
        }
    }

    // Check if services are enabled
    let rdp_enabled = is_rdp_enabled();
    let winrm_enabled = is_winrm_enabled();
    let dcom_enabled = is_dcom_enabled();

    let rights = RemoteAccessInfo {
        rdp_access,
        winrm_access,
        dcom_access,
        winrm_enabled,
        rdp_enabled,
        dcom_enabled,
    };

    (true, CommandResponseData::RemoteAccessRights { rights })
}

/// Check if RDP is enabled
fn is_rdp_enabled() -> bool {
    unsafe {
        let key_path = to_wide("SYSTEM\\CurrentControlSet\\Control\\Terminal Server");
        let mut hkey = windows::Win32::System::Registry::HKEY::default();

        let result = RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR::from_raw(key_path.as_ptr()),
            Some(0),
            KEY_READ,
            &mut hkey,
        );

        if result.is_err() {
            return false;
        }

        let value_name = to_wide("fDenyTSConnections");
        let mut value_type = REG_VALUE_TYPE::default();
        let mut data: u32 = 1; // Default to denied
        let mut data_size: u32 = std::mem::size_of::<u32>() as u32;

        let _ = RegQueryValueExW(
            hkey,
            PCWSTR::from_raw(value_name.as_ptr()),
            None,
            Some(&mut value_type),
            Some(&mut data as *mut u32 as *mut u8),
            Some(&mut data_size),
        );

        let _ = RegCloseKey(hkey);

        // 0 = RDP enabled, 1 = RDP disabled
        data == 0
    }
}

/// Check if WinRM service is running
fn is_winrm_enabled() -> bool {
    unsafe {
        let scm = OpenSCManagerW(
            PCWSTR::null(),
            PCWSTR::null(),
            SC_MANAGER_CONNECT,
        );

        let scm = match scm {
            Ok(h) => h,
            Err(_) => return false,
        };

        let service_name = to_wide("WinRM");
        let service = OpenServiceW(
            scm,
            PCWSTR::from_raw(service_name.as_ptr()),
            SERVICE_QUERY_STATUS,
        );

        let service = match service {
            Ok(h) => h,
            Err(_) => return false,
        };

        let mut status = SERVICE_STATUS::default();
        if QueryServiceStatus(service, &mut status).is_ok() {
            return status.dwCurrentState == SERVICE_RUNNING;
        }

        false
    }
}

/// Check if DCOM is enabled (it's enabled by default on Windows)
fn is_dcom_enabled() -> bool {
    unsafe {
        let key_path = to_wide("SOFTWARE\\Microsoft\\Ole");
        let mut hkey = windows::Win32::System::Registry::HKEY::default();

        let result = RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR::from_raw(key_path.as_ptr()),
            Some(0),
            KEY_READ,
            &mut hkey,
        );

        if result.is_err() {
            // If key doesn't exist, DCOM is likely enabled (default)
            return true;
        }

        let value_name = to_wide("EnableDCOM");
        let mut data = [0u8; 4];
        let mut data_size: u32 = 4;
        let mut value_type = REG_VALUE_TYPE::default();

        let query_result = RegQueryValueExW(
            hkey,
            PCWSTR::from_raw(value_name.as_ptr()),
            None,
            Some(&mut value_type),
            Some(data.as_mut_ptr()),
            Some(&mut data_size),
        );

        let _ = RegCloseKey(hkey);

        if query_result.is_err() {
            // Default is enabled
            return true;
        }

        // EnableDCOM is typically "Y" (0x59) or "N" (0x4E) as a string
        // Or could be DWORD 1/0
        if value_type == REG_DWORD {
            let value = u32::from_le_bytes(data);
            value != 0
        } else {
            // String value - check for "Y"
            data[0] == b'Y' || data[0] == b'y'
        }
    }
}
