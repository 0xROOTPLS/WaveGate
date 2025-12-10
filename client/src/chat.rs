//! Chat window for user interaction.
//!
//! Spawns a simple chat GUI in its own thread for operator-user communication.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

/// Messages from the chat window to the main client
#[derive(Debug, Clone)]
pub enum ChatEvent {
    /// User sent a message
    UserMessage(String),
    /// Window was closed
    WindowClosed,
}

/// Messages from the main client to the chat window
#[derive(Debug, Clone)]
pub enum ChatCommand {
    /// Add a message to the chat (from operator)
    AddMessage { sender: String, message: String },
    /// Close the window
    Close,
}

/// Handle to an active chat session
pub struct ChatSession {
    command_tx: Sender<ChatCommand>,
    event_rx: Arc<Mutex<Receiver<ChatEvent>>>,
    operator_name: String,
}

impl ChatSession {
    /// Send a message from the operator to display in chat
    pub fn send_operator_message(&self, message: &str) {
        let _ = self.command_tx.send(ChatCommand::AddMessage {
            sender: self.operator_name.clone(),
            message: message.to_string(),
        });
    }

    /// Close the chat window
    pub fn close(&self) {
        let _ = self.command_tx.send(ChatCommand::Close);
    }

    /// Try to receive an event (non-blocking)
    pub fn try_recv_event(&self) -> Option<ChatEvent> {
        if let Ok(rx) = self.event_rx.lock() {
            rx.try_recv().ok()
        } else {
            None
        }
    }
}

/// Global chat session holder
static CHAT_SESSION: once_cell::sync::Lazy<Mutex<Option<ChatSession>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(None));

/// Start a new chat session
pub fn start_chat(operator_name: &str) -> Result<(), String> {
    let mut session_guard = CHAT_SESSION.lock().map_err(|e| e.to_string())?;

    // Close existing session if any
    if let Some(session) = session_guard.take() {
        session.close();
        // Give the old window thread time to process the close command
        drop(session_guard);
        std::thread::sleep(std::time::Duration::from_millis(150));
        session_guard = CHAT_SESSION.lock().map_err(|e| e.to_string())?;
    }

    // Create channels
    let (command_tx, command_rx) = mpsc::channel::<ChatCommand>();
    let (event_tx, event_rx) = mpsc::channel::<ChatEvent>();

    let op_name = operator_name.to_string();
    let op_name_clone = op_name.clone();

    // Spawn the chat window in a separate thread
    thread::spawn(move || {
        let _ = run_chat_window(&op_name_clone, command_rx, event_tx);
    });

    *session_guard = Some(ChatSession {
        command_tx,
        event_rx: Arc::new(Mutex::new(event_rx)),
        operator_name: op_name,
    });

    Ok(())
}

/// Send a message from the operator
pub fn send_message(message: &str) -> Result<(), String> {
    let session_guard = CHAT_SESSION.lock().map_err(|e| e.to_string())?;
    if let Some(session) = session_guard.as_ref() {
        session.send_operator_message(message);
        Ok(())
    } else {
        Err("No active chat session".to_string())
    }
}

/// Close the chat session
pub fn close_chat() -> Result<(), String> {
    let mut session_guard = CHAT_SESSION.lock().map_err(|e| e.to_string())?;
    if let Some(session) = session_guard.take() {
        session.close();
        // Give the window thread time to process the close command
        drop(session_guard);
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Ok(())
}

/// Poll for events from the chat window
pub fn poll_event() -> Option<ChatEvent> {
    if let Ok(session_guard) = CHAT_SESSION.lock() {
        if let Some(session) = session_guard.as_ref() {
            return session.try_recv_event();
        }
    }
    None
}

/// Check if chat is active
#[allow(dead_code)]
pub fn is_chat_active() -> bool {
    if let Ok(session_guard) = CHAT_SESSION.lock() {
        session_guard.is_some()
    } else {
        false
    }
}

// ============================================================================
// Windows GUI Implementation
// ============================================================================

fn run_chat_window(
    operator_name: &str,
    command_rx: Receiver<ChatCommand>,
    event_tx: Sender<ChatEvent>,
) -> Result<(), String> {
    use windows::core::{PCWSTR, w};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{GetStockObject, HBRUSH, WHITE_BRUSH, UpdateWindow};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Controls::EM_SETSEL;
    use windows::Win32::UI::WindowsAndMessaging::*;

    const ID_CHAT_LOG: i32 = 101;
    const ID_INPUT: i32 = 102;
    const ID_SEND: i32 = 103;
    const TIMER_ID: usize = 1;

    // Store state in thread-local for window proc access
    thread_local! {
        static CHAT_STATE: std::cell::RefCell<Option<ChatWindowState>> = std::cell::RefCell::new(None);
    }

    struct ChatWindowState {
        command_rx: Receiver<ChatCommand>,
        event_tx: Sender<ChatEvent>,
        operator_name: String,
        chat_log: String,
        hwnd_log: HWND,
        hwnd_input: HWND,
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_CREATE => {
                LRESULT(0)
            }
            WM_SIZE => {
                let mut rect = RECT::default();
                let _ = GetClientRect(hwnd, &mut rect);
                let width = rect.right - rect.left;
                let height = rect.bottom - rect.top;

                CHAT_STATE.with(|state| {
                    if let Some(ref state) = *state.borrow() {
                        let _ = MoveWindow(state.hwnd_log, 10, 10, width - 20, height - 80, true);
                        let _ = MoveWindow(state.hwnd_input, 10, height - 60, width - 90, 25, true);
                        if let Ok(hwnd_send) = GetDlgItem(Some(hwnd), ID_SEND) {
                            let _ = MoveWindow(hwnd_send, width - 70, height - 60, 60, 25, true);
                        }
                    }
                });
                LRESULT(0)
            }
            WM_GETMINMAXINFO => {
                let mmi = lparam.0 as *mut MINMAXINFO;
                if !mmi.is_null() {
                    (*mmi).ptMinTrackSize.x = 350;
                    (*mmi).ptMinTrackSize.y = 250;
                }
                LRESULT(0)
            }
            WM_TIMER => {
                CHAT_STATE.with(|state| {
                    if let Some(ref mut state) = *state.borrow_mut() {
                        while let Ok(cmd) = state.command_rx.try_recv() {
                            match cmd {
                                ChatCommand::AddMessage { sender, message } => {
                                    let timestamp = time::OffsetDateTime::now_local()
                                        .map(|t| format!("{:02}:{:02}:{:02}", t.hour(), t.minute(), t.second()))
                                        .unwrap_or_else(|_| "??:??:??".to_string());
                                    let line = format!("[{}] {}: {}\r\n", timestamp, sender, message);
                                    state.chat_log.push_str(&line);

                                    let text: Vec<u16> = state.chat_log.encode_utf16().chain(std::iter::once(0)).collect();
                                    let _ = SetWindowTextW(state.hwnd_log, PCWSTR(text.as_ptr()));

                                    let len = state.chat_log.len() as i32;
                                    let _ = SendMessageW(state.hwnd_log, EM_SETSEL, Some(WPARAM(len as usize)), Some(LPARAM(len as isize)));
                                }
                                ChatCommand::Close => {
                                    let _ = SendMessageW(hwnd, WM_CLOSE, Some(WPARAM(0)), Some(LPARAM(0)));
                                }
                            }
                        }
                    }
                });
                LRESULT(0)
            }
            WM_COMMAND => {
                let id = (wparam.0 & 0xFFFF) as i32;
                if id == ID_SEND {
                    if let Ok(hwnd_input) = GetDlgItem(Some(hwnd), ID_INPUT) {
                        let mut buffer = [0u16; 4096];
                        let len = GetWindowTextW(hwnd_input, &mut buffer);
                        if len > 0 {
                            let text = String::from_utf16_lossy(&buffer[..len as usize]);
                            if !text.trim().is_empty() {
                                CHAT_STATE.with(|state| {
                                    if let Some(ref mut state) = *state.borrow_mut() {
                                        let timestamp = time::OffsetDateTime::now_local()
                                            .map(|t| format!("{:02}:{:02}:{:02}", t.hour(), t.minute(), t.second()))
                                            .unwrap_or_else(|_| "??:??:??".to_string());
                                        let line = format!("[{}] You: {}\r\n", timestamp, text.trim());
                                        state.chat_log.push_str(&line);

                                        let log_text: Vec<u16> = state.chat_log.encode_utf16().chain(std::iter::once(0)).collect();
                                        let _ = SetWindowTextW(state.hwnd_log, PCWSTR(log_text.as_ptr()));

                                        let log_len = state.chat_log.len() as i32;
                                        let _ = SendMessageW(state.hwnd_log, EM_SETSEL, Some(WPARAM(log_len as usize)), Some(LPARAM(log_len as isize)));

                                        let _ = state.event_tx.send(ChatEvent::UserMessage(text.trim().to_string()));
                                    }
                                });
                                let empty: Vec<u16> = vec![0];
                                let _ = SetWindowTextW(hwnd_input, PCWSTR(empty.as_ptr()));
                            }
                        }
                    }
                }
                LRESULT(0)
            }
            WM_CLOSE => {
                CHAT_STATE.with(|state| {
                    if let Some(ref state) = *state.borrow() {
                        let _ = state.event_tx.send(ChatEvent::WindowClosed);
                    }
                });
                let _ = KillTimer(Some(hwnd), TIMER_ID);
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    unsafe {
        let hinstance = GetModuleHandleW(None).map_err(|e| e.to_string())?;

        let class_name = w!("WaveGateChatWindow");

        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window_proc),
            hInstance: hinstance.into(),
            lpszClassName: class_name,
            hbrBackground: HBRUSH(GetStockObject(WHITE_BRUSH).0),
            ..Default::default()
        };

        RegisterClassW(&wc);

        let title = format!("Chat - {}", operator_name);
        let title_wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();

        let hwnd = CreateWindowExW(
            Default::default(),
            class_name,
            PCWSTR(title_wide.as_ptr()),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            400,
            350,
            None,
            None,
            Some(hinstance.into()),
            None,
        ).map_err(|e| format!("CreateWindowExW failed: {}", e))?;

        // Create chat log (multiline readonly edit)
        let hwnd_log = CreateWindowExW(
            Default::default(),
            w!("EDIT"),
            PCWSTR::null(),
            WS_CHILD | WS_VISIBLE | WS_VSCROLL | WS_BORDER |
            WINDOW_STYLE((ES_MULTILINE | ES_AUTOVSCROLL | ES_READONLY) as u32),
            10, 10, 360, 220,
            Some(hwnd),
            Some(HMENU(ID_CHAT_LOG as _)),
            Some(hinstance.into()),
            None,
        ).map_err(|e| format!("Failed to create chat log: {}", e))?;

        // Create input field
        let hwnd_input = CreateWindowExW(
            Default::default(),
            w!("EDIT"),
            PCWSTR::null(),
            WS_CHILD | WS_VISIBLE | WS_BORDER | WS_TABSTOP,
            10, 260, 290, 25,
            Some(hwnd),
            Some(HMENU(ID_INPUT as _)),
            Some(hinstance.into()),
            None,
        ).map_err(|e| format!("Failed to create input field: {}", e))?;

        // Create send button
        let _hwnd_send = CreateWindowExW(
            Default::default(),
            w!("BUTTON"),
            w!("Send"),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32),
            310, 260, 60, 25,
            Some(hwnd),
            Some(HMENU(ID_SEND as _)),
            Some(hinstance.into()),
            None,
        ).map_err(|e| format!("Failed to create send button: {}", e))?;

        // Initialize state
        let initial_msg = format!("Chat session started with {}.\r\n", operator_name);
        CHAT_STATE.with(|state| {
            *state.borrow_mut() = Some(ChatWindowState {
                command_rx,
                event_tx,
                operator_name: operator_name.to_string(),
                chat_log: initial_msg.clone(),
                hwnd_log,
                hwnd_input,
            });
        });

        // Set initial log text
        let init_text: Vec<u16> = initial_msg.encode_utf16().chain(std::iter::once(0)).collect();
        let _ = SetWindowTextW(hwnd_log, PCWSTR(init_text.as_ptr()));

        // Start timer for checking commands (100ms interval)
        let _ = SetTimer(Some(hwnd), TIMER_ID, 100, None);

        // Force initial layout
        let _ = SendMessageW(hwnd, WM_SIZE, Some(WPARAM(0)), Some(LPARAM(0)));

        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = UpdateWindow(hwnd);

        // Message loop
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        // Clean up state
        CHAT_STATE.with(|state| {
            *state.borrow_mut() = None;
        });
    }

    Ok(())
}
