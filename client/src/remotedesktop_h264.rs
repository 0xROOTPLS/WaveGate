//! H.264-based Remote Desktop streaming module.
//!
//! Uses DXGI Desktop Duplication for capture combined with hardware-accelerated
//! H.264 encoding via Windows Media Foundation for smooth, low-latency streaming.
//!
//! Benefits over JPEG tile-based streaming:
//! - Inter-frame compression (P-frames reference previous frames)
//! - Motion estimation (smooth window dragging, video playback)
//! - 3-5x lower bandwidth at same quality
//! - Hardware acceleration on Intel/NVIDIA/AMD GPUs

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use tokio::sync::mpsc;

use crate::dxgi::{DxgiCapturer, CaptureError};
use crate::h264_encoder::{H264Encoder, EncoderConfig, EncoderError};

/// H.264 stream state
struct H264StreamState {
    running: Arc<AtomicBool>,
    frame_tx: Option<mpsc::UnboundedSender<H264Frame>>,
}

impl Default for H264StreamState {
    fn default() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            frame_tx: None,
        }
    }
}

static H264_STATE: Lazy<Mutex<H264StreamState>> = Lazy::new(|| Mutex::new(H264StreamState::default()));

/// Encoded H.264 frame ready for transmission
#[derive(Debug, Clone)]
pub struct H264Frame {
    /// NAL unit data (Annex B format with start codes)
    pub data: Vec<u8>,
    /// Whether this is a keyframe (IDR)
    pub is_keyframe: bool,
    /// Frame timestamp in milliseconds
    pub timestamp_ms: u64,
    /// Screen width
    pub width: u16,
    /// Screen height
    pub height: u16,
}

impl H264Frame {
    /// Serialize to binary format for transmission
    /// Format: [width:u16][height:u16][is_keyframe:u8][timestamp_ms:u64][data_len:u32][data...]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(17 + self.data.len());
        buf.extend_from_slice(&self.width.to_le_bytes());
        buf.extend_from_slice(&self.height.to_le_bytes());
        buf.push(if self.is_keyframe { 1 } else { 0 });
        buf.extend_from_slice(&self.timestamp_ms.to_le_bytes());
        buf.extend_from_slice(&(self.data.len() as u32).to_le_bytes());
        buf.extend_from_slice(&self.data);
        buf
    }

    /// Deserialize from binary format
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 17 {
            return None;
        }

        let width = u16::from_le_bytes([data[0], data[1]]);
        let height = u16::from_le_bytes([data[2], data[3]]);
        let is_keyframe = data[4] != 0;
        let timestamp_ms = u64::from_le_bytes([
            data[5], data[6], data[7], data[8],
            data[9], data[10], data[11], data[12],
        ]);
        let data_len = u32::from_le_bytes([data[13], data[14], data[15], data[16]]) as usize;

        if data.len() < 17 + data_len {
            return None;
        }

        Some(H264Frame {
            data: data[17..17 + data_len].to_vec(),
            is_keyframe,
            timestamp_ms,
            width,
            height,
        })
    }
}

/// H.264 streaming configuration
#[derive(Debug, Clone)]
pub struct H264StreamConfig {
    /// Target FPS (1-60)
    pub fps: u8,
    /// Target bitrate in Mbps (1-50)
    pub bitrate_mbps: u8,
    /// Keyframe interval in seconds (1-10)
    pub keyframe_interval_secs: u8,
    /// Use low-latency mode
    pub low_latency: bool,
}

impl Default for H264StreamConfig {
    fn default() -> Self {
        Self {
            fps: 30,
            bitrate_mbps: 4,
            keyframe_interval_secs: 2,
            low_latency: true,
        }
    }
}

/// Start result containing screen dimensions
pub struct StartResult {
    pub width: u16,
    pub height: u16,
    pub is_hardware: bool,
}

/// Start H.264 remote desktop streaming
pub fn start_h264_stream(
    config: H264StreamConfig,
    frame_tx: mpsc::UnboundedSender<H264Frame>,
) -> Result<StartResult, String> {
    let mut state = H264_STATE.lock();

    if state.running.load(Ordering::SeqCst) {
        if let Some(ref old_tx) = state.frame_tx {
            if old_tx.is_closed() {
                state.running.store(false, Ordering::SeqCst);
                state.frame_tx = None;
            } else {
                return Err("H.264 stream already running".to_string());
            }
        } else {
            state.running.store(false, Ordering::SeqCst);
        }
    }

    // Get screen dimensions first
    let (screen_width, screen_height) = get_screen_dimensions();
    if screen_width == 0 || screen_height == 0 {
        return Err("Failed to get screen dimensions".to_string());
    }

    // Try to create H.264 encoder to verify it works
    let fps = config.fps.max(1).min(60) as u32;
    let bitrate = (config.bitrate_mbps.max(1).min(50) as u32) * 1_000_000;
    let keyframe_interval = fps * config.keyframe_interval_secs.max(1).min(10) as u32;

    let encoder_config = EncoderConfig {
        width: screen_width as u32,
        height: screen_height as u32,
        fps,
        bitrate,
        keyframe_interval,
        low_latency: config.low_latency,
        prefer_hardware: true,
    };

    // Create encoder to check if it works (will be recreated in thread)
    let test_encoder = H264Encoder::new(encoder_config.clone())
        .map_err(|e| format!("Failed to create H.264 encoder: {}", e))?;
    let is_hardware = test_encoder.is_hardware();
    drop(test_encoder);

    state.running = Arc::new(AtomicBool::new(true));
    state.frame_tx = Some(frame_tx);
    let running = state.running.clone();
    let tx = state.frame_tx.clone();

    drop(state);

    // Spawn capture thread
    std::thread::spawn(move || {
        if let Err(e) = capture_loop_h264(encoder_config, running, tx) {
            eprintln!("H.264 capture loop error: {}", e);
        }
    });

    Ok(StartResult {
        width: screen_width as u16,
        height: screen_height as u16,
        is_hardware,
    })
}

/// Stop H.264 streaming
pub fn stop_h264_stream() -> Result<(), String> {
    let mut state = H264_STATE.lock();

    if !state.running.load(Ordering::SeqCst) {
        return Err("No H.264 stream running".to_string());
    }

    state.running.store(false, Ordering::SeqCst);
    state.frame_tx = None;

    Ok(())
}

/// Check if H.264 streaming is active
pub fn is_h264_streaming() -> bool {
    H264_STATE.lock().running.load(Ordering::SeqCst)
}

/// Get screen dimensions from DXGI (primary monitor only)
fn get_screen_dimensions() -> (i32, i32) {
    // Create a temporary capturer to get the actual capture dimensions
    // This ensures we match what DXGI will actually capture
    match DxgiCapturer::new() {
        Ok(capturer) => {
            let (w, h) = capturer.dimensions();
            (w as i32, h as i32)
        }
        Err(_) => {
            // Fallback to primary monitor dimensions
            use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
            unsafe {
                (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN))
            }
        }
    }
}

/// H.264 capture and encoding loop
fn capture_loop_h264(
    encoder_config: EncoderConfig,
    running: Arc<AtomicBool>,
    frame_tx: Option<mpsc::UnboundedSender<H264Frame>>,
) -> Result<(), String> {
    let tx = frame_tx.ok_or("No frame sender")?;

    let fps = encoder_config.fps;
    let frame_interval = Duration::from_millis(1000 / fps as u64);
    let width = encoder_config.width as u16;
    let height = encoder_config.height as u16;

    // Create DXGI capturer
    let mut capturer = DxgiCapturer::new()?;

    // Create H.264 encoder
    let mut encoder = H264Encoder::new(encoder_config)
        .map_err(|e| format!("Failed to create encoder: {}", e))?;

    println!(
        "H.264 encoder started: {}x{} @ {}fps, hardware={}",
        width, height, fps,
        if encoder.is_hardware() { "yes" } else { "no" }
    );

    let stream_start = Instant::now();
    let mut frame_count = 0u64;
    let keyframe_interval = encoder.config().keyframe_interval;

    // Adaptive timeout based on FPS
    let timeout_ms = (1000 / fps).max(16).min(50);

    while running.load(Ordering::SeqCst) {
        let frame_start = Instant::now();

        // Capture frame
        let frame = match capturer.capture_frame(timeout_ms) {
            Ok(f) => f,
            Err(CaptureError::Timeout) | Err(CaptureError::NoFrame) => {
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

        // Force keyframe at interval or on first frame
        let force_keyframe = frame_count == 0 || frame_count % keyframe_interval as u64 == 0;
        frame_count += 1;

        // Encode frame (BGRA -> H.264)
        match encoder.encode_bgra(frame.data, force_keyframe) {
            Ok(Some(encoded)) => {
                let timestamp_ms = stream_start.elapsed().as_millis() as u64;

                let h264_frame = H264Frame {
                    data: encoded.data,
                    is_keyframe: encoded.is_keyframe,
                    timestamp_ms,
                    width,
                    height,
                };

                if tx.send(h264_frame).is_err() {
                    break;
                }
            }
            Ok(None) => {
                // Encoder needs more input before producing output
                // This is normal for B-frames or when encoder is buffering
            }
            Err(e) => {
                eprintln!("Encode error: {}", e);
                // Try to recover by requesting a keyframe next time
            }
        }

        // Maintain frame rate
        let elapsed = frame_start.elapsed();
        if elapsed < frame_interval {
            std::thread::sleep(frame_interval - elapsed);
        }
    }

    // Flush encoder
    if let Ok(remaining) = encoder.flush() {
        let timestamp_ms = stream_start.elapsed().as_millis() as u64;
        for encoded in remaining {
            let h264_frame = H264Frame {
                data: encoded.data,
                is_keyframe: encoded.is_keyframe,
                timestamp_ms,
                width,
                height,
            };
            let _ = tx.send(h264_frame);
        }
    }

    running.store(false, Ordering::SeqCst);
    println!("H.264 encoder stopped after {} frames", frame_count);
    Ok(())
}

/// Quality presets for H.264 streaming
#[derive(Debug, Clone, Copy)]
pub enum QualityPreset {
    /// Low bandwidth (~1 Mbps), good for slow connections
    Low,
    /// Medium quality (~3 Mbps), balanced
    Medium,
    /// High quality (~6 Mbps), good for LAN
    High,
    /// Maximum quality (~12 Mbps), best visual fidelity
    Ultra,
}

impl QualityPreset {
    pub fn to_config(self, fps: u8) -> H264StreamConfig {
        match self {
            QualityPreset::Low => H264StreamConfig {
                fps: fps.min(20),
                bitrate_mbps: 1,
                keyframe_interval_secs: 3,
                low_latency: true,
            },
            QualityPreset::Medium => H264StreamConfig {
                fps: fps.min(30),
                bitrate_mbps: 3,
                keyframe_interval_secs: 2,
                low_latency: true,
            },
            QualityPreset::High => H264StreamConfig {
                fps: fps.min(60),
                bitrate_mbps: 6,
                keyframe_interval_secs: 2,
                low_latency: true,
            },
            QualityPreset::Ultra => H264StreamConfig {
                fps,
                bitrate_mbps: 12,
                keyframe_interval_secs: 2,
                low_latency: true,
            },
        }
    }
}
