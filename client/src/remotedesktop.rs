//! Remote Desktop module for screen capture and input simulation.
//!
//! Uses DXGI Desktop Duplication for high-performance capture with
//! dirty rectangle tracking. Falls back to GDI if DXGI is unavailable.
//!
//! Performance optimizations:
//! - Zero-copy frame access from GPU memory
//! - jpeg-encoder for fast pure-Rust JPEG encoding
//! - Parallel tile encoding with rayon
//! - HashSet for O(1) tile deduplication

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use jpeg_encoder::{Encoder, ColorType};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use rayon::prelude::*;
use tokio::sync::mpsc;

use wavegate_shared::{CommandResponseData, ScreenTile};
use crate::dxgi::{DxgiCapturer, CaptureError, DirtyRect};

/// Tile size for incremental updates (128x128 reduces tile count while maintaining granularity)
const TILE_SIZE: u32 = 128;

/// Maximum number of tiles to send per non-keyframe to avoid overwhelming the connection
/// Keyframes are unlimited to ensure full screen is sent
const MAX_TILES_PER_FRAME: usize = 200;

/// Remote desktop stream state
struct RemoteDesktopState {
    running: Arc<AtomicBool>,
    frame_tx: Option<mpsc::UnboundedSender<Vec<u8>>>,
}

impl Default for RemoteDesktopState {
    fn default() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            frame_tx: None,
        }
    }
}

static RD_STATE: Lazy<Mutex<RemoteDesktopState>> = Lazy::new(|| Mutex::new(RemoteDesktopState::default()));


/// Start remote desktop streaming
pub fn start_remote_desktop(
    fps: u8,
    quality: u8,
    frame_tx: mpsc::UnboundedSender<Vec<u8>>,
) -> (bool, CommandResponseData) {
    let mut state = RD_STATE.lock();

    if state.running.load(Ordering::SeqCst) {
        if let Some(ref old_tx) = state.frame_tx {
            if old_tx.is_closed() {
                state.running.store(false, Ordering::SeqCst);
                state.frame_tx = None;
            } else {
                return (false, CommandResponseData::Error {
                    message: "Remote desktop stream already running".to_string(),
                });
            }
        } else {
            state.running.store(false, Ordering::SeqCst);
        }
    }

    // Get screen dimensions
    let (screen_width, screen_height) = get_screen_dimensions();
    if screen_width == 0 || screen_height == 0 {
        return (false, CommandResponseData::Error {
            message: "Failed to get screen dimensions".to_string(),
        });
    }

    state.running = Arc::new(AtomicBool::new(true));
    state.frame_tx = Some(frame_tx);
    let running = state.running.clone();
    let tx = state.frame_tx.clone();

    drop(state);

    let fps = fps.max(1).min(60) as u32;
    let quality = quality.max(10).min(100);

    std::thread::spawn(move || {
        capture_loop_dxgi(fps, quality, running.clone(), tx.clone())
            .unwrap_or_else(|e| {
                eprintln!("DXGI capture failed: {}, falling back to GDI", e);
                capture_loop_gdi(fps, quality, running, tx);
            });
    });

    (true, CommandResponseData::RemoteDesktopStarted {
        width: screen_width as u16,
        height: screen_height as u16,
    })
}

/// Stop remote desktop streaming
pub fn stop_remote_desktop() -> (bool, CommandResponseData) {
    let mut state = RD_STATE.lock();

    if !state.running.load(Ordering::SeqCst) {
        return (false, CommandResponseData::Error {
            message: "No remote desktop stream running".to_string(),
        });
    }

    state.running.store(false, Ordering::SeqCst);
    state.frame_tx = None;

    (true, CommandResponseData::RemoteDesktopStopped)
}

/// Check if streaming
pub fn is_streaming() -> bool {
    RD_STATE.lock().running.load(Ordering::SeqCst)
}

/// Get screen dimensions
fn get_screen_dimensions() -> (i32, i32) {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN};
    unsafe {
        (GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN))
    }
}

/// Tile info for parallel encoding
#[derive(Clone)]
struct TileInfo {
    x: u32,
    y: u32,
    rect: DirtyRect,
}

/// Extract a tile from BGRA data (handles stride correctly)
/// Returns a contiguous BGRA buffer for the tile
#[inline]
fn extract_tile_bgra(
    bgra_data: &[u8],
    stride: u32,
    rect: &DirtyRect,
) -> Vec<u8> {
    let tile_width = rect.width() as usize;
    let tile_height = rect.height() as usize;
    let src_stride = stride as usize;

    let mut tile_buffer = Vec::with_capacity(tile_width * tile_height * 4);
    let left = rect.left as usize * 4;
    let row_bytes = tile_width * 4;

    for y in rect.top as usize..rect.bottom as usize {
        let row_start = y * src_stride + left;
        let row_end = row_start + row_bytes;
        if row_end <= bgra_data.len() {
            tile_buffer.extend_from_slice(&bgra_data[row_start..row_end]);
        }
    }

    tile_buffer
}

/// DXGI-based capture loop with dirty rectangles and tile encoding
///
/// Performance optimizations:
/// - Zero-copy: Frame data borrowed directly from GPU memory
/// - No color conversion: turbojpeg encodes BGRA directly
/// - Parallel encoding: Tiles encoded concurrently with rayon
/// - O(1) deduplication: HashSet instead of linear search
fn capture_loop_dxgi(
    fps: u32,
    quality: u8,
    running: Arc<AtomicBool>,
    frame_tx: Option<mpsc::UnboundedSender<Vec<u8>>>,
) -> Result<(), String> {
    let tx = frame_tx.ok_or("No frame sender")?;
    let frame_interval = Duration::from_millis(1000 / fps as u64);

    // Create DXGI capturer
    let mut capturer = DxgiCapturer::new()?;
    let (width, height) = capturer.dimensions();

    // Calculate tile grid
    let tiles_x = (width + TILE_SIZE - 1) / TILE_SIZE;
    let tiles_y = (height + TILE_SIZE - 1) / TILE_SIZE;

    let mut frame_count = 0u32;
    let keyframe_interval = fps * 2; // Full frame every 2 seconds

    // Use shorter timeout for lower latency
    let timeout_ms = (1000 / fps).max(16).min(50);

    // Pre-allocate tile info vec and HashSet for deduplication
    let mut changed_tiles: Vec<TileInfo> = Vec::with_capacity(256);
    let mut tile_set: HashSet<(u32, u32)> = HashSet::with_capacity(256);

    while running.load(Ordering::SeqCst) {
        let frame_start = Instant::now();

        // Capture frame (ZERO COPY - data borrowed from GPU)
        let frame = match capturer.capture_frame(timeout_ms) {
            Ok(f) => f,
            Err(CaptureError::Timeout) | Err(CaptureError::NoFrame) => {
                // Brief sleep and retry - don't wait full frame interval
                std::thread::sleep(Duration::from_millis(1));
                continue;
            }
            Err(CaptureError::AccessLost) => {
                // Recreate capturer
                std::thread::sleep(Duration::from_millis(100));
                capturer = match DxgiCapturer::new() {
                    Ok(c) => c,
                    Err(_) => return Err("Failed to recreate capturer".into()),
                };
                continue;
            }
            Err(e) => {
                eprintln!("Capture error: {}", e);
                std::thread::sleep(frame_interval);
                continue;
            }
        };

        // Determine if this is a keyframe
        let is_keyframe = frame_count % keyframe_interval == 0;
        frame_count += 1;

        // Collect changed tiles with O(1) deduplication
        changed_tiles.clear();
        tile_set.clear();

        if is_keyframe || frame_count == 1 {
            // Send all tiles for keyframe - no limit, must complete full screen
            for ty in 0..tiles_y {
                for tx_idx in 0..tiles_x {
                    let rect = DirtyRect {
                        left: (tx_idx * TILE_SIZE) as i32,
                        top: (ty * TILE_SIZE) as i32,
                        right: ((tx_idx + 1) * TILE_SIZE).min(width) as i32,
                        bottom: ((ty + 1) * TILE_SIZE).min(height) as i32,
                    };
                    changed_tiles.push(TileInfo { x: tx_idx, y: ty, rect });
                }
            }
        } else {
            // Use dirty rectangles to find changed tiles
            for dirty_rect in &frame.dirty_rects {
                // Find all tiles that intersect with this dirty rect
                let start_tx = (dirty_rect.left.max(0) as u32) / TILE_SIZE;
                let end_tx = ((dirty_rect.right as u32 + TILE_SIZE - 1) / TILE_SIZE).min(tiles_x);
                let start_ty = (dirty_rect.top.max(0) as u32) / TILE_SIZE;
                let end_ty = ((dirty_rect.bottom as u32 + TILE_SIZE - 1) / TILE_SIZE).min(tiles_y);

                for ty in start_ty..end_ty {
                    for tx_idx in start_tx..end_tx {
                        // O(1) deduplication with HashSet
                        if tile_set.insert((tx_idx, ty)) {
                            let rect = DirtyRect {
                                left: (tx_idx * TILE_SIZE) as i32,
                                top: (ty * TILE_SIZE) as i32,
                                right: ((tx_idx + 1) * TILE_SIZE).min(width) as i32,
                                bottom: ((ty + 1) * TILE_SIZE).min(height) as i32,
                            };
                            changed_tiles.push(TileInfo { x: tx_idx, y: ty, rect });
                        }
                    }
                }
            }

            // Limit tiles per frame for non-keyframes only
            if changed_tiles.len() > MAX_TILES_PER_FRAME {
                changed_tiles.truncate(MAX_TILES_PER_FRAME);
            }
        }

        // PARALLEL tile encoding with rayon
        // Each tile: extract BGRA data -> encode to JPEG (no color conversion!)
        if !changed_tiles.is_empty() {
            let frame_data = frame.data;
            let frame_stride = frame.stride;

            let tiles: Vec<ScreenTile> = changed_tiles
                .par_iter()
                .filter_map(|tile_info| {
                    let rect = &tile_info.rect;
                    let tile_width = rect.width() as usize;
                    let tile_height = rect.height() as usize;

                    // Extract BGRA tile data
                    let bgra_data = extract_tile_bgra(frame_data, frame_stride, rect);

                    // Encode BGRA directly to JPEG using turbojpeg (NO color conversion!)
                    encode_tile_jpeg_bgra(&bgra_data, tile_width, tile_height, quality)
                        .map(|jpeg| ScreenTile {
                            x: rect.left as u16,
                            y: rect.top as u16,
                            width: tile_width as u16,
                            height: tile_height as u16,
                            jpeg_data: jpeg,
                        })
                })
                .collect();

            if !tiles.is_empty() {
                // Send as binary frame with tile data
                if let Ok(payload) = encode_tile_frame(width as u16, height as u16, is_keyframe, &tiles) {
                    if tx.send(payload).is_err() {
                        break;
                    }
                }
            }
        }

        // Maintain frame rate
        let elapsed = frame_start.elapsed();
        if elapsed < frame_interval {
            std::thread::sleep(frame_interval - elapsed);
        }
    }

    running.store(false, Ordering::SeqCst);
    Ok(())
}

/// GDI fallback capture loop
fn capture_loop_gdi(
    fps: u32,
    quality: u8,
    running: Arc<AtomicBool>,
    frame_tx: Option<mpsc::UnboundedSender<Vec<u8>>>,
) {
    use windows::Win32::Foundation::*;
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::UI::WindowsAndMessaging::*;

    let tx = match frame_tx {
        Some(tx) => tx,
        None => return,
    };

    let frame_interval = Duration::from_millis(1000 / fps as u64);
    let mut frame_count = 0u32;
    let keyframe_interval = fps * 2;
    let mut previous_frame: Option<Vec<u8>> = None;

    while running.load(Ordering::SeqCst) {
        let frame_start = Instant::now();

        unsafe {
            let width = GetSystemMetrics(SM_CXVIRTUALSCREEN);
            let height = GetSystemMetrics(SM_CYVIRTUALSCREEN);
            let origin_x = GetSystemMetrics(SM_XVIRTUALSCREEN);
            let origin_y = GetSystemMetrics(SM_YVIRTUALSCREEN);

            if width == 0 || height == 0 {
                std::thread::sleep(frame_interval);
                continue;
            }

            let screen_dc = GetDC(None);
            if screen_dc.is_invalid() {
                std::thread::sleep(frame_interval);
                continue;
            }

            let mem_dc = CreateCompatibleDC(Some(screen_dc));
            if mem_dc.is_invalid() {
                ReleaseDC(None, screen_dc);
                std::thread::sleep(frame_interval);
                continue;
            }

            let bitmap = CreateCompatibleBitmap(screen_dc, width, height);
            if bitmap.is_invalid() {
                DeleteDC(mem_dc);
                ReleaseDC(None, screen_dc);
                std::thread::sleep(frame_interval);
                continue;
            }

            let old_bitmap = SelectObject(mem_dc, bitmap.into());
            let _ = BitBlt(mem_dc, 0, 0, width, height, Some(screen_dc), origin_x, origin_y, SRCCOPY);

            let mut bmp_info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 24,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
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
                std::thread::sleep(frame_interval);
                continue;
            }

            // Convert BGR to RGB
            for row in pixels.chunks_mut(stride) {
                for pixel in row[..width as usize * 3].chunks_mut(3) {
                    pixel.swap(0, 2);
                }
            }

            // Remove padding
            let mut rgb_data = Vec::with_capacity((width * 3) as usize * height as usize);
            for row in pixels.chunks(stride) {
                rgb_data.extend_from_slice(&row[..width as usize * 3]);
            }

            // Check if we should send (basic delta check)
            let is_keyframe = frame_count % keyframe_interval == 0;
            frame_count += 1;

            let should_send = if is_keyframe {
                true
            } else if let Some(ref prev) = previous_frame {
                frames_differ(&rgb_data, prev)
            } else {
                true
            };

            if !should_send {
                previous_frame = Some(rgb_data);
                let elapsed = frame_start.elapsed();
                if elapsed < frame_interval {
                    std::thread::sleep(frame_interval - elapsed);
                }
                continue;
            }

            // Encode full frame as single tile using turbojpeg
            let jpeg = encode_jpeg_rgb(&rgb_data, width as usize, height as usize, quality);

            previous_frame = Some(rgb_data);

            if let Some(jpeg) = jpeg {
                let tile = ScreenTile {
                    x: 0,
                    y: 0,
                    width: width as u16,
                    height: height as u16,
                    jpeg_data: jpeg,
                };

                if let Ok(payload) = encode_tile_frame(width as u16, height as u16, is_keyframe, &[tile]) {
                    if tx.send(payload).is_err() {
                        break;
                    }
                }
            }
        }

        let elapsed = frame_start.elapsed();
        if elapsed < frame_interval {
            std::thread::sleep(frame_interval - elapsed);
        }
    }

    running.store(false, Ordering::SeqCst);
}

/// Check if frames differ significantly (for GDI fallback)
fn frames_differ(current: &[u8], previous: &[u8]) -> bool {
    if current.len() != previous.len() {
        return true;
    }

    let mut diff_count = 0u32;
    let threshold = 30u32;
    let sample_step = 16; // Sample every 16th pixel for speed

    for i in (0..current.len() / 3).step_by(sample_step) {
        let idx = i * 3;
        if idx + 3 > current.len() {
            break;
        }

        let dr = (current[idx] as i32 - previous[idx] as i32).unsigned_abs();
        let dg = (current[idx + 1] as i32 - previous[idx + 1] as i32).unsigned_abs();
        let db = (current[idx + 2] as i32 - previous[idx + 2] as i32).unsigned_abs();

        if dr + dg + db > threshold {
            diff_count += 1;
        }
    }

    // Changed if more than 0.5% of sampled pixels differ
    diff_count > (current.len() / 3 / sample_step / 200) as u32
}

/// Convert BGRA to RGB in a pre-allocated buffer (for tile encoding)
/// Only converts the tile, not the whole frame
#[inline]
fn bgra_to_rgb_tile(bgra: &[u8], rgb: &mut Vec<u8>) {
    rgb.clear();
    rgb.reserve(bgra.len() / 4 * 3);

    for chunk in bgra.chunks_exact(4) {
        rgb.push(chunk[2]); // R
        rgb.push(chunk[1]); // G
        rgb.push(chunk[0]); // B
    }
}

/// Encode BGRA tile to JPEG using jpeg-encoder (pure Rust, fast)
/// Converts BGRA->RGB only for the tile being encoded, not the whole frame
fn encode_tile_jpeg_bgra(bgra_data: &[u8], width: usize, height: usize, quality: u8) -> Option<Vec<u8>> {
    std::panic::catch_unwind(|| {
        // Convert BGRA to RGB for this tile only
        let mut rgb_data = Vec::with_capacity(width * height * 3);
        bgra_to_rgb_tile(bgra_data, &mut rgb_data);

        // Encode with jpeg-encoder
        let mut output = Vec::new();
        let encoder = Encoder::new(&mut output, quality);
        encoder.encode(&rgb_data, width as u16, height as u16, ColorType::Rgb).ok()?;
        Some(output)
    })
    .ok()
    .flatten()
}

/// Encode RGB data to JPEG using jpeg-encoder (for GDI fallback)
fn encode_jpeg_rgb(rgb_data: &[u8], width: usize, height: usize, quality: u8) -> Option<Vec<u8>> {
    std::panic::catch_unwind(|| {
        let mut output = Vec::new();
        let encoder = Encoder::new(&mut output, quality);
        encoder.encode(rgb_data, width as u16, height as u16, ColorType::Rgb).ok()?;
        Some(output)
    })
    .ok()
    .flatten()
}

/// Encode tile frame for transmission
/// Format: [width:u16][height:u16][is_keyframe:u8][tile_count:u16][tiles...]
/// Each tile: [x:u16][y:u16][w:u16][h:u16][jpeg_len:u32][jpeg_data...]
fn encode_tile_frame(width: u16, height: u16, is_keyframe: bool, tiles: &[ScreenTile]) -> Result<Vec<u8>, ()> {
    let mut buf = Vec::new();

    // Header
    buf.extend_from_slice(&width.to_le_bytes());
    buf.extend_from_slice(&height.to_le_bytes());
    buf.push(if is_keyframe { 1 } else { 0 });
    buf.extend_from_slice(&(tiles.len() as u16).to_le_bytes());

    // Tiles
    for tile in tiles {
        buf.extend_from_slice(&tile.x.to_le_bytes());
        buf.extend_from_slice(&tile.y.to_le_bytes());
        buf.extend_from_slice(&tile.width.to_le_bytes());
        buf.extend_from_slice(&tile.height.to_le_bytes());
        buf.extend_from_slice(&(tile.jpeg_data.len() as u32).to_le_bytes());
        buf.extend_from_slice(&tile.jpeg_data);
    }

    Ok(buf)
}

// ============================================================================
// Input Simulation (unchanged from original)
// ============================================================================

/// Send mouse input
pub fn send_mouse_input(x: u16, y: u16, action: &str, scroll_delta: Option<i16>) -> (bool, CommandResponseData) {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;

    unsafe {
        let mut inputs: Vec<INPUT> = Vec::new();

        if action == "move" || action.contains("down") || action.contains("up") {
            inputs.push(INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx: x as i32,
                        dy: y as i32,
                        mouseData: 0,
                        dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            });
        }

        match action {
            "left_down" => {
                inputs.push(INPUT {
                    r#type: INPUT_MOUSE,
                    Anonymous: INPUT_0 {
                        mi: MOUSEINPUT {
                            dx: 0, dy: 0, mouseData: 0,
                            dwFlags: MOUSEEVENTF_LEFTDOWN,
                            time: 0, dwExtraInfo: 0,
                        },
                    },
                });
            }
            "left_up" => {
                inputs.push(INPUT {
                    r#type: INPUT_MOUSE,
                    Anonymous: INPUT_0 {
                        mi: MOUSEINPUT {
                            dx: 0, dy: 0, mouseData: 0,
                            dwFlags: MOUSEEVENTF_LEFTUP,
                            time: 0, dwExtraInfo: 0,
                        },
                    },
                });
            }
            "right_down" => {
                inputs.push(INPUT {
                    r#type: INPUT_MOUSE,
                    Anonymous: INPUT_0 {
                        mi: MOUSEINPUT {
                            dx: 0, dy: 0, mouseData: 0,
                            dwFlags: MOUSEEVENTF_RIGHTDOWN,
                            time: 0, dwExtraInfo: 0,
                        },
                    },
                });
            }
            "right_up" => {
                inputs.push(INPUT {
                    r#type: INPUT_MOUSE,
                    Anonymous: INPUT_0 {
                        mi: MOUSEINPUT {
                            dx: 0, dy: 0, mouseData: 0,
                            dwFlags: MOUSEEVENTF_RIGHTUP,
                            time: 0, dwExtraInfo: 0,
                        },
                    },
                });
            }
            "middle_down" => {
                inputs.push(INPUT {
                    r#type: INPUT_MOUSE,
                    Anonymous: INPUT_0 {
                        mi: MOUSEINPUT {
                            dx: 0, dy: 0, mouseData: 0,
                            dwFlags: MOUSEEVENTF_MIDDLEDOWN,
                            time: 0, dwExtraInfo: 0,
                        },
                    },
                });
            }
            "middle_up" => {
                inputs.push(INPUT {
                    r#type: INPUT_MOUSE,
                    Anonymous: INPUT_0 {
                        mi: MOUSEINPUT {
                            dx: 0, dy: 0, mouseData: 0,
                            dwFlags: MOUSEEVENTF_MIDDLEUP,
                            time: 0, dwExtraInfo: 0,
                        },
                    },
                });
            }
            "scroll" => {
                if let Some(delta) = scroll_delta {
                    inputs.push(INPUT {
                        r#type: INPUT_MOUSE,
                        Anonymous: INPUT_0 {
                            mi: MOUSEINPUT {
                                dx: x as i32,
                                dy: y as i32,
                                mouseData: delta as u32,
                                dwFlags: MOUSEEVENTF_WHEEL | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                                time: 0,
                                dwExtraInfo: 0,
                            },
                        },
                    });
                }
            }
            _ => {}
        }

        if inputs.is_empty() {
            return (false, CommandResponseData::RemoteDesktopInputResult { success: false });
        }

        let sent = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
        (sent > 0, CommandResponseData::RemoteDesktopInputResult { success: sent > 0 })
    }
}

/// Send keyboard input
pub fn send_key_input(vk_code: u16, action: &str) -> (bool, CommandResponseData) {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;

    unsafe {
        let flags = match action {
            "down" => KEYBD_EVENT_FLAGS(0),
            "up" => KEYEVENTF_KEYUP,
            _ => return (false, CommandResponseData::RemoteDesktopInputResult { success: false }),
        };

        let input = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(vk_code),
                    wScan: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };

        let sent = SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
        (sent > 0, CommandResponseData::RemoteDesktopInputResult { success: sent > 0 })
    }
}

/// Send special key combinations
pub fn send_special_key(key: &str) -> (bool, CommandResponseData) {
    const VK_CONTROL: u16 = 0x11;
    const VK_MENU: u16 = 0x12;
    const VK_DELETE: u16 = 0x2E;
    const VK_TAB: u16 = 0x09;
    const VK_F4: u16 = 0x73;
    const VK_LWIN: u16 = 0x5B;
    const VK_ESCAPE: u16 = 0x1B;
    const VK_SNAPSHOT: u16 = 0x2C;

    match key {
        "ctrl_alt_del" => send_key_combo(&[VK_CONTROL, VK_MENU, VK_DELETE]),
        "alt_tab" => send_key_combo(&[VK_MENU, VK_TAB]),
        "alt_f4" => send_key_combo(&[VK_MENU, VK_F4]),
        "win" => send_key_combo(&[VK_LWIN]),
        "ctrl_esc" => send_key_combo(&[VK_CONTROL, VK_ESCAPE]),
        "print_screen" => send_key_combo(&[VK_SNAPSHOT]),
        _ => (false, CommandResponseData::Error {
            message: format!("Unknown special key: {}", key),
        }),
    }
}

fn send_key_combo(keys: &[u16]) -> (bool, CommandResponseData) {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;

    unsafe {
        let mut inputs: Vec<INPUT> = Vec::new();

        for &vk in keys {
            inputs.push(INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(vk),
                        wScan: 0,
                        dwFlags: KEYBD_EVENT_FLAGS(0),
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            });
        }

        for &vk in keys.iter().rev() {
            inputs.push(INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(vk),
                        wScan: 0,
                        dwFlags: KEYEVENTF_KEYUP,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            });
        }

        let sent = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
        (sent > 0, CommandResponseData::RemoteDesktopInputResult { success: sent > 0 })
    }
}
