//! Windows Services Manager for the client.
//!
//! Enumerates Windows services and provides control operations (start, stop, restart).

use wavegate_shared::{CommandResponseData, ServiceInfo};

/// List all Windows services
pub fn list_services() -> (bool, CommandResponseData) {
    match get_services() {
        Ok(services) => (true, CommandResponseData::ServiceList { services }),
        Err(e) => (false, CommandResponseData::Error { message: e }),
    }
}

/// Start a service by name
pub fn start_service(service_name: &str) -> (bool, CommandResponseData) {
    match control_service(service_name, ServiceAction::Start) {
        Ok(msg) => (true, CommandResponseData::ServiceResult { success: true, message: msg }),
        Err(e) => (false, CommandResponseData::ServiceResult { success: false, message: e }),
    }
}

/// Stop a service by name
pub fn stop_service(service_name: &str) -> (bool, CommandResponseData) {
    match control_service(service_name, ServiceAction::Stop) {
        Ok(msg) => (true, CommandResponseData::ServiceResult { success: true, message: msg }),
        Err(e) => (false, CommandResponseData::ServiceResult { success: false, message: e }),
    }
}

/// Restart a service (stop then start)
pub fn restart_service(service_name: &str) -> (bool, CommandResponseData) {
    // Stop the service first
    if let Err(e) = control_service(service_name, ServiceAction::Stop) {
        // If stop fails because it's already stopped, that's ok
        if !e.contains("not running") && !e.contains("not been started") {
            return (false, CommandResponseData::ServiceResult {
                success: false,
                message: format!("Failed to stop service: {}", e)
            });
        }
    }

    // Wait a moment for the service to fully stop
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Start the service
    match control_service(service_name, ServiceAction::Start) {
        Ok(msg) => (true, CommandResponseData::ServiceResult { success: true, message: msg }),
        Err(e) => (false, CommandResponseData::ServiceResult { success: false, message: e }),
    }
}

enum ServiceAction {
    Start,
    Stop,
}

/// Get all Windows services using the Service Control Manager
fn get_services() -> Result<Vec<ServiceInfo>, String> {
    use windows::core::PWSTR;
    use windows::Win32::System::Services::{
        CloseServiceHandle, EnumServicesStatusExW, OpenSCManagerW, OpenServiceW, QueryServiceConfigW,
        SC_MANAGER_ENUMERATE_SERVICE, SERVICE_QUERY_CONFIG, SERVICE_STATE_ALL,
        SERVICE_WIN32, ENUM_SERVICE_STATUS_PROCESSW, QUERY_SERVICE_CONFIGW,
        SERVICE_RUNNING, SERVICE_STOPPED, SERVICE_PAUSED, SERVICE_START_PENDING,
        SERVICE_STOP_PENDING, SERVICE_CONTINUE_PENDING, SERVICE_PAUSE_PENDING,
        SERVICE_AUTO_START, SERVICE_BOOT_START, SERVICE_DEMAND_START, SERVICE_DISABLED,
        SERVICE_SYSTEM_START, SC_HANDLE,
    };

    let mut services = Vec::new();

    unsafe {
        // Open the Service Control Manager
        let scm = OpenSCManagerW(None, None, SC_MANAGER_ENUMERATE_SERVICE)
            .map_err(|e| format!("Failed to open SCM: {}", e))?;

        // Get required buffer size
        let mut bytes_needed: u32 = 0;
        let mut services_returned: u32 = 0;
        let mut resume_handle: u32 = 0;

        let _ = EnumServicesStatusExW(
            scm,
            windows::Win32::System::Services::SC_ENUM_PROCESS_INFO,
            SERVICE_WIN32,
            SERVICE_STATE_ALL,
            None,
            &mut bytes_needed,
            &mut services_returned,
            Some(&mut resume_handle),
            None,
        );

        // Allocate buffer
        let mut buffer: Vec<u8> = vec![0u8; bytes_needed as usize];
        resume_handle = 0;

        // Get the services
        let result = EnumServicesStatusExW(
            scm,
            windows::Win32::System::Services::SC_ENUM_PROCESS_INFO,
            SERVICE_WIN32,
            SERVICE_STATE_ALL,
            Some(&mut buffer),
            &mut bytes_needed,
            &mut services_returned,
            Some(&mut resume_handle),
            None,
        );

        if result.is_err() {
            CloseServiceHandle(scm);
            return Err(format!("EnumServicesStatusExW failed: {:?}", result));
        }

        // Parse the results
        let entries = std::slice::from_raw_parts(
            buffer.as_ptr() as *const ENUM_SERVICE_STATUS_PROCESSW,
            services_returned as usize,
        );

        for entry in entries {
            // Get service name
            let name = if entry.lpServiceName.is_null() {
                String::new()
            } else {
                pwstr_to_string(entry.lpServiceName)
            };

            // Get display name
            let display_name = if entry.lpDisplayName.is_null() {
                name.clone()
            } else {
                pwstr_to_string(entry.lpDisplayName)
            };

            // Get status
            let status = match entry.ServiceStatusProcess.dwCurrentState {
                x if x == SERVICE_RUNNING => "Running",
                x if x == SERVICE_STOPPED => "Stopped",
                x if x == SERVICE_PAUSED => "Paused",
                x if x == SERVICE_START_PENDING => "Starting",
                x if x == SERVICE_STOP_PENDING => "Stopping",
                x if x == SERVICE_CONTINUE_PENDING => "Resuming",
                x if x == SERVICE_PAUSE_PENDING => "Pausing",
                _ => "Unknown",
            };

            // Get startup type by opening the service and querying config
            let startup_type = get_service_startup_type(scm, &name);

            let pid = entry.ServiceStatusProcess.dwProcessId;

            services.push(ServiceInfo {
                name: name.clone(),
                display_name,
                status: status.to_string(),
                startup_type,
                pid: if pid > 0 { Some(pid) } else { None },
            });
        }

        CloseServiceHandle(scm);
    }

    // Sort by display name
    services.sort_by(|a, b| a.display_name.to_lowercase().cmp(&b.display_name.to_lowercase()));

    Ok(services)
}

/// Get the startup type for a service
fn get_service_startup_type(scm: windows::Win32::System::Services::SC_HANDLE, service_name: &str) -> String {
    use windows::core::PCWSTR;
    use windows::Win32::System::Services::{
        CloseServiceHandle, OpenServiceW, QueryServiceConfigW,
        SERVICE_QUERY_CONFIG, QUERY_SERVICE_CONFIGW,
        SERVICE_AUTO_START, SERVICE_BOOT_START, SERVICE_DEMAND_START, SERVICE_DISABLED,
        SERVICE_SYSTEM_START,
    };

    unsafe {
        let name_wide: Vec<u16> = service_name.encode_utf16().chain(std::iter::once(0)).collect();

        let service = match OpenServiceW(scm, PCWSTR(name_wide.as_ptr()), SERVICE_QUERY_CONFIG) {
            Ok(s) => s,
            Err(_) => return "Unknown".to_string(),
        };

        // Get required buffer size
        let mut bytes_needed: u32 = 0;
        let _ = QueryServiceConfigW(service, None, 0, &mut bytes_needed);

        // Allocate buffer
        let mut buffer: Vec<u8> = vec![0u8; bytes_needed as usize];

        let result = QueryServiceConfigW(
            service,
            Some(buffer.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW),
            bytes_needed,
            &mut bytes_needed,
        );

        let startup_type = if result.is_ok() {
            let config = &*(buffer.as_ptr() as *const QUERY_SERVICE_CONFIGW);
            match config.dwStartType {
                x if x == SERVICE_AUTO_START => "Automatic",
                x if x == SERVICE_BOOT_START => "Boot",
                x if x == SERVICE_DEMAND_START => "Manual",
                x if x == SERVICE_DISABLED => "Disabled",
                x if x == SERVICE_SYSTEM_START => "System",
                _ => "Unknown",
            }
        } else {
            "Unknown"
        };

        CloseServiceHandle(service);
        startup_type.to_string()
    }
}

/// Control a service (start or stop)
fn control_service(service_name: &str, action: ServiceAction) -> Result<String, String> {
    use windows::core::PCWSTR;
    use windows::Win32::System::Services::{
        CloseServiceHandle, ControlService, OpenSCManagerW, OpenServiceW, StartServiceW,
        SC_MANAGER_CONNECT, SERVICE_CONTROL_STOP, SERVICE_START, SERVICE_STOP,
        SERVICE_STATUS,
    };

    unsafe {
        // Open the Service Control Manager
        let scm = OpenSCManagerW(None, None, SC_MANAGER_CONNECT)
            .map_err(|e| format!("Failed to open SCM: {}", e))?;

        let name_wide: Vec<u16> = service_name.encode_utf16().chain(std::iter::once(0)).collect();

        let access = match action {
            ServiceAction::Start => SERVICE_START,
            ServiceAction::Stop => SERVICE_STOP,
        };

        let service = match OpenServiceW(scm, PCWSTR(name_wide.as_ptr()), access) {
            Ok(s) => s,
            Err(e) => {
                CloseServiceHandle(scm);
                return Err(format!("Failed to open service '{}': {}", service_name, e));
            }
        };

        let result = match action {
            ServiceAction::Start => {
                match StartServiceW(service, None) {
                    Ok(_) => Ok(format!("Service '{}' started", service_name)),
                    Err(e) => Err(format!("Failed to start service '{}': {}", service_name, e)),
                }
            }
            ServiceAction::Stop => {
                let mut status: SERVICE_STATUS = std::mem::zeroed();
                match ControlService(service, SERVICE_CONTROL_STOP, &mut status) {
                    Ok(_) => Ok(format!("Service '{}' stopped", service_name)),
                    Err(e) => Err(format!("Failed to stop service '{}': {}", service_name, e)),
                }
            }
        };

        CloseServiceHandle(service);
        CloseServiceHandle(scm);

        result
    }
}

/// Convert a PWSTR to String
fn pwstr_to_string(pwstr: windows::core::PWSTR) -> String {
    if pwstr.is_null() {
        return String::new();
    }
    unsafe {
        let len = (0..).take_while(|&i| *pwstr.0.add(i) != 0).count();
        let slice = std::slice::from_raw_parts(pwstr.0, len);
        String::from_utf16_lossy(slice)
    }
}
