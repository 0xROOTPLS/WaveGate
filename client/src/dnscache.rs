//! DNS Cache viewing and manipulation module.
//!
//! Provides ability to view, flush, and manipulate DNS cache.

use wavegate_shared::{CommandResponseData, DnsCacheEntry};
use std::process::Command;

/// Get DNS cache entries using ipconfig /displaydns
pub fn get_dns_cache() -> (bool, CommandResponseData) {
    let output = match Command::new("ipconfig")
        .args(["/displaydns"])
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            return (false, CommandResponseData::Error {
                message: format!("Failed to execute ipconfig: {}", e)
            });
        }
    };

    if !output.status.success() {
        return (false, CommandResponseData::Error {
            message: "ipconfig /displaydns failed".to_string()
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let entries = parse_dns_cache(&stdout);

    (true, CommandResponseData::DnsCacheEntries { entries })
}

/// Parse ipconfig /displaydns output
fn parse_dns_cache(output: &str) -> Vec<DnsCacheEntry> {
    let mut entries = Vec::new();
    let mut current_name = String::new();
    let mut current_type = String::new();
    let mut current_ttl: u32 = 0;
    let mut current_data = String::new();
    let mut in_record = false;

    for line in output.lines() {
        let line = line.trim();

        // Record Name line indicates start of new entry
        if line.starts_with("Record Name") {
            // Save previous entry if exists
            if in_record && !current_name.is_empty() {
                entries.push(DnsCacheEntry {
                    name: current_name.clone(),
                    record_type: current_type.clone(),
                    data: current_data.clone(),
                    ttl: current_ttl,
                });
            }

            // Parse new record name
            if let Some(name) = line.split(':').nth(1) {
                current_name = name.trim().to_string();
                current_type.clear();
                current_data.clear();
                current_ttl = 0;
                in_record = true;
            }
        } else if line.starts_with("Record Type") {
            if let Some(type_str) = line.split(':').nth(1) {
                // Convert numeric type to string
                let type_num: u32 = type_str.trim().parse().unwrap_or(0);
                current_type = match type_num {
                    1 => "A".to_string(),
                    5 => "CNAME".to_string(),
                    28 => "AAAA".to_string(),
                    12 => "PTR".to_string(),
                    15 => "MX".to_string(),
                    16 => "TXT".to_string(),
                    2 => "NS".to_string(),
                    6 => "SOA".to_string(),
                    33 => "SRV".to_string(),
                    _ => format!("Type{}", type_num),
                };
            }
        } else if line.starts_with("Time To Live") {
            if let Some(ttl_str) = line.split(':').nth(1) {
                current_ttl = ttl_str.trim().parse().unwrap_or(0);
            }
        } else if line.starts_with("Data Length") {
            // Skip data length
        } else if line.starts_with("Section") {
            // Skip section
        } else if line.starts_with("A (Host) Record") || line.starts_with("AAAA Record") {
            if let Some(data) = line.split(':').nth(1) {
                current_data = data.trim().to_string();
            }
        } else if line.starts_with("CNAME Record") {
            if let Some(data) = line.split(':').nth(1) {
                current_data = data.trim().to_string();
            }
        } else if line.starts_with("PTR Record") {
            if let Some(data) = line.split(':').nth(1) {
                current_data = data.trim().to_string();
            }
        } else if line.contains("Record") && line.contains(":") && current_data.is_empty() {
            // Generic record data
            if let Some(data) = line.split(':').nth(1) {
                current_data = data.trim().to_string();
            }
        }
    }

    // Don't forget the last entry
    if in_record && !current_name.is_empty() {
        entries.push(DnsCacheEntry {
            name: current_name,
            record_type: current_type,
            data: current_data,
            ttl: current_ttl,
        });
    }

    entries
}

/// Flush DNS cache using ipconfig /flushdns
pub fn flush_dns_cache() -> (bool, CommandResponseData) {
    let output = match Command::new("ipconfig")
        .args(["/flushdns"])
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            return (false, CommandResponseData::DnsCacheResult {
                success: false,
                message: format!("Failed to execute ipconfig: {}", e)
            });
        }
    };

    if output.status.success() {
        (true, CommandResponseData::DnsCacheResult {
            success: true,
            message: "DNS cache flushed successfully".to_string()
        })
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        (false, CommandResponseData::DnsCacheResult {
            success: false,
            message: format!("Failed to flush DNS cache: {}", stderr)
        })
    }
}

/// Add a DNS entry by modifying the hosts file
/// This is a persistent way to "poison" DNS resolution for specific hostnames
pub fn add_dns_entry(hostname: &str, ip: &str) -> (bool, CommandResponseData) {
    // Validate IP address format
    if ip.parse::<std::net::IpAddr>().is_err() {
        return (false, CommandResponseData::DnsCacheResult {
            success: false,
            message: format!("Invalid IP address: {}", ip)
        });
    }

    // Read current hosts file
    let hosts_path = r"C:\Windows\System32\drivers\etc\hosts";
    let content = match std::fs::read_to_string(hosts_path) {
        Ok(c) => c,
        Err(e) => {
            return (false, CommandResponseData::DnsCacheResult {
                success: false,
                message: format!("Failed to read hosts file: {}", e)
            });
        }
    };

    // Check if entry already exists
    let entry_line = format!("{}\t{}", ip, hostname);
    for line in content.lines() {
        let line_trimmed = line.trim();
        if line_trimmed.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line_trimmed.split_whitespace().collect();
        if parts.len() >= 2 && parts[1].eq_ignore_ascii_case(hostname) {
            // Update existing entry
            let new_content = content.replace(line, &entry_line);
            if let Err(e) = std::fs::write(hosts_path, new_content) {
                return (false, CommandResponseData::DnsCacheResult {
                    success: false,
                    message: format!("Failed to write hosts file: {}", e)
                });
            }
            // Flush DNS to apply changes
            let _ = flush_dns_cache();
            return (true, CommandResponseData::DnsCacheResult {
                success: true,
                message: format!("Updated hosts entry: {} -> {}", hostname, ip)
            });
        }
    }

    // Append new entry
    let new_content = if content.ends_with('\n') {
        format!("{}{}\n", content, entry_line)
    } else {
        format!("{}\n{}\n", content, entry_line)
    };

    if let Err(e) = std::fs::write(hosts_path, new_content) {
        return (false, CommandResponseData::DnsCacheResult {
            success: false,
            message: format!("Failed to write hosts file: {}", e)
        });
    }

    // Flush DNS to apply changes
    let _ = flush_dns_cache();

    (true, CommandResponseData::DnsCacheResult {
        success: true,
        message: format!("Added hosts entry: {} -> {}", hostname, ip)
    })
}
