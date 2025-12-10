//! DNS/Hosts file management for client-side execution.

use wavegate_shared::{CommandResponseData, HostsEntry};
use std::fs;
use std::io::{BufRead, BufReader, Write};

/// Get the hosts file path
fn get_hosts_path() -> &'static str {
    r"C:\Windows\System32\drivers\etc\hosts"
}

/// Parse a single line from the hosts file
fn parse_hosts_line(line: &str) -> Option<HostsEntry> {
    let trimmed = line.trim();

    // Skip empty lines
    if trimmed.is_empty() {
        return None;
    }

    // Handle comments
    let (content, comment) = if let Some(idx) = trimmed.find('#') {
        let content = trimmed[..idx].trim();
        let comment = trimmed[idx + 1..].trim();
        (content, if comment.is_empty() { None } else { Some(comment.to_string()) })
    } else {
        (trimmed, None)
    };

    // Skip pure comment lines
    if content.is_empty() {
        return None;
    }

    // Split into parts (IP and hostnames)
    let parts: Vec<&str> = content.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }

    let ip = parts[0].to_string();
    let hostname = parts[1].to_string();

    Some(HostsEntry { ip, hostname, comment })
}

/// Get all entries from the hosts file
pub fn get_hosts_entries() -> (bool, CommandResponseData) {
    let hosts_path = get_hosts_path();

    let file = match fs::File::open(hosts_path) {
        Ok(f) => f,
        Err(e) => {
            return (false, CommandResponseData::Error {
                message: format!("Failed to open hosts file: {}", e),
            });
        }
    };

    let reader = BufReader::new(file);
    let mut entries = Vec::new();

    for line in reader.lines() {
        if let Ok(line) = line {
            if let Some(entry) = parse_hosts_line(&line) {
                entries.push(entry);
            }
        }
    }

    (true, CommandResponseData::HostsEntries { entries })
}

/// Add a new entry to the hosts file
pub fn add_hosts_entry(hostname: &str, ip: &str) -> (bool, CommandResponseData) {
    let hosts_path = get_hosts_path();

    // Read existing content
    let content = match fs::read_to_string(hosts_path) {
        Ok(c) => c,
        Err(e) => {
            return (false, CommandResponseData::HostsResult {
                success: false,
                message: format!("Failed to read hosts file: {}", e),
            });
        }
    };

    // Check if entry already exists
    for line in content.lines() {
        if let Some(entry) = parse_hosts_line(line) {
            if entry.hostname.eq_ignore_ascii_case(hostname) {
                return (false, CommandResponseData::HostsResult {
                    success: false,
                    message: format!("Entry for '{}' already exists", hostname),
                });
            }
        }
    }

    // Append new entry
    let new_entry = format!("{}\t{}\n", ip, hostname);
    let mut file = match fs::OpenOptions::new()
        .append(true)
        .open(hosts_path)
    {
        Ok(f) => f,
        Err(e) => {
            return (false, CommandResponseData::HostsResult {
                success: false,
                message: format!("Failed to open hosts file for writing: {}", e),
            });
        }
    };

    if let Err(e) = file.write_all(new_entry.as_bytes()) {
        return (false, CommandResponseData::HostsResult {
            success: false,
            message: format!("Failed to write to hosts file: {}", e),
        });
    }

    (true, CommandResponseData::HostsResult {
        success: true,
        message: format!("Added entry: {} -> {}", hostname, ip),
    })
}

/// Remove an entry from the hosts file
pub fn remove_hosts_entry(hostname: &str) -> (bool, CommandResponseData) {
    let hosts_path = get_hosts_path();

    // Read existing content
    let content = match fs::read_to_string(hosts_path) {
        Ok(c) => c,
        Err(e) => {
            return (false, CommandResponseData::HostsResult {
                success: false,
                message: format!("Failed to read hosts file: {}", e),
            });
        }
    };

    let mut new_lines = Vec::new();
    let mut found = false;

    for line in content.lines() {
        let should_keep = if let Some(entry) = parse_hosts_line(line) {
            if entry.hostname.eq_ignore_ascii_case(hostname) {
                found = true;
                false
            } else {
                true
            }
        } else {
            // Keep comments and empty lines
            true
        };

        if should_keep {
            new_lines.push(line);
        }
    }

    if !found {
        return (false, CommandResponseData::HostsResult {
            success: false,
            message: format!("Entry for '{}' not found", hostname),
        });
    }

    // Write back
    let new_content = new_lines.join("\n") + "\n";
    if let Err(e) = fs::write(hosts_path, new_content) {
        return (false, CommandResponseData::HostsResult {
            success: false,
            message: format!("Failed to write hosts file: {}", e),
        });
    }

    (true, CommandResponseData::HostsResult {
        success: true,
        message: format!("Removed entry: {}", hostname),
    })
}
