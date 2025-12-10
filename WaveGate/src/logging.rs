//! Server event logging system.
//!
//! Provides a centralized logging system that stores events in memory
//! and can be queried by the frontend.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

/// Maximum number of log entries to keep in memory
const MAX_LOG_ENTRIES: usize = 1000;

/// Log entry severity level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Info,
    Success,
    Warning,
    Error,
}

/// A single log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    /// Unix timestamp in milliseconds
    pub timestamp: u64,
    /// Log level
    pub level: LogLevel,
    /// Log message
    pub message: String,
    /// Optional associated client UID
    pub client_uid: Option<String>,
}

impl LogEntry {
    pub fn new(level: LogLevel, message: impl Into<String>) -> Self {
        Self {
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
            level,
            message: message.into(),
            client_uid: None,
        }
    }

    pub fn with_client(mut self, uid: impl Into<String>) -> Self {
        self.client_uid = Some(uid.into());
        self
    }
}

/// Thread-safe log storage
#[derive(Debug)]
pub struct LogStore {
    entries: RwLock<VecDeque<LogEntry>>,
}

impl LogStore {
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(VecDeque::with_capacity(MAX_LOG_ENTRIES)),
        }
    }

    /// Add a log entry
    pub fn log(&self, entry: LogEntry) {
        let mut entries = self.entries.write();

        // Remove oldest entry if at capacity
        if entries.len() >= MAX_LOG_ENTRIES {
            entries.pop_front();
        }

        // Print to console as well
        let time = chrono_lite_format(entry.timestamp);
        println!("[{}] {:?}: {}", time, entry.level, entry.message);

        entries.push_back(entry);
    }

    /// Log an info message
    pub fn info(&self, message: impl Into<String>) {
        self.log(LogEntry::new(LogLevel::Info, message));
    }

    /// Log a success message
    pub fn success(&self, message: impl Into<String>) {
        self.log(LogEntry::new(LogLevel::Success, message));
    }

    /// Log a warning message
    pub fn warning(&self, message: impl Into<String>) {
        self.log(LogEntry::new(LogLevel::Warning, message));
    }

    /// Log an error message
    pub fn error(&self, message: impl Into<String>) {
        self.log(LogEntry::new(LogLevel::Error, message));
    }

    /// Log a client-related info message
    pub fn client_info(&self, uid: &str, message: impl Into<String>) {
        self.log(LogEntry::new(LogLevel::Info, message).with_client(uid));
    }

    /// Log a client-related success message
    pub fn client_success(&self, uid: &str, message: impl Into<String>) {
        self.log(LogEntry::new(LogLevel::Success, message).with_client(uid));
    }

    /// Log a client-related warning message
    pub fn client_warning(&self, uid: &str, message: impl Into<String>) {
        self.log(LogEntry::new(LogLevel::Warning, message).with_client(uid));
    }

    /// Log a client-related error message
    pub fn client_error(&self, uid: &str, message: impl Into<String>) {
        self.log(LogEntry::new(LogLevel::Error, message).with_client(uid));
    }

    /// Get all log entries
    pub fn get_all(&self) -> Vec<LogEntry> {
        self.entries.read().iter().cloned().collect()
    }

    /// Get log entries since a given timestamp
    pub fn get_since(&self, since_timestamp: u64) -> Vec<LogEntry> {
        self.entries
            .read()
            .iter()
            .filter(|e| e.timestamp > since_timestamp)
            .cloned()
            .collect()
    }

    /// Get the last N log entries
    pub fn get_recent(&self, count: usize) -> Vec<LogEntry> {
        let entries = self.entries.read();
        let start = entries.len().saturating_sub(count);
        entries.iter().skip(start).cloned().collect()
    }

    /// Clear all log entries
    pub fn clear(&self) {
        self.entries.write().clear();
    }
}

impl Default for LogStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared log store
pub type SharedLogStore = Arc<LogStore>;

/// Simple time formatting without pulling in chrono
fn chrono_lite_format(timestamp_ms: u64) -> String {
    let secs = (timestamp_ms / 1000) % 86400; // seconds since midnight UTC
    let hours = (secs / 3600) % 24;
    let minutes = (secs % 3600) / 60;
    let seconds = secs % 60;
    format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
}
