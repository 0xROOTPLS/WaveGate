//! Interactive shell session management.
//!
//! Spawns a PowerShell process on Windows (or sh on Unix) and pipes I/O through the client connection.

use std::process::Stdio;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc;
use parking_lot::Mutex;
use std::sync::Arc;

use wavegate_shared::{ClientMessageType, ShellOutputMessage, ShellExitMessage};

/// Message sender type for sending data back to the main connection
pub type ShellSender = mpsc::Sender<(ClientMessageType, Vec<u8>)>;

/// Handle to an active shell session
pub struct ShellSession {
    /// Sender to write to shell's stdin
    stdin_tx: mpsc::Sender<String>,
    /// Flag to signal shutdown
    shutdown: Arc<Mutex<bool>>,
}

impl ShellSession {
    /// Send input to the shell
    pub async fn send_input(&self, data: String) -> Result<(), String> {
        self.stdin_tx.send(data).await
            .map_err(|e| format!("Failed to send input to shell: {}", e))
    }

    /// Close the shell session
    pub fn close(&self) {
        *self.shutdown.lock() = true;
    }
}

/// Start a new interactive shell session.
/// Returns a ShellSession handle for sending input and closing.
pub async fn start_shell(output_tx: ShellSender) -> Result<ShellSession, String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    // Try multiple paths to find PowerShell
    let possible_paths = [
        ("Hardcoded System32", Some("C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe".to_string())),
        ("Hardcoded SysWOW64", Some("C:\\Windows\\SysWOW64\\WindowsPowerShell\\v1.0\\powershell.exe".to_string())),
        ("SYSTEMROOT System32", std::env::var("SYSTEMROOT").ok().map(|r| format!("{}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe", r))),
        ("SYSTEMROOT SysWOW64", std::env::var("SYSTEMROOT").ok().map(|r| format!("{}\\SysWOW64\\WindowsPowerShell\\v1.0\\powershell.exe", r))),
    ];

    let mut last_error = String::from("No shell paths to try");
    let mut child_opt: Option<Child> = None;

    for (_name, path_opt) in possible_paths.iter() {
        if let Some(path) = path_opt {
            if std::path::Path::new(path).exists() {
                match Command::new(path)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .creation_flags(CREATE_NO_WINDOW)
                    .spawn()
                {
                    Ok(c) => {
                        child_opt = Some(c);
                        break;
                    }
                    Err(e) => {
                        last_error = format!("{}: {}", path, e);
                    }
                }
            }
        }
    }

    let mut child = child_opt.ok_or_else(|| format!("Failed to spawn shell: {}", last_error))?;

    let stdin = child.stdin.take()
        .ok_or_else(|| "Failed to capture stdin".to_string())?;
    let stdout = child.stdout.take()
        .ok_or_else(|| "Failed to capture stdout".to_string())?;
    let stderr = child.stderr.take()
        .ok_or_else(|| "Failed to capture stderr".to_string())?;

    // Channel for stdin input
    let (stdin_tx, stdin_rx) = mpsc::channel::<String>(32);

    // Shutdown flag
    let shutdown = Arc::new(Mutex::new(false));
    let shutdown_stdin = shutdown.clone();
    let shutdown_stdout = shutdown.clone();
    let shutdown_stderr = shutdown.clone();
    let shutdown_wait = shutdown.clone();

    // Spawn stdin writer task
    tokio::spawn(stdin_writer_task(stdin, stdin_rx, shutdown_stdin));

    // Spawn stdout reader task
    let output_tx_stdout = output_tx.clone();
    tokio::spawn(stdout_reader_task(stdout, output_tx_stdout, shutdown_stdout));

    // Spawn stderr reader task
    let output_tx_stderr = output_tx.clone();
    tokio::spawn(stderr_reader_task(stderr, output_tx_stderr, shutdown_stderr));

    // Spawn process wait task
    tokio::spawn(process_wait_task(child, output_tx, shutdown_wait));

    Ok(ShellSession {
        stdin_tx,
        shutdown,
    })
}

/// Task that writes input to shell's stdin
async fn stdin_writer_task(
    mut stdin: ChildStdin,
    mut rx: mpsc::Receiver<String>,
    shutdown: Arc<Mutex<bool>>,
) {
    while let Some(input) = rx.recv().await {
        if *shutdown.lock() {
            break;
        }

        // Convert CR to LF for cmd.exe (xterm sends \r, cmd needs \n)
        let input = input.replace('\r', "\n");

        // Write input to stdin
        if stdin.write_all(input.as_bytes()).await.is_err() {
            break;
        }
        if stdin.flush().await.is_err() {
            break;
        }
    }

    // Close stdin when done
    drop(stdin);
}

/// Task that reads stdout and sends to server
async fn stdout_reader_task(
    mut stdout: tokio::process::ChildStdout,
    tx: ShellSender,
    shutdown: Arc<Mutex<bool>>,
) {
    let mut buffer = [0u8; 4096];

    loop {
        if *shutdown.lock() {
            break;
        }

        // Read whatever bytes are available (will block until some data arrives)
        match stdout.read(&mut buffer).await {
            Ok(0) => {
                // EOF - shell closed
                break;
            }
            Ok(n) => {
                // Convert to string and send
                let output = String::from_utf8_lossy(&buffer[..n]).to_string();
                let msg = ShellOutputMessage { data: output };

                if let Ok(payload) = serde_json::to_vec(&msg) {
                    if tx.send((ClientMessageType::ShellOutput, payload)).await.is_err() {
                        break;
                    }
                }
            }
            Err(_) => break,
        }
    }
}

/// Task that reads stderr and sends to server
async fn stderr_reader_task(
    mut stderr: tokio::process::ChildStderr,
    tx: ShellSender,
    shutdown: Arc<Mutex<bool>>,
) {
    let mut buffer = [0u8; 4096];

    loop {
        if *shutdown.lock() {
            break;
        }

        match stderr.read(&mut buffer).await {
            Ok(0) => break,
            Ok(n) => {
                let output = String::from_utf8_lossy(&buffer[..n]).to_string();
                let msg = ShellOutputMessage { data: output };

                if let Ok(payload) = serde_json::to_vec(&msg) {
                    if tx.send((ClientMessageType::ShellOutput, payload)).await.is_err() {
                        break;
                    }
                }
            }
            Err(_) => break,
        }
    }
}

/// Task that waits for the shell process to exit
async fn process_wait_task(
    mut child: Child,
    tx: ShellSender,
    shutdown: Arc<Mutex<bool>>,
) {
    let exit_status = child.wait().await;

    // Signal shutdown to other tasks
    *shutdown.lock() = true;

    let exit_code = exit_status.ok().and_then(|s| s.code());
    let msg = ShellExitMessage { exit_code };

    if let Ok(payload) = serde_json::to_vec(&msg) {
        let _ = tx.send((ClientMessageType::ShellExit, payload)).await;
    }
}
