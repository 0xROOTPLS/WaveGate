//! Active Directory enumeration module.
//!
//! Uses ldap3 for directory queries with GSSAPI authentication,
//! Windows APIs for DC discovery and trusts, WMI for sessions.

use wavegate_shared::{
    AdUser, AdGroup, AdGroupMember, AdComputer, AdSpnEntry, AdSession, AdTrust,
    AdObjectAcl, AdAce, CommandResponseData,
};

use windows::core::PCWSTR;
use windows::Win32::NetworkManagement::NetManagement::NetApiBufferFree;
use windows::Win32::Networking::ActiveDirectory::{
    DsGetDcNameW, DsEnumerateDomainTrustsW, DS_DOMAIN_TRUSTSW,
    DOMAIN_CONTROLLER_INFOW,
};

use ldap3::{LdapConnAsync, Scope, SearchEntry};
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

/// Get LDAP connection to domain controller
async fn get_ldap_connection() -> Result<(ldap3::Ldap, String, String), String> {
    // Get DC info using Windows API
    let (dc_host, domain_dn) = unsafe {
        let mut dc_info: *mut DOMAIN_CONTROLLER_INFOW = null_mut();

        let result = DsGetDcNameW(
            PCWSTR::null(),
            PCWSTR::null(),
            None,
            PCWSTR::null(),
            0,
            &mut dc_info,
        );

        if result != 0 || dc_info.is_null() {
            return Err("Not domain joined or DC unavailable".to_string());
        }

        let dc = &*dc_info;
        let dc_name = wide_to_string(dc.DomainControllerName.0)
            .trim_start_matches("\\\\")
            .to_string();
        let domain_name = wide_to_string(dc.DomainName.0);

        let _ = NetApiBufferFree(Some(dc_info as *const _));

        // Convert domain name to DN (e.g., "corp.local" -> "DC=corp,DC=local")
        let dn = domain_name.split('.')
            .map(|part| format!("DC={}", part))
            .collect::<Vec<_>>()
            .join(",");

        (dc_name, dn)
    };

    // Connect to LDAP
    let ldap_url = format!("ldap://{}:389", dc_host);
    let (conn, mut ldap) = LdapConnAsync::new(&ldap_url)
        .await
        .map_err(|e| format!("LDAP connect failed: {}", e))?;

    // Drive the connection
    ldap3::drive!(conn);

    // Try GSSAPI (Kerberos) bind first, fall back to anonymous
    match ldap.sasl_gssapi_bind(&dc_host).await {
        Ok(_) => {},
        Err(_) => {
            // Fall back to anonymous bind (limited access but may work for some queries)
            ldap.simple_bind("", "")
                .await
                .map_err(|e| format!("LDAP bind failed: {}", e))?;
        }
    }

    Ok((ldap, domain_dn, dc_host))
}

/// Get domain information
pub fn get_domain_info() -> (bool, CommandResponseData) {
    // Use tokio runtime for async LDAP
    let rt = match tokio::runtime::Handle::try_current() {
        Ok(h) => h,
        Err(_) => return (false, CommandResponseData::Error {
            message: "No async runtime available".to_string()
        }),
    };

    rt.block_on(async {
        get_domain_info_async().await
    })
}

async fn get_domain_info_async() -> (bool, CommandResponseData) {
    // Get DC info using Windows API first
    let (dc_name, dc_ip, domain_name, forest_name) = unsafe {
        let mut dc_info: *mut DOMAIN_CONTROLLER_INFOW = null_mut();

        let result = DsGetDcNameW(
            PCWSTR::null(),
            PCWSTR::null(),
            None,
            PCWSTR::null(),
            0,
            &mut dc_info,
        );

        if result != 0 || dc_info.is_null() {
            return (false, CommandResponseData::AdDomainInfo {
                domain_name: String::new(),
                forest_name: String::new(),
                domain_controller: String::new(),
                domain_controller_ip: String::new(),
                functional_level: String::new(),
                is_domain_joined: false,
            });
        }

        let dc = &*dc_info;
        let dc_name = wide_to_string(dc.DomainControllerName.0)
            .trim_start_matches("\\\\")
            .to_string();
        let dc_ip = wide_to_string(dc.DomainControllerAddress.0)
            .trim_start_matches("\\\\")
            .to_string();
        let domain = wide_to_string(dc.DomainName.0);
        let forest = wide_to_string(dc.DnsForestName.0);

        let _ = NetApiBufferFree(Some(dc_info as *const _));
        (dc_name, dc_ip, domain, forest)
    };

    // Query functional level from rootDSE via LDAP
    let functional_level = match query_functional_level(&dc_name).await {
        Ok(level) => level,
        Err(_) => "Unknown".to_string(),
    };

    (true, CommandResponseData::AdDomainInfo {
        domain_name,
        forest_name,
        domain_controller: dc_name,
        domain_controller_ip: dc_ip,
        functional_level,
        is_domain_joined: true,
    })
}

/// Query domain functional level from rootDSE
async fn query_functional_level(dc_host: &str) -> Result<String, String> {
    let ldap_url = format!("ldap://{}:389", dc_host);
    let (conn, mut ldap) = LdapConnAsync::new(&ldap_url)
        .await
        .map_err(|e| e.to_string())?;

    ldap3::drive!(conn);

    // Anonymous bind for rootDSE (allowed by default)
    ldap.simple_bind("", "").await.map_err(|e| e.to_string())?;

    // Query rootDSE
    let (entries, _) = ldap.search(
        "",
        Scope::Base,
        "(objectClass=*)",
        vec!["domainFunctionality", "forestFunctionality"]
    ).await.map_err(|e| e.to_string())?.success().map_err(|e| e.to_string())?;

    ldap.unbind().await.ok();

    if let Some(entry) = entries.into_iter().next() {
        let entry = SearchEntry::construct(entry);
        if let Some(vals) = entry.attrs.get("domainFunctionality") {
            if let Some(level) = vals.first() {
                return Ok(map_functional_level(level));
            }
        }
    }

    Ok("Unknown".to_string())
}

/// Map numeric functional level to human-readable string
fn map_functional_level(level: &str) -> String {
    match level {
        "0" => "Windows 2000".to_string(),
        "1" => "Windows Server 2003 Interim".to_string(),
        "2" => "Windows Server 2003".to_string(),
        "3" => "Windows Server 2008".to_string(),
        "4" => "Windows Server 2008 R2".to_string(),
        "5" => "Windows Server 2012".to_string(),
        "6" => "Windows Server 2012 R2".to_string(),
        "7" => "Windows Server 2016".to_string(),
        _ => format!("Level {}", level),
    }
}

/// Enumerate domain users via LDAP
pub fn enum_users(filter: Option<String>, search: Option<String>) -> (bool, CommandResponseData) {
    let rt = match tokio::runtime::Handle::try_current() {
        Ok(h) => h,
        Err(_) => return (false, CommandResponseData::Error {
            message: "No async runtime available".to_string()
        }),
    };

    rt.block_on(async {
        enum_users_async(filter, search).await
    })
}

async fn enum_users_async(filter: Option<String>, search: Option<String>) -> (bool, CommandResponseData) {
    let (mut ldap, base_dn, _) = match get_ldap_connection().await {
        Ok(conn) => conn,
        Err(e) => return (false, CommandResponseData::Error { message: e }),
    };

    // Build LDAP filter
    let mut ldap_filter = "(&(objectClass=user)(objectCategory=person)".to_string();

    // Add search term
    if let Some(ref s) = search {
        ldap_filter.push_str(&format!(
            "(|(sAMAccountName=*{}*)(displayName=*{}*)(cn=*{}*))",
            s, s, s
        ));
    }

    // Add type filter
    match filter.as_deref() {
        Some("enabled") => ldap_filter.push_str("(!(userAccountControl:1.2.840.113556.1.4.803:=2))"),
        Some("disabled") => ldap_filter.push_str("(userAccountControl:1.2.840.113556.1.4.803:=2)"),
        Some("admins") => ldap_filter.push_str("(adminCount=1)"),
        _ => {}
    }

    ldap_filter.push(')');

    let attrs = vec![
        "sAMAccountName", "displayName", "userPrincipalName", "distinguishedName",
        "userAccountControl", "adminCount", "servicePrincipalName",
        "lastLogonTimestamp", "pwdLastSet", "description"
    ];

    let (entries, _) = match ldap.search(&base_dn, Scope::Subtree, &ldap_filter, attrs).await {
        Ok(result) => match result.success() {
            Ok(r) => r,
            Err(e) => {
                ldap.unbind().await.ok();
                return (false, CommandResponseData::Error { message: e.to_string() });
            }
        },
        Err(e) => {
            ldap.unbind().await.ok();
            return (false, CommandResponseData::Error { message: e.to_string() });
        }
    };

    let mut users = Vec::new();

    for entry in entries {
        let entry = SearchEntry::construct(entry);

        let sam = entry.attrs.get("sAMAccountName")
            .and_then(|v| v.first())
            .cloned()
            .unwrap_or_default();

        let display = entry.attrs.get("displayName")
            .and_then(|v| v.first())
            .cloned();

        let upn = entry.attrs.get("userPrincipalName")
            .and_then(|v| v.first())
            .cloned();

        let dn = entry.attrs.get("distinguishedName")
            .and_then(|v| v.first())
            .cloned();

        let uac: u32 = entry.attrs.get("userAccountControl")
            .and_then(|v| v.first())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        let is_disabled = (uac & 0x0002) != 0;

        let is_admin = entry.attrs.get("adminCount")
            .and_then(|v| v.first())
            .map(|s| s == "1")
            .unwrap_or(false);

        // Service account = has SPN
        let is_service_account = entry.attrs.get("servicePrincipalName")
            .map(|v| !v.is_empty())
            .unwrap_or(false);

        // Convert Windows FILETIME to ISO string
        let last_logon = entry.attrs.get("lastLogonTimestamp")
            .and_then(|v| v.first())
            .and_then(|s| filetime_to_iso(s));

        let pwd_last_set = entry.attrs.get("pwdLastSet")
            .and_then(|v| v.first())
            .and_then(|s| filetime_to_iso(s));

        let description = entry.attrs.get("description")
            .and_then(|v| v.first())
            .cloned();

        users.push(AdUser {
            sam_account_name: sam,
            display_name: display,
            upn,
            dn,
            enabled: !is_disabled,
            is_admin,
            is_service_account,
            last_logon,
            pwd_last_set,
            description,
            uac_flags: uac,
        });
    }

    ldap.unbind().await.ok();
    (true, CommandResponseData::AdUserList { users })
}

/// Convert Windows FILETIME (100-ns since 1601) to ISO date string
fn filetime_to_iso(filetime_str: &str) -> Option<String> {
    let ft: i64 = filetime_str.parse().ok()?;
    if ft == 0 || ft == i64::MAX {
        return None;
    }

    // Windows FILETIME epoch is 1601-01-01, Unix is 1970-01-01
    // Difference is 11644473600 seconds
    let unix_epoch_offset: i64 = 116444736000000000;
    let unix_ts = (ft - unix_epoch_offset) / 10000000;

    if unix_ts < 0 {
        return None;
    }

    // Simple date formatting
    let secs = unix_ts % 60;
    let mins = (unix_ts / 60) % 60;
    let hours = (unix_ts / 3600) % 24;
    let total_days = unix_ts / 86400;

    // Approximate date (not accounting for leap years precisely)
    let year = 1970 + total_days / 365;
    let day_of_year = total_days % 365;
    let month = day_of_year / 30 + 1;
    let day = day_of_year % 30 + 1;

    Some(format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        year, month.min(12), day.min(31), hours, mins, secs))
}

/// Enumerate domain groups via LDAP
pub fn enum_groups(search: Option<String>) -> (bool, CommandResponseData) {
    let rt = match tokio::runtime::Handle::try_current() {
        Ok(h) => h,
        Err(_) => return (false, CommandResponseData::Error {
            message: "No async runtime available".to_string()
        }),
    };

    rt.block_on(async {
        enum_groups_async(search).await
    })
}

async fn enum_groups_async(search: Option<String>) -> (bool, CommandResponseData) {
    let (mut ldap, base_dn, _) = match get_ldap_connection().await {
        Ok(conn) => conn,
        Err(e) => return (false, CommandResponseData::Error { message: e }),
    };

    let mut ldap_filter = "(objectClass=group)".to_string();
    if let Some(ref s) = search {
        ldap_filter = format!("(&(objectClass=group)(|(sAMAccountName=*{}*)(cn=*{}*)))", s, s);
    }

    let attrs = vec![
        "sAMAccountName", "distinguishedName", "groupType", "description", "member"
    ];

    let (entries, _) = match ldap.search(&base_dn, Scope::Subtree, &ldap_filter, attrs).await {
        Ok(result) => match result.success() {
            Ok(r) => r,
            Err(e) => {
                ldap.unbind().await.ok();
                return (false, CommandResponseData::Error { message: e.to_string() });
            }
        },
        Err(e) => {
            ldap.unbind().await.ok();
            return (false, CommandResponseData::Error { message: e.to_string() });
        }
    };

    let mut groups = Vec::new();

    for entry in entries {
        let entry = SearchEntry::construct(entry);

        let sam = entry.attrs.get("sAMAccountName")
            .and_then(|v| v.first())
            .cloned()
            .unwrap_or_default();

        let dn = entry.attrs.get("distinguishedName")
            .and_then(|v| v.first())
            .cloned();

        let group_type_val: i32 = entry.attrs.get("groupType")
            .and_then(|v| v.first())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        // Parse group type flags
        let scope = if (group_type_val & 0x00000002) != 0 {
            "Global"
        } else if (group_type_val & 0x00000004) != 0 {
            "DomainLocal"
        } else if (group_type_val & 0x00000008) != 0 {
            "Universal"
        } else {
            "Unknown"
        };

        let group_type = if (group_type_val as u32 & 0x80000000) != 0 {
            "Security"
        } else {
            "Distribution"
        };

        let member_count = entry.attrs.get("member")
            .map(|v| v.len() as u32)
            .unwrap_or(0);

        let description = entry.attrs.get("description")
            .and_then(|v| v.first())
            .cloned();

        groups.push(AdGroup {
            sam_account_name: sam,
            dn,
            scope: scope.to_string(),
            group_type: group_type.to_string(),
            member_count,
            description,
        });
    }

    ldap.unbind().await.ok();
    (true, CommandResponseData::AdGroupList { groups })
}

/// Get members of a specific group via LDAP
pub fn get_group_members(group_name: &str) -> (bool, CommandResponseData) {
    let rt = match tokio::runtime::Handle::try_current() {
        Ok(h) => h,
        Err(_) => return (false, CommandResponseData::Error {
            message: "No async runtime available".to_string()
        }),
    };

    rt.block_on(async {
        get_group_members_async(group_name).await
    })
}

async fn get_group_members_async(group_name: &str) -> (bool, CommandResponseData) {
    let (mut ldap, base_dn, _) = match get_ldap_connection().await {
        Ok(conn) => conn,
        Err(e) => return (false, CommandResponseData::Error { message: e }),
    };

    // First find the group
    let group_filter = format!("(&(objectClass=group)(sAMAccountName={}))", group_name);

    let (entries, _) = match ldap.search(&base_dn, Scope::Subtree, &group_filter, vec!["member"]).await {
        Ok(result) => match result.success() {
            Ok(r) => r,
            Err(e) => {
                ldap.unbind().await.ok();
                return (false, CommandResponseData::Error { message: e.to_string() });
            }
        },
        Err(e) => {
            ldap.unbind().await.ok();
            return (false, CommandResponseData::Error { message: e.to_string() });
        }
    };

    let mut members = Vec::new();

    if let Some(entry) = entries.into_iter().next() {
        let entry = SearchEntry::construct(entry);

        if let Some(member_dns) = entry.attrs.get("member") {
            for member_dn in member_dns {
                // Query each member to get details
                // Escape special characters in DN for LDAP filter
                let escaped_dn = member_dn
                    .replace('\\', "\\5c")
                    .replace('(', "\\28")
                    .replace(')', "\\29")
                    .replace('*', "\\2a")
                    .replace('\0', "\\00");
                let member_filter = format!("(distinguishedName={})", escaped_dn);

                if let Ok(result) = ldap.search(
                    &base_dn,
                    Scope::Subtree,
                    &member_filter,
                    vec!["sAMAccountName", "objectClass"]
                ).await {
                    if let Ok((member_entries, _)) = result.success() {
                        if let Some(m_entry) = member_entries.into_iter().next() {
                            let m_entry = SearchEntry::construct(m_entry);

                            let sam = m_entry.attrs.get("sAMAccountName")
                                .and_then(|v| v.first())
                                .cloned()
                                .unwrap_or_default();

                            let obj_classes = m_entry.attrs.get("objectClass")
                                .cloned()
                                .unwrap_or_default();

                            let obj_type = if obj_classes.iter().any(|c| c == "user") {
                                "user"
                            } else if obj_classes.iter().any(|c| c == "group") {
                                "group"
                            } else if obj_classes.iter().any(|c| c == "computer") {
                                "computer"
                            } else {
                                "unknown"
                            };

                            members.push(AdGroupMember {
                                sam_account_name: sam,
                                dn: member_dn.clone(),
                                object_type: obj_type.to_string(),
                            });
                        }
                    }
                }
            }
        }
    }

    ldap.unbind().await.ok();
    (true, CommandResponseData::AdGroupMembers {
        group_name: group_name.to_string(),
        members,
    })
}

/// Enumerate domain computers via LDAP
pub fn enum_computers(filter: Option<String>, search: Option<String>) -> (bool, CommandResponseData) {
    let rt = match tokio::runtime::Handle::try_current() {
        Ok(h) => h,
        Err(_) => return (false, CommandResponseData::Error {
            message: "No async runtime available".to_string()
        }),
    };

    rt.block_on(async {
        enum_computers_async(filter, search).await
    })
}

async fn enum_computers_async(filter: Option<String>, search: Option<String>) -> (bool, CommandResponseData) {
    let (mut ldap, base_dn, _) = match get_ldap_connection().await {
        Ok(conn) => conn,
        Err(e) => return (false, CommandResponseData::Error { message: e }),
    };

    let mut ldap_filter = "(objectClass=computer)".to_string();

    // Build filter
    let mut conditions = vec!["(objectClass=computer)".to_string()];

    if let Some(ref s) = search {
        conditions.push(format!("(|(sAMAccountName=*{}*)(cn=*{}*)(dNSHostName=*{}*))", s, s, s));
    }

    match filter.as_deref() {
        Some("dcs") => conditions.push("(userAccountControl:1.2.840.113556.1.4.803:=8192)".to_string()),
        Some("servers") => conditions.push("(operatingSystem=*Server*)".to_string()),
        Some("workstations") => {
            conditions.push("(!(operatingSystem=*Server*))".to_string());
            conditions.push("(!(userAccountControl:1.2.840.113556.1.4.803:=8192))".to_string());
        }
        _ => {}
    }

    if conditions.len() > 1 {
        ldap_filter = format!("(&{})", conditions.join(""));
    }

    let attrs = vec![
        "sAMAccountName", "dNSHostName", "operatingSystem", "operatingSystemVersion",
        "userAccountControl", "lastLogonTimestamp", "description"
    ];

    let (entries, _) = match ldap.search(&base_dn, Scope::Subtree, &ldap_filter, attrs).await {
        Ok(result) => match result.success() {
            Ok(r) => r,
            Err(e) => {
                ldap.unbind().await.ok();
                return (false, CommandResponseData::Error { message: e.to_string() });
            }
        },
        Err(e) => {
            ldap.unbind().await.ok();
            return (false, CommandResponseData::Error { message: e.to_string() });
        }
    };

    let mut computers = Vec::new();

    for entry in entries {
        let entry = SearchEntry::construct(entry);

        let name = entry.attrs.get("sAMAccountName")
            .and_then(|v| v.first())
            .cloned()
            .unwrap_or_default()
            .trim_end_matches('$')
            .to_string();

        let dns_hostname = entry.attrs.get("dNSHostName")
            .and_then(|v| v.first())
            .cloned();

        let os = entry.attrs.get("operatingSystem")
            .and_then(|v| v.first())
            .cloned();

        let os_version = entry.attrs.get("operatingSystemVersion")
            .and_then(|v| v.first())
            .cloned();

        let uac: u32 = entry.attrs.get("userAccountControl")
            .and_then(|v| v.first())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        // SERVER_TRUST_ACCOUNT = 0x2000 (8192) = Domain Controller
        let is_dc = (uac & 0x2000) != 0;

        let is_server = os.as_ref()
            .map(|s| s.contains("Server"))
            .unwrap_or(false) && !is_dc;

        let last_logon = entry.attrs.get("lastLogonTimestamp")
            .and_then(|v| v.first())
            .and_then(|s| filetime_to_iso(s));

        let description = entry.attrs.get("description")
            .and_then(|v| v.first())
            .cloned();

        // Resolve DNS hostname to IP addresses
        let ip_addresses = if let Some(ref hostname) = dns_hostname {
            resolve_hostname(hostname).await
        } else {
            Vec::new()
        };

        computers.push(AdComputer {
            name,
            dns_hostname,
            os,
            os_version,
            ip_addresses,
            is_dc,
            is_server,
            last_logon,
            description,
        });
    }

    ldap.unbind().await.ok();
    (true, CommandResponseData::AdComputerList { computers })
}

/// Enumerate SPNs via LDAP
pub fn enum_spns(search: Option<String>) -> (bool, CommandResponseData) {
    let rt = match tokio::runtime::Handle::try_current() {
        Ok(h) => h,
        Err(_) => return (false, CommandResponseData::Error {
            message: "No async runtime available".to_string()
        }),
    };

    rt.block_on(async {
        enum_spns_async(search).await
    })
}

async fn enum_spns_async(search: Option<String>) -> (bool, CommandResponseData) {
    let (mut ldap, base_dn, _) = match get_ldap_connection().await {
        Ok(conn) => conn,
        Err(e) => return (false, CommandResponseData::Error { message: e }),
    };

    // Query for objects with SPNs (users and computers)
    let ldap_filter = "(servicePrincipalName=*)";

    let attrs = vec![
        "sAMAccountName", "distinguishedName", "servicePrincipalName", "objectClass"
    ];

    let (entries, _) = match ldap.search(&base_dn, Scope::Subtree, ldap_filter, attrs).await {
        Ok(result) => match result.success() {
            Ok(r) => r,
            Err(e) => {
                ldap.unbind().await.ok();
                return (false, CommandResponseData::Error { message: e.to_string() });
            }
        },
        Err(e) => {
            ldap.unbind().await.ok();
            return (false, CommandResponseData::Error { message: e.to_string() });
        }
    };

    let mut spns = Vec::new();

    for entry in entries {
        let entry = SearchEntry::construct(entry);

        let account_name = entry.attrs.get("sAMAccountName")
            .and_then(|v| v.first())
            .cloned()
            .unwrap_or_default();

        let account_dn = entry.attrs.get("distinguishedName")
            .and_then(|v| v.first())
            .cloned()
            .unwrap_or_default();

        let obj_classes = entry.attrs.get("objectClass")
            .cloned()
            .unwrap_or_default();

        let is_user = obj_classes.iter().any(|c| c == "user") &&
            !obj_classes.iter().any(|c| c == "computer");

        if let Some(spn_values) = entry.attrs.get("servicePrincipalName") {
            for spn in spn_values {
                // Apply search filter
                if let Some(ref s) = search {
                    let s_lower = s.to_lowercase();
                    if !spn.to_lowercase().contains(&s_lower) &&
                       !account_name.to_lowercase().contains(&s_lower) {
                        continue;
                    }
                }

                // Parse SPN
                let parts: Vec<&str> = spn.split('/').collect();
                let service_type = parts.first().unwrap_or(&"").to_string();
                let target_host = if parts.len() > 1 {
                    parts[1].split(':').next().unwrap_or("").to_string()
                } else {
                    String::new()
                };

                spns.push(AdSpnEntry {
                    spn: spn.clone(),
                    account_name: account_name.clone(),
                    account_dn: account_dn.clone(),
                    is_user_account: is_user,
                    service_type,
                    target_host,
                });
            }
        }
    }

    ldap.unbind().await.ok();
    (true, CommandResponseData::AdSpnList { spns })
}

/// Enumerate logged-on sessions (uses WMI - sessions are runtime data, not AD data)
pub fn enum_sessions(_target: Option<String>) -> (bool, CommandResponseData) {
    use wmi::{COMLibrary, WMIConnection, Variant};
    use std::collections::HashMap;

    let com = match COMLibrary::new() {
        Ok(c) => c,
        Err(e) => return (false, CommandResponseData::Error {
            message: format!("COM init failed: {}", e)
        }),
    };

    let wmi = match WMIConnection::new(com) {
        Ok(w) => w,
        Err(e) => return (false, CommandResponseData::Error {
            message: format!("WMI connection failed: {}", e)
        }),
    };

    let mut sessions = Vec::new();

    // Query logon sessions with associated users
    let query = "SELECT * FROM Win32_LogonSession WHERE LogonType = 2 OR LogonType = 10";
    let session_results: Vec<HashMap<String, Variant>> = match wmi.raw_query(query) {
        Ok(r) => r,
        Err(_) => Vec::new(),
    };

    for session in session_results {
        let logon_id = match session.get("LogonId") {
            Some(Variant::String(s)) => s.clone(),
            _ => continue,
        };

        let session_type = match session.get("LogonType") {
            Some(Variant::UI4(2)) => "Interactive",
            Some(Variant::UI4(10)) => "RemoteInteractive",
            _ => "Other",
        };

        let start_time = session.get("StartTime")
            .and_then(|v| if let Variant::String(s) = v { Some(s.clone()) } else { None });

        // Query for associated user via Win32_LoggedOnUser
        let user_query = format!(
            "SELECT * FROM Win32_LoggedOnUser WHERE Dependent = \"Win32_LogonSession.LogonId='{}'\"",
            logon_id
        );

        let user_results: Vec<HashMap<String, Variant>> = match wmi.raw_query(&user_query) {
            Ok(r) => r,
            Err(_) => continue,
        };

        for user_assoc in user_results {
            if let Some(Variant::String(antecedent)) = user_assoc.get("Antecedent") {
                // Parse the antecedent to get username
                // Format: \\.\root\cimv2:Win32_Account.Domain="DOMAIN",Name="username"
                if let Some(name_start) = antecedent.find("Name=\"") {
                    let name_part = &antecedent[name_start + 6..];
                    if let Some(name_end) = name_part.find('"') {
                        let username = &name_part[..name_end];

                        // Get domain
                        let domain = if let Some(dom_start) = antecedent.find("Domain=\"") {
                            let dom_part = &antecedent[dom_start + 8..];
                            dom_part.split('"').next().unwrap_or("")
                        } else {
                            ""
                        };

                        let full_username = if domain.is_empty() {
                            username.to_string()
                        } else {
                            format!("{}\\{}", domain, username)
                        };

                        sessions.push(AdSession {
                            username: full_username,
                            computer: hostname::get()
                                .map(|h| h.to_string_lossy().to_string())
                                .unwrap_or_else(|_| "localhost".to_string()),
                            session_id: logon_id.parse().unwrap_or(0),
                            session_type: session_type.to_string(),
                            logon_time: start_time.clone(),
                        });
                    }
                }
            }
        }
    }

    (true, CommandResponseData::AdSessionList { sessions })
}

/// Enumerate domain trusts (uses Windows API - most reliable for trusts)
pub fn enum_trusts() -> (bool, CommandResponseData) {
    unsafe {
        let mut trusts = Vec::new();
        let mut domain_trusts: *mut DS_DOMAIN_TRUSTSW = null_mut();
        let mut domain_count: u32 = 0;

        // Flags: DS_DOMAIN_IN_FOREST | DS_DOMAIN_DIRECT_OUTBOUND | DS_DOMAIN_DIRECT_INBOUND
        let flags = 0x23u32;

        let result = DsEnumerateDomainTrustsW(
            PCWSTR::null(),
            flags,
            &mut domain_trusts,
            &mut domain_count,
        );

        if result == 0 && !domain_trusts.is_null() {
            for i in 0..domain_count as isize {
                let trust = &*domain_trusts.offset(i);
                let target = wide_to_string(trust.DnsDomainName.0);

                let flags_raw = trust.Flags;

                // Skip current domain (DS_DOMAIN_PRIMARY = 0x00000008)
                if (flags_raw & 0x00000008) != 0 {
                    continue;
                }

                let is_outbound = (flags_raw & 0x00000002) != 0;
                let is_inbound = (flags_raw & 0x00000020) != 0;

                let direction = if is_outbound && is_inbound {
                    "Bidirectional"
                } else if is_outbound {
                    "Outbound"
                } else {
                    "Inbound"
                };

                let is_tree_root = (flags_raw & 0x00000004) != 0;
                let in_forest = (flags_raw & 0x00000001) != 0;

                let trust_type = if is_tree_root {
                    "TreeRoot"
                } else if in_forest {
                    "ParentChild"
                } else {
                    "External"
                };

                // Query SID filtering status via LDAP TDO attributes
                let sid_filtering = query_sid_filtering(&target);

                trusts.push(AdTrust {
                    target_domain: target,
                    direction: direction.to_string(),
                    trust_type: trust_type.to_string(),
                    is_transitive: in_forest,
                    sid_filtering,
                });
            }

            let _ = NetApiBufferFree(Some(domain_trusts as *const _));
        }

        (true, CommandResponseData::AdTrustList { trusts })
    }
}

/// Resolve hostname to IP addresses using tokio DNS
async fn resolve_hostname(hostname: &str) -> Vec<String> {
    // Add port to make it a valid socket address for lookup
    let lookup_addr = format!("{}:0", hostname);

    let result = match tokio::net::lookup_host(&lookup_addr).await {
        Ok(addrs) => addrs
            .map(|addr| addr.ip().to_string())
            .collect::<std::collections::HashSet<_>>() // Dedupe
            .into_iter()
            .collect(),
        Err(_) => Vec::new(),
    };
    result
}

/// Query SID filtering status for a trust via TDO trustAttributes
fn query_sid_filtering(target_domain: &str) -> bool {
    // Query the Trusted Domain Object (TDO) for trustAttributes
    // TDO is at: CN=<trust_name>,CN=System,<domain_dn>
    // TRUST_ATTRIBUTE_QUARANTINED_DOMAIN (0x4) = SID filtering enabled

    let rt = match tokio::runtime::Handle::try_current() {
        Ok(h) => h,
        Err(_) => return true, // Default to enabled (safer assumption)
    };

    rt.block_on(async {
        query_sid_filtering_async(target_domain).await
    })
}

async fn query_sid_filtering_async(target_domain: &str) -> bool {
    let (mut ldap, base_dn, _) = match get_ldap_connection().await {
        Ok(conn) => conn,
        Err(_) => return true, // Default to enabled if can't query
    };

    // TDO is stored in CN=System container
    // The CN is typically the NetBIOS name of the trusted domain
    let netbios_name = target_domain.split('.').next().unwrap_or(target_domain);
    let system_dn = format!("CN=System,{}", base_dn);

    // Query for the TDO
    let filter = format!("(&(objectClass=trustedDomain)(cn={}))", netbios_name);

    let result = ldap.search(
        &system_dn,
        Scope::OneLevel,
        &filter,
        vec!["trustAttributes"]
    ).await;

    let sid_filtering = match result {
        Ok(res) => {
            match res.success() {
                Ok((entries, _)) => {
                    if let Some(entry) = entries.into_iter().next() {
                        let entry = SearchEntry::construct(entry);
                        let trust_attrs: u32 = entry.attrs.get("trustAttributes")
                            .and_then(|v| v.first())
                            .and_then(|s| s.parse().ok())
                            .unwrap_or(0);

                        // TRUST_ATTRIBUTE_QUARANTINED_DOMAIN = 0x4
                        (trust_attrs & 0x4) != 0
                    } else {
                        true // TDO not found, assume enabled
                    }
                }
                Err(_) => true,
            }
        }
        Err(_) => true,
    };

    ldap.unbind().await.ok();
    sid_filtering
}

// ============================================================================
// ACL Enumeration
// ============================================================================

/// Well-known rights GUIDs for AD extended rights
mod acl_guids {
    pub const RESET_PASSWORD: &str = "00299570-246d-11d0-a768-00aa006e0529";
    pub const CHANGE_PASSWORD: &str = "ab721a53-1e2f-11d0-9819-00aa0040529b";
    pub const WRITE_MEMBERS: &str = "bf9679c0-0de6-11d0-a285-00aa003049e2";
    pub const DS_REPLICATION_GET_CHANGES: &str = "1131f6aa-9c07-11d1-f79f-00c04fc2dcd2";
    pub const DS_REPLICATION_GET_CHANGES_ALL: &str = "1131f6ad-9c07-11d1-f79f-00c04fc2dcd2";
    pub const USER_FORCE_CHANGE_PASSWORD: &str = "00299570-246d-11d0-a768-00aa006e0529";
    pub const SELF_MEMBERSHIP: &str = "bf9679c0-0de6-11d0-a285-00aa003049e2";
}

/// AD rights bit masks
mod ad_rights {
    pub const GENERIC_ALL: u32 = 0x10000000;
    pub const GENERIC_WRITE: u32 = 0x40000000;
    pub const WRITE_DACL: u32 = 0x00040000;
    pub const WRITE_OWNER: u32 = 0x00080000;
    pub const WRITE_PROPERTY: u32 = 0x00000020;
    pub const SELF: u32 = 0x00000008;
    pub const EXTENDED_RIGHT: u32 = 0x00000100;
    pub const DELETE: u32 = 0x00010000;
    pub const DELETE_TREE: u32 = 0x00000040;
    pub const CREATE_CHILD: u32 = 0x00000001;
    pub const DELETE_CHILD: u32 = 0x00000002;
}

/// Enumerate ACLs on AD objects
pub fn enum_acls(object_type: Option<String>, target_dn: Option<String>) -> (bool, CommandResponseData) {
    let rt = match tokio::runtime::Handle::try_current() {
        Ok(h) => h,
        Err(_) => return (false, CommandResponseData::Error {
            message: "No async runtime available".to_string()
        }),
    };

    rt.block_on(async {
        enum_acls_async(object_type, target_dn).await
    })
}

async fn enum_acls_async(object_type: Option<String>, target_dn: Option<String>) -> (bool, CommandResponseData) {
    let (mut ldap, base_dn, _) = match get_ldap_connection().await {
        Ok(conn) => conn,
        Err(e) => return (false, CommandResponseData::Error { message: e }),
    };

    let mut acls = Vec::new();

    // Build filter based on object type
    let filter = if let Some(ref dn) = target_dn {
        // Query specific object
        format!("(distinguishedName={})", dn)
    } else {
        match object_type.as_deref() {
            Some("users") => "(&(objectClass=user)(objectCategory=person)(adminCount=1))".to_string(),
            Some("groups") => "(&(objectClass=group)(|(cn=Domain Admins)(cn=Enterprise Admins)(cn=Administrators)(cn=Account Operators)(cn=Backup Operators)(adminCount=1)))".to_string(),
            Some("computers") => "(&(objectClass=computer)(userAccountControl:1.2.840.113556.1.4.803:=8192))".to_string(), // DCs only
            Some("ous") => "(objectClass=organizationalUnit)".to_string(),
            Some("gpos") => "(objectClass=groupPolicyContainer)".to_string(),
            _ => {
                // Default: query high-value targets (admin users/groups)
                "(|(& (objectClass=user)(adminCount=1))(&(objectClass=group)(adminCount=1)))".to_string()
            }
        }
    };

    let attrs = vec!["distinguishedName", "sAMAccountName", "cn", "objectClass", "nTSecurityDescriptor"];

    // Note: nTSecurityDescriptor requires special handling - ldap3 returns it as binary
    let search_result = ldap.search(&base_dn, Scope::Subtree, &filter, attrs).await;

    let entries = match search_result {
        Ok(result) => match result.success() {
            Ok((entries, _)) => entries,
            Err(e) => {
                ldap.unbind().await.ok();
                return (false, CommandResponseData::Error {
                    message: format!("LDAP search failed: {}", e)
                });
            }
        },
        Err(e) => {
            ldap.unbind().await.ok();
            return (false, CommandResponseData::Error {
                message: format!("LDAP search failed: {}", e)
            });
        }
    };

    for entry in entries {
        let entry = SearchEntry::construct(entry);

        let dn = entry.attrs.get("distinguishedName")
            .and_then(|v| v.first())
            .cloned()
            .unwrap_or_default();

        let name = entry.attrs.get("sAMAccountName")
            .or_else(|| entry.attrs.get("cn"))
            .and_then(|v| v.first())
            .cloned()
            .unwrap_or_default();

        let obj_classes = entry.attrs.get("objectClass")
            .cloned()
            .unwrap_or_default();

        let obj_type = if obj_classes.iter().any(|c| c == "user") && !obj_classes.iter().any(|c| c == "computer") {
            "user"
        } else if obj_classes.iter().any(|c| c == "group") {
            "group"
        } else if obj_classes.iter().any(|c| c == "computer") {
            "computer"
        } else if obj_classes.iter().any(|c| c == "organizationalUnit") {
            "ou"
        } else if obj_classes.iter().any(|c| c == "groupPolicyContainer") {
            "gpo"
        } else {
            "unknown"
        };

        // Get nTSecurityDescriptor from binary attributes
        let sd_binary = entry.bin_attrs.get("nTSecurityDescriptor")
            .and_then(|v| v.first())
            .cloned();

        if let Some(sd_data) = sd_binary {
            // Parse security descriptor
            let aces = parse_security_descriptor(&sd_data);

            if !aces.is_empty() {
                acls.push(AdObjectAcl {
                    object_dn: dn,
                    object_type: obj_type.to_string(),
                    object_name: name,
                    aces,
                });
            }
        }
    }

    ldap.unbind().await.ok();
    (true, CommandResponseData::AdAclList { acls })
}

/// Parse a binary security descriptor and extract interesting ACEs
fn parse_security_descriptor(sd_data: &[u8]) -> Vec<AdAce> {
    use windows::Win32::Security::{
        GetSecurityDescriptorDacl, GetAce, IsValidSecurityDescriptor,
        ACL, ACE_HEADER, ACCESS_ALLOWED_ACE, ACCESS_ALLOWED_OBJECT_ACE,
        PSID, PSECURITY_DESCRIPTOR,
    };
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows::Win32::Foundation::{LocalFree, HLOCAL};

    let mut aces = Vec::new();

    unsafe {
        // Create a SECURITY_DESCRIPTOR from raw bytes
        let sd_ptr = PSECURITY_DESCRIPTOR(sd_data.as_ptr() as *mut _);

        // Validate the security descriptor
        if !IsValidSecurityDescriptor(sd_ptr).as_bool() {
            return aces;
        }

        // Get the DACL
        let mut dacl_present = windows::core::BOOL::default();
        let mut dacl: *mut ACL = null_mut();
        let mut dacl_defaulted = windows::core::BOOL::default();

        if GetSecurityDescriptorDacl(sd_ptr, &mut dacl_present, &mut dacl, &mut dacl_defaulted).is_err() {
            return aces;
        }

        if !dacl_present.as_bool() || dacl.is_null() {
            return aces;
        }

        let dacl_ref = &*dacl;
        let ace_count = dacl_ref.AceCount as u32;

        for i in 0..ace_count {
            let mut ace_ptr: *mut std::ffi::c_void = null_mut();
            if GetAce(dacl, i, &mut ace_ptr).is_err() {
                continue;
            }

            let ace_header = ace_ptr as *const ACE_HEADER;
            let ace_type = (*ace_header).AceType;
            let ace_flags = (*ace_header).AceFlags;

            // We're interested in:
            // 0x00 = ACCESS_ALLOWED_ACE_TYPE
            // 0x05 = ACCESS_ALLOWED_OBJECT_ACE_TYPE
            // Skip denied ACEs for now (focus on allowed)

            let (sid, access_mask, object_type_guid, inherited_object_type_guid) = if ace_type == 0x00 {
                // ACCESS_ALLOWED_ACE
                let ace = ace_ptr as *const ACCESS_ALLOWED_ACE;
                let sid = PSID(&(*ace).SidStart as *const u32 as *mut _);
                let mask = (*ace).Mask;
                (sid, mask, None, None)
            } else if ace_type == 0x05 {
                // ACCESS_ALLOWED_OBJECT_ACE
                let ace = ace_ptr as *const ACCESS_ALLOWED_OBJECT_ACE;
                let flags = (*ace).Flags;
                let mask = (*ace).Mask;

                // Object ACE has variable layout based on flags
                let mut offset = 0usize;
                let mut obj_guid: Option<String> = None;
                let mut inh_obj_guid: Option<String> = None;

                // ACE_OBJECT_TYPE_PRESENT = 0x1
                if (flags.0 & 0x1) != 0 {
                    obj_guid = Some(guid_to_string(&(*ace).ObjectType));
                    offset += 16;
                }
                // ACE_INHERITED_OBJECT_TYPE_PRESENT = 0x2
                if (flags.0 & 0x2) != 0 {
                    inh_obj_guid = Some(guid_to_string(&(*ace).InheritedObjectType));
                    offset += 16;
                }

                // SID starts after the GUIDs
                let sid_start = (&(*ace).ObjectType as *const _ as *const u8).add(offset);
                let sid = PSID(sid_start as *mut _);

                (sid, mask, obj_guid, inh_obj_guid)
            } else {
                continue;
            };

            // Convert access mask to readable right
            let right = mask_to_right(access_mask, &object_type_guid);

            // Skip if not an interesting right
            if right.is_empty() {
                continue;
            }

            // Convert SID to string
            let mut sid_string_ptr: windows::core::PWSTR = windows::core::PWSTR::null();
            let sid_string = if ConvertSidToStringSidW(sid, &mut sid_string_ptr).is_ok() {
                let s = wide_to_string(sid_string_ptr.0);
                let _ = LocalFree(Some(HLOCAL(sid_string_ptr.0 as *mut _)));
                s
            } else {
                continue;
            };

            // Resolve SID to name
            let principal = resolve_sid_to_name(sid);

            // Skip well-known/expected principals
            if should_skip_principal(&principal, &sid_string) {
                continue;
            }

            // Is inherited?
            let inherited = (ace_flags & 0x10) != 0; // INHERITED_ACE

            aces.push(AdAce {
                principal,
                principal_sid: sid_string,
                right,
                inherited,
                access_type: "Allow".to_string(),
                object_type_guid,
                inherited_object_type_guid,
            });
        }
    }

    aces
}

/// Convert GUID to string
fn guid_to_string(guid: &windows::core::GUID) -> String {
    format!("{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        guid.data1, guid.data2, guid.data3,
        guid.data4[0], guid.data4[1], guid.data4[2], guid.data4[3],
        guid.data4[4], guid.data4[5], guid.data4[6], guid.data4[7])
}

/// Convert access mask to human-readable right name
fn mask_to_right(mask: u32, object_type_guid: &Option<String>) -> String {
    // Check for full control first
    if (mask & ad_rights::GENERIC_ALL) != 0 {
        return "GenericAll".to_string();
    }

    // Check for other interesting rights
    if (mask & ad_rights::WRITE_DACL) != 0 {
        return "WriteDACL".to_string();
    }
    if (mask & ad_rights::WRITE_OWNER) != 0 {
        return "WriteOwner".to_string();
    }
    if (mask & ad_rights::GENERIC_WRITE) != 0 {
        return "GenericWrite".to_string();
    }

    // Extended rights with object type GUID
    if (mask & ad_rights::EXTENDED_RIGHT) != 0 {
        if let Some(ref guid) = object_type_guid {
            let guid_lower = guid.to_lowercase();
            if guid_lower == acl_guids::RESET_PASSWORD {
                return "ForceChangePassword".to_string();
            }
            if guid_lower == acl_guids::DS_REPLICATION_GET_CHANGES {
                return "DS-Replication-Get-Changes".to_string();
            }
            if guid_lower == acl_guids::DS_REPLICATION_GET_CHANGES_ALL {
                return "DS-Replication-Get-Changes-All".to_string();
            }
            return format!("ExtendedRight({})", guid);
        }
        return "ExtendedRight".to_string();
    }

    // Write property
    if (mask & ad_rights::WRITE_PROPERTY) != 0 {
        if let Some(ref guid) = object_type_guid {
            let guid_lower = guid.to_lowercase();
            if guid_lower == acl_guids::WRITE_MEMBERS {
                return "WriteMembers".to_string();
            }
            return format!("WriteProperty({})", guid);
        }
        return "WriteProperty".to_string();
    }

    // Self (e.g., self-membership)
    if (mask & ad_rights::SELF) != 0 {
        if let Some(ref guid) = object_type_guid {
            let guid_lower = guid.to_lowercase();
            if guid_lower == acl_guids::SELF_MEMBERSHIP {
                return "Self-Membership".to_string();
            }
        }
        return "Self".to_string();
    }

    // Not an interesting right for our purposes
    String::new()
}

/// Resolve SID to account name
fn resolve_sid_to_name(sid: windows::Win32::Security::PSID) -> String {
    use windows::Win32::Security::{LookupAccountSidW, SID_NAME_USE};

    unsafe {
        let mut name_size: u32 = 256;
        let mut domain_size: u32 = 256;
        let mut name_buf = vec![0u16; name_size as usize];
        let mut domain_buf = vec![0u16; domain_size as usize];
        let mut sid_type = SID_NAME_USE::default();

        let result = LookupAccountSidW(
            windows::core::PCWSTR::null(),
            sid,
            Some(windows::core::PWSTR::from_raw(name_buf.as_mut_ptr())),
            &mut name_size,
            Some(windows::core::PWSTR::from_raw(domain_buf.as_mut_ptr())),
            &mut domain_size,
            &mut sid_type,
        );

        if result.is_ok() {
            let name = wide_to_string(name_buf.as_ptr());
            let domain = wide_to_string(domain_buf.as_ptr());

            if domain.is_empty() {
                name
            } else {
                format!("{}\\{}", domain, name)
            }
        } else {
            // Return SID string if lookup fails
            String::new()
        }
    }
}

/// Check if principal should be skipped (expected/well-known)
fn should_skip_principal(principal: &str, sid: &str) -> bool {
    let principal_lower = principal.to_lowercase();

    // Skip SYSTEM, Domain Admins, Enterprise Admins (they're expected to have rights)
    if principal_lower.contains("nt authority\\system") ||
       principal_lower.contains("\\domain admins") ||
       principal_lower.contains("\\enterprise admins") ||
       principal_lower.contains("\\administrators") ||
       principal_lower == "system" {
        return true;
    }

    // Skip well-known SIDs
    // S-1-5-18 = SYSTEM
    // S-1-5-32-544 = Administrators
    if sid == "S-1-5-18" || sid == "S-1-5-32-544" {
        return true;
    }

    // Skip if principal couldn't be resolved (empty)
    if principal.is_empty() {
        return true;
    }

    false
}
