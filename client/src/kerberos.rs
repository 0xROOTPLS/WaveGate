//! Kerberos ticket management module.
//!
//! Implements ticket listing and cache management (klist equivalent).

use wavegate_shared::{KerberosTicket, CommandResponseData};

use windows::Win32::Foundation::{HANDLE, LUID, STATUS_SUCCESS};
use windows::Win32::Security::Authentication::Identity::{
    LsaConnectUntrusted, LsaLookupAuthenticationPackage, LsaCallAuthenticationPackage,
    LsaDeregisterLogonProcess, LsaFreeReturnBuffer,
    KERB_QUERY_TKT_CACHE_REQUEST,
    KERB_QUERY_TKT_CACHE_EX_RESPONSE, KERB_TICKET_CACHE_INFO_EX,
    KERB_PURGE_TKT_CACHE_REQUEST, LSA_UNICODE_STRING,
    KerbQueryTicketCacheExMessage, KerbPurgeTicketCacheMessage,
    LSA_STRING,
};

use std::ptr::null_mut;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;

/// Extract Kerberos tickets from current session (equivalent to `klist`)
pub fn extract_tickets() -> (bool, CommandResponseData) {
    unsafe {
        let mut tickets = Vec::new();

        // Connect to LSA
        let mut lsa_handle: HANDLE = HANDLE::default();
        let status = LsaConnectUntrusted(&mut lsa_handle);

        if status != STATUS_SUCCESS {
            return (false, CommandResponseData::Error {
                message: format!("LsaConnectUntrusted failed: {:?}", status)
            });
        }

        // Get Kerberos package ID
        let mut kerb_pkg_id: u32 = 0;
        let kerb_bytes = b"Kerberos\0";
        let kerb_name = LSA_STRING {
            Length: 8,
            MaximumLength: 9,
            Buffer: windows::core::PSTR::from_raw(kerb_bytes.as_ptr() as *mut u8),
        };

        let status = LsaLookupAuthenticationPackage(lsa_handle, &kerb_name, &mut kerb_pkg_id);
        if status != STATUS_SUCCESS {
            let _ = LsaDeregisterLogonProcess(lsa_handle);
            return (false, CommandResponseData::Error {
                message: format!("LsaLookupAuthenticationPackage failed: {:?}", status)
            });
        }

        // Query ticket cache
        let request = KERB_QUERY_TKT_CACHE_REQUEST {
            MessageType: KerbQueryTicketCacheExMessage,
            LogonId: LUID::default(),
        };

        let mut response: *mut std::ffi::c_void = null_mut();
        let mut response_len: u32 = 0;
        let mut protocol_status: i32 = 0;

        let status = LsaCallAuthenticationPackage(
            lsa_handle,
            kerb_pkg_id,
            &request as *const _ as *const _,
            std::mem::size_of::<KERB_QUERY_TKT_CACHE_REQUEST>() as u32,
            Some(&mut response),
            Some(&mut response_len),
            Some(&mut protocol_status),
        );

        if status != STATUS_SUCCESS || response.is_null() {
            let _ = LsaDeregisterLogonProcess(lsa_handle);
            return (false, CommandResponseData::Error {
                message: format!("LsaCallAuthenticationPackage failed: {:?}", status)
            });
        }

        // Parse the response
        let cache_response = response as *const KERB_QUERY_TKT_CACHE_EX_RESPONSE;
        let count = (*cache_response).CountOfTickets as usize;

        if count > 0 {
            let ticket_array = std::ptr::addr_of!((*cache_response).Tickets) as *const KERB_TICKET_CACHE_INFO_EX;

            for i in 0..count {
                let ticket_info = &*ticket_array.add(i);

                let client_name = lsa_unicode_to_string(&ticket_info.ClientName);
                let client_realm = lsa_unicode_to_string(&ticket_info.ClientRealm);
                let server_name = lsa_unicode_to_string(&ticket_info.ServerName);
                let server_realm = lsa_unicode_to_string(&ticket_info.ServerRealm);

                // Map encryption type
                let etype = match ticket_info.EncryptionType {
                    23 => "RC4-HMAC",
                    17 => "AES128-CTS",
                    18 => "AES256-CTS",
                    _ => "Unknown",
                };

                // Convert FILETIME to string
                let start_time = filetime_to_string(ticket_info.StartTime);
                let end_time = filetime_to_string(ticket_info.EndTime);
                let renew_time = filetime_to_string(ticket_info.RenewTime);

                // Parse flags
                let flags = parse_ticket_flags(ticket_info.TicketFlags);

                tickets.push(KerberosTicket {
                    client_name,
                    client_realm,
                    server_name,
                    server_realm,
                    etype: etype.to_string(),
                    start_time,
                    end_time,
                    renew_until: if renew_time.is_empty() { None } else { Some(renew_time) },
                    flags,
                    ticket_b64: None,
                });
            }
        }

        let _ = LsaFreeReturnBuffer(response);
        let _ = LsaDeregisterLogonProcess(lsa_handle);

        (true, CommandResponseData::KerberosTicketList { tickets })
    }
}

/// Purge all Kerberos tickets from current session (equivalent to `klist purge`)
pub fn purge_tickets() -> (bool, CommandResponseData) {
    unsafe {
        // Connect to LSA
        let mut lsa_handle: HANDLE = HANDLE::default();
        let status = LsaConnectUntrusted(&mut lsa_handle);

        if status != STATUS_SUCCESS {
            return (false, CommandResponseData::KerberosResult {
                success: false,
                message: format!("LsaConnectUntrusted failed: {:?}", status)
            });
        }

        // Get Kerberos package ID
        let mut kerb_pkg_id: u32 = 0;
        let kerb_bytes = b"Kerberos\0";
        let kerb_name = LSA_STRING {
            Length: 8,
            MaximumLength: 9,
            Buffer: windows::core::PSTR::from_raw(kerb_bytes.as_ptr() as *mut u8),
        };

        let status = LsaLookupAuthenticationPackage(lsa_handle, &kerb_name, &mut kerb_pkg_id);
        if status != STATUS_SUCCESS {
            let _ = LsaDeregisterLogonProcess(lsa_handle);
            return (false, CommandResponseData::KerberosResult {
                success: false,
                message: format!("LsaLookupAuthenticationPackage failed: {:?}", status)
            });
        }

        // Purge ticket cache
        let request = KERB_PURGE_TKT_CACHE_REQUEST {
            MessageType: KerbPurgeTicketCacheMessage,
            LogonId: LUID::default(),
            ServerName: LSA_UNICODE_STRING::default(),
            RealmName: LSA_UNICODE_STRING::default(),
        };

        let mut response: *mut std::ffi::c_void = null_mut();
        let mut response_len: u32 = 0;
        let mut protocol_status: i32 = 0;

        let status = LsaCallAuthenticationPackage(
            lsa_handle,
            kerb_pkg_id,
            &request as *const _ as *const _,
            std::mem::size_of::<KERB_PURGE_TKT_CACHE_REQUEST>() as u32,
            Some(&mut response),
            Some(&mut response_len),
            Some(&mut protocol_status),
        );

        if !response.is_null() {
            let _ = LsaFreeReturnBuffer(response);
        }
        let _ = LsaDeregisterLogonProcess(lsa_handle);

        if status == STATUS_SUCCESS && protocol_status == 0 {
            (true, CommandResponseData::KerberosResult {
                success: true,
                message: "Kerberos ticket cache purged".to_string()
            })
        } else {
            (false, CommandResponseData::KerberosResult {
                success: false,
                message: format!("Purge failed: status={:?}, protocol_status={}", status, protocol_status)
            })
        }
    }
}

/// Convert LSA_UNICODE_STRING to Rust String
fn lsa_unicode_to_string(lsa: &LSA_UNICODE_STRING) -> String {
    if lsa.Buffer.is_null() || lsa.Length == 0 {
        return String::new();
    }
    unsafe {
        let len = (lsa.Length / 2) as usize;
        let slice = std::slice::from_raw_parts(lsa.Buffer.0, len);
        OsString::from_wide(slice).to_string_lossy().into_owned()
    }
}

/// Convert FILETIME (i64) to string
fn filetime_to_string(ft: i64) -> String {
    if ft == 0 || ft == i64::MAX {
        return String::new();
    }

    // FILETIME is 100-nanosecond intervals since 1601-01-01
    // Convert to Unix timestamp
    let unix_epoch = 116444736000000000i64; // 1970-01-01 in FILETIME
    let unix_ts = (ft - unix_epoch) / 10000000;

    // Simple date formatting
    let secs = unix_ts % 60;
    let mins = (unix_ts / 60) % 60;
    let hours = (unix_ts / 3600) % 24;
    let days = unix_ts / 86400;

    // Approximate date calculation
    let years = 1970 + days / 365;
    let remaining_days = days % 365;
    let months = remaining_days / 30 + 1;
    let day = remaining_days % 30 + 1;

    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        years, months, day, hours, mins, secs)
}

/// Parse Kerberos ticket flags
fn parse_ticket_flags(flags: u32) -> Vec<String> {
    let mut result = Vec::new();

    if flags & 0x40000000 != 0 { result.push("forwardable".to_string()); }
    if flags & 0x20000000 != 0 { result.push("forwarded".to_string()); }
    if flags & 0x10000000 != 0 { result.push("proxiable".to_string()); }
    if flags & 0x08000000 != 0 { result.push("proxy".to_string()); }
    if flags & 0x04000000 != 0 { result.push("may-postdate".to_string()); }
    if flags & 0x02000000 != 0 { result.push("postdated".to_string()); }
    if flags & 0x01000000 != 0 { result.push("invalid".to_string()); }
    if flags & 0x00800000 != 0 { result.push("renewable".to_string()); }
    if flags & 0x00400000 != 0 { result.push("initial".to_string()); }
    if flags & 0x00200000 != 0 { result.push("pre-authent".to_string()); }
    if flags & 0x00100000 != 0 { result.push("hw-authent".to_string()); }
    if flags & 0x00020000 != 0 { result.push("ok-as-delegate".to_string()); }
    if flags & 0x00010000 != 0 { result.push("name-canonicalize".to_string()); }

    result
}
