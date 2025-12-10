//! Process manager operations for the client.

use wavegate_shared::{CommandResponseData, ProcessInfo};
use std::collections::HashMap;
use std::mem;
use std::sync::Mutex;
use once_cell::sync::Lazy;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW,
    PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ, GetProcessTimes};

/// Cache for CPU time measurements (pid -> (kernel_time, user_time, timestamp))
static CPU_CACHE: Lazy<Mutex<HashMap<u32, (u64, u64, std::time::Instant)>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// List all running processes
pub fn list_processes() -> (bool, CommandResponseData) {
    let mut processes = Vec::new();
    let mut new_cache: HashMap<u32, (u64, u64, std::time::Instant)> = HashMap::new();
    let now = std::time::Instant::now();

    // Get old cache for CPU calculation
    let old_cache = CPU_CACHE.lock().ok().map(|c| c.clone()).unwrap_or_default();

    unsafe {
        let snapshot = match CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
            Ok(h) => h,
            Err(_) => {
                return (false, CommandResponseData::Error {
                    message: "Failed to create process snapshot".to_string(),
                });
            }
        };

        let mut entry: PROCESSENTRY32W = mem::zeroed();
        entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;

        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let name_len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..name_len]);
                let pid = entry.th32ProcessID;

                let mut memory_bytes: u64 = 0;
                let mut cpu_percent: f32 = 0.0;

                if let Ok(proc_handle) = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid) {
                    // Get memory info
                    let mut mem_counters: PROCESS_MEMORY_COUNTERS = mem::zeroed();
                    mem_counters.cb = mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
                    if GetProcessMemoryInfo(proc_handle, &mut mem_counters, mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32).is_ok() {
                        memory_bytes = mem_counters.WorkingSetSize as u64;
                    }

                    // Get CPU times
                    let mut creation_time = mem::zeroed();
                    let mut exit_time = mem::zeroed();
                    let mut kernel_time = mem::zeroed();
                    let mut user_time = mem::zeroed();

                    if GetProcessTimes(proc_handle, &mut creation_time, &mut exit_time, &mut kernel_time, &mut user_time).is_ok() {
                        let kernel = filetime_to_u64(&kernel_time);
                        let user = filetime_to_u64(&user_time);
                        let total_time = kernel + user;

                        // Calculate CPU % if we have previous measurement
                        if let Some(&(old_kernel, old_user, old_instant)) = old_cache.get(&pid) {
                            let old_total = old_kernel + old_user;
                            let time_diff = total_time.saturating_sub(old_total);
                            let elapsed_100ns = old_instant.elapsed().as_nanos() as u64 / 100;

                            if elapsed_100ns > 0 {
                                // Divide by number of CPUs for accurate percentage
                                let num_cpus = std::thread::available_parallelism().map(|p| p.get()).unwrap_or(1) as u64;
                                cpu_percent = ((time_diff as f64 / elapsed_100ns as f64) * 100.0 / num_cpus as f64) as f32;
                                cpu_percent = cpu_percent.min(100.0).max(0.0);
                            }
                        }

                        new_cache.insert(pid, (kernel, user, now));
                    }

                    let _ = CloseHandle(proc_handle);
                }

                processes.push(ProcessInfo {
                    pid,
                    name,
                    cpu_percent,
                    memory_bytes,
                });

                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }

        let _ = CloseHandle(snapshot);
    }

    // Update cache
    if let Ok(mut cache) = CPU_CACHE.lock() {
        *cache = new_cache;
    }

    processes.sort_by(|a, b| b.memory_bytes.cmp(&a.memory_bytes));
    (true, CommandResponseData::ProcessList { processes })
}

/// Convert FILETIME to u64 (100-nanosecond intervals)
fn filetime_to_u64(ft: &windows::Win32::Foundation::FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64)
}

/// Kill a process by PID
pub fn kill_process(pid: u32) -> (bool, CommandResponseData) {
    unsafe {
        match OpenProcess(PROCESS_TERMINATE, false, pid) {
            Ok(handle) => {
                let result = TerminateProcess(handle, 1);
                let _ = CloseHandle(handle);

                match result {
                    Ok(_) => (true, CommandResponseData::Generic {
                        message: format!("Process {} terminated", pid),
                    }),
                    Err(_) => (false, CommandResponseData::Error {
                        message: format!("Failed to terminate process {}", pid),
                    }),
                }
            }
            Err(_) => (false, CommandResponseData::Error {
                message: format!("Failed to open process {}", pid),
            }),
        }
    }
}
