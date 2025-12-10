//! TCP connections manager for the client.
//!
//! Enumerates established TCP connections with their owning process info.

use wavegate_shared::{CommandResponseData, TcpConnectionInfo};
use std::collections::HashMap;

/// List all established TCP connections
pub fn list_tcp_connections() -> (bool, CommandResponseData) {
    match get_tcp_connections() {
        Ok(connections) => (true, CommandResponseData::TcpConnectionList { connections }),
        Err(e) => (false, CommandResponseData::Error { message: e }),
    }
}

/// Get TCP connections using Windows API
fn get_tcp_connections() -> Result<Vec<TcpConnectionInfo>, String> {
    use std::mem;
    use windows::Win32::Foundation::{CloseHandle, ERROR_INSUFFICIENT_BUFFER};
    use windows::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCP_STATE_ESTAB, MIB_TCPROW_OWNER_PID,
        MIB_TCPTABLE_OWNER_PID, TCP_TABLE_OWNER_PID_ALL,
    };
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW,
        PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };

    let mut connections = Vec::new();

    // First, build a map of PID -> process name
    let process_names = get_process_names()?;

    unsafe {
        // Get required buffer size
        let mut size: u32 = 0;
        let result = GetExtendedTcpTable(
            None,
            &mut size,
            false,
            windows::Win32::Networking::WinSock::AF_INET.0 as u32,
            TCP_TABLE_OWNER_PID_ALL,
            0,
        );

        if result != ERROR_INSUFFICIENT_BUFFER.0 && result != 0 {
            return Err(format!("GetExtendedTcpTable failed: {}", result));
        }

        // Allocate buffer
        let mut buffer: Vec<u8> = vec![0u8; size as usize];

        // Get the table
        let result = GetExtendedTcpTable(
            Some(buffer.as_mut_ptr() as *mut _),
            &mut size,
            false,
            windows::Win32::Networking::WinSock::AF_INET.0 as u32,
            TCP_TABLE_OWNER_PID_ALL,
            0,
        );

        if result != 0 {
            return Err(format!("GetExtendedTcpTable failed: {}", result));
        }

        // Parse the table
        let table = &*(buffer.as_ptr() as *const MIB_TCPTABLE_OWNER_PID);
        let num_entries = table.dwNumEntries as usize;

        // Get pointer to first row
        let rows_ptr = table.table.as_ptr();

        for i in 0..num_entries {
            let row = &*rows_ptr.add(i);

            // Only include established connections
            if row.dwState != MIB_TCP_STATE_ESTAB.0 as u32 {
                continue;
            }

            let local_addr = format_ip_addr(row.dwLocalAddr);
            let local_port = u16::from_be(row.dwLocalPort as u16);
            let remote_addr = format_ip_addr(row.dwRemoteAddr);
            let remote_port = u16::from_be(row.dwRemotePort as u16);
            let pid = row.dwOwningPid;

            let process_name = process_names
                .get(&pid)
                .cloned()
                .unwrap_or_else(|| "<unknown>".to_string());

            connections.push(TcpConnectionInfo {
                local_address: format!("{}:{}", local_addr, local_port),
                remote_address: format!("{}:{}", remote_addr, remote_port),
                pid,
                process_name,
                state: "ESTABLISHED".to_string(),
            });
        }
    }

    // Sort by process name
    connections.sort_by(|a, b| a.process_name.to_lowercase().cmp(&b.process_name.to_lowercase()));

    Ok(connections)
}

/// Build a map of PID -> process name
fn get_process_names() -> Result<HashMap<u32, String>, String> {
    use std::mem;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW,
        PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };

    let mut map = HashMap::new();

    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
            .map_err(|e| format!("CreateToolhelp32Snapshot failed: {}", e))?;

        let mut entry: PROCESSENTRY32W = mem::zeroed();
        entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;

        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let name_len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..name_len]);
                let pid = entry.th32ProcessID;

                map.insert(pid, name);

                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }

        let _ = CloseHandle(snapshot);
    }

    Ok(map)
}

/// Format an IPv4 address from a u32 (in network byte order)
fn format_ip_addr(addr: u32) -> String {
    let bytes = addr.to_ne_bytes();
    format!("{}.{}.{}.{}", bytes[0], bytes[1], bytes[2], bytes[3])
}

/// Kill a TCP connection by terminating its owning process
pub fn kill_tcp_connection(pid: u32) -> (bool, CommandResponseData) {
    // Reuse the process manager's kill function
    crate::processmanager::kill_process(pid)
}
