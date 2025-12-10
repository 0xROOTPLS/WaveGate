//! WMI Query execution module.
//!
//! Provides ability to execute arbitrary WMI queries and return results.
//! Runs queries on a separate thread with a 30 second timeout.

use wavegate_shared::CommandResponseData;
use wmi::{COMLibrary, WMIConnection, Variant};
use std::collections::HashMap;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// Query timeout in seconds
const WMI_QUERY_TIMEOUT_SECS: u64 = 30;

/// Execute a WMI query and return results (with timeout)
pub fn execute_query(query: &str, namespace: Option<&str>) -> (bool, CommandResponseData) {
    let query = query.to_string();
    let namespace = namespace.map(|s| s.to_string());

    // Create channel for result
    let (tx, rx) = mpsc::channel();

    // Spawn thread to run the query
    thread::spawn(move || {
        let result = execute_query_inner(&query, namespace.as_deref());
        let _ = tx.send(result);
    });

    // Wait for result with timeout
    match rx.recv_timeout(Duration::from_secs(WMI_QUERY_TIMEOUT_SECS)) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            (false, CommandResponseData::Error {
                message: format!("WMI query timed out after {} seconds", WMI_QUERY_TIMEOUT_SECS)
            })
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            (false, CommandResponseData::Error {
                message: "WMI query thread terminated unexpectedly".to_string()
            })
        }
    }
}

/// Inner function that actually executes the WMI query
fn execute_query_inner(query: &str, namespace: Option<&str>) -> (bool, CommandResponseData) {
    let com = match COMLibrary::new() {
        Ok(c) => c,
        Err(e) => {
            return (false, CommandResponseData::Error {
                message: format!("Failed to initialize COM: {}", e)
            });
        }
    };

    // Default namespace is root\cimv2
    let ns = namespace.unwrap_or("root\\cimv2");

    let wmi_con = match WMIConnection::with_namespace_path(ns, com) {
        Ok(c) => c,
        Err(e) => {
            return (false, CommandResponseData::Error {
                message: format!("Failed to connect to WMI namespace '{}': {}", ns, e)
            });
        }
    };

    // Execute raw query and get results as HashMap
    let results: Vec<HashMap<String, Variant>> = match wmi_con.raw_query(query) {
        Ok(r) => r,
        Err(e) => {
            return (false, CommandResponseData::Error {
                message: format!("WMI query failed: {}", e)
            });
        }
    };

    if results.is_empty() {
        return (true, CommandResponseData::WmiQueryResult {
            columns: vec![],
            rows: vec![],
        });
    }

    // Extract column names from first result
    let columns: Vec<String> = results[0].keys().cloned().collect();

    // Convert results to string rows
    let rows: Vec<Vec<String>> = results.iter().map(|row| {
        columns.iter().map(|col| {
            match row.get(col) {
                Some(variant) => variant_to_string(variant),
                None => String::new(),
            }
        }).collect()
    }).collect();

    (true, CommandResponseData::WmiQueryResult { columns, rows })
}

/// Convert WMI Variant to string representation
fn variant_to_string(v: &Variant) -> String {
    match v {
        Variant::Null => "NULL".to_string(),
        Variant::Empty => "".to_string(),
        Variant::String(s) => s.clone(),
        Variant::I1(n) => n.to_string(),
        Variant::I2(n) => n.to_string(),
        Variant::I4(n) => n.to_string(),
        Variant::I8(n) => n.to_string(),
        Variant::UI1(n) => n.to_string(),
        Variant::UI2(n) => n.to_string(),
        Variant::UI4(n) => n.to_string(),
        Variant::UI8(n) => n.to_string(),
        Variant::R4(n) => n.to_string(),
        Variant::R8(n) => n.to_string(),
        Variant::Bool(b) => b.to_string(),
        Variant::Array(arr) => {
            let items: Vec<String> = arr.iter().map(|item| variant_to_string(item)).collect();
            format!("[{}]", items.join(", "))
        }
        Variant::Object(_) => "[Object]".to_string(),
        Variant::Unknown(_) => "[Unknown]".to_string(),
    }
}
