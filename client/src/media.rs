//! Media capture module for webcam and audio streaming.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use cpal::traits::{DeviceTrait, HostTrait};
use image::imageops::FilterType;
use image::{ImageBuffer, Rgb, RgbImage};
use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::{CameraIndex, RequestedFormat, RequestedFormatType, FrameFormat, Resolution, CameraFormat};
use nokhwa::Camera;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use tokio::sync::mpsc;

use wavegate_shared::{BinaryMediaFrame, CommandResponseData, MediaDeviceInfo};

/// Media stream state
struct MediaStreamState {
    /// Whether streaming is active
    running: Arc<AtomicBool>,
    /// Frame sender (to main connection loop) - sends raw binary frames
    frame_tx: Option<mpsc::UnboundedSender<BinaryMediaFrame>>,
}

impl Default for MediaStreamState {
    fn default() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            frame_tx: None,
        }
    }
}

/// Parse resolution string to (width, height) or None for native
fn parse_resolution(res: Option<&str>) -> Option<(u32, u32)> {
    match res {
        Some("1080p") => Some((1920, 1080)),
        Some("720p") => Some((1280, 720)),
        Some("480p") => Some((854, 480)),
        Some("360p") => Some((640, 360)),
        Some("240p") => Some((426, 240)),
        _ => None, // native resolution
    }
}

static STREAM_STATE: Lazy<Mutex<MediaStreamState>> = Lazy::new(|| Mutex::new(MediaStreamState::default()));

/// List available video devices (webcams)
pub fn list_video_devices() -> Vec<MediaDeviceInfo> {
    let mut devices = Vec::new();

    // Query available cameras using nokhwa
    if let Ok(cameras) = nokhwa::query(nokhwa::utils::ApiBackend::Auto) {
        for cam in cameras {
            devices.push(MediaDeviceInfo {
                id: format!("{}", cam.index()),
                name: cam.human_name().to_string(),
            });
        }
    }

    devices
}

/// List available audio input devices
pub fn list_audio_devices() -> Vec<MediaDeviceInfo> {
    let mut devices = Vec::new();

    // Use cpal to enumerate audio input devices
    let host = cpal::default_host();
    if let Ok(input_devices) = host.input_devices() {
        for (idx, device) in input_devices.enumerate() {
            let name = device.name().unwrap_or_else(|_| format!("Audio Device {}", idx));
            devices.push(MediaDeviceInfo {
                id: format!("{}", idx),
                name,
            });
        }
    }

    devices
}

/// Get all media devices
pub fn get_media_devices() -> (bool, CommandResponseData) {
    let video_devices = list_video_devices();
    let audio_devices = list_audio_devices();

    (true, CommandResponseData::MediaDevices {
        video_devices,
        audio_devices,
    })
}

/// Start media streaming
pub fn start_media_stream(
    video_device: Option<String>,
    _audio_device: Option<String>,
    fps: u8,
    quality: u8,
    resolution: Option<String>,
    frame_tx: mpsc::UnboundedSender<BinaryMediaFrame>,
) -> (bool, CommandResponseData) {
    let mut state = STREAM_STATE.lock();

    // Check if already streaming - but also verify the sender is still valid
    if state.running.load(Ordering::SeqCst) {
        // Check if previous stream is actually still alive by testing the sender
        if let Some(ref old_tx) = state.frame_tx {
            if old_tx.is_closed() {
                // Previous receiver was dropped, reset stale state
                state.running.store(false, Ordering::SeqCst);
                state.frame_tx = None;
            } else {
                return (false, CommandResponseData::Error {
                    message: "Stream already running".to_string(),
                });
            }
        } else {
            // running is true but no sender - stale state, reset it
            state.running.store(false, Ordering::SeqCst);
        }
    }

    // Create a fresh AtomicBool for this session to avoid stale Arc references
    state.running = Arc::new(AtomicBool::new(true));
    state.frame_tx = Some(frame_tx);
    let running = state.running.clone();
    let tx = state.frame_tx.clone();

    drop(state); // Release lock before spawning thread

    // Determine camera index
    let camera_idx = video_device
        .as_ref()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);

    let fps = fps.max(1).min(60) as u32; // Allow up to 60fps now
    let quality = quality.max(10).min(100);
    let target_res = parse_resolution(resolution.as_deref());

    // Spawn capture thread
    std::thread::spawn(move || {
        capture_loop(camera_idx, fps, quality, target_res, running, tx);
    });

    (true, CommandResponseData::MediaStreamStarted)
}

/// Stop media streaming
pub fn stop_media_stream() -> (bool, CommandResponseData) {
    let mut state = STREAM_STATE.lock();

    if !state.running.load(Ordering::SeqCst) {
        return (false, CommandResponseData::Error {
            message: "No stream running".to_string(),
        });
    }

    state.running.store(false, Ordering::SeqCst);
    state.frame_tx = None;

    (true, CommandResponseData::MediaStreamStopped)
}

/// Check if media stream is currently running
pub fn is_streaming() -> bool {
    STREAM_STATE.lock().running.load(Ordering::SeqCst)
}

/// Main capture loop (runs in separate thread)
fn capture_loop(
    camera_idx: u32,
    fps: u32,
    quality: u8,
    target_resolution: Option<(u32, u32)>,
    running: Arc<AtomicBool>,
    frame_tx: Option<mpsc::UnboundedSender<BinaryMediaFrame>>,
) {
    let tx = match frame_tx {
        Some(tx) => tx,
        None => return,
    };

    // Open camera - try to get MJPEG at target resolution for zero-encode capture
    let index = CameraIndex::Index(camera_idx);
    let (req_width, req_height) = target_resolution.unwrap_or((1280, 720));

    // Request MJPEG format directly - camera hardware does the compression
    let mjpeg_format = CameraFormat::new(
        Resolution::new(req_width, req_height),
        FrameFormat::MJPEG,
        fps,
    );
    let requested = RequestedFormat::with_formats(RequestedFormatType::Closest(mjpeg_format), &[FrameFormat::MJPEG]);

    let mut camera = match Camera::new(index.clone(), requested) {
        Ok(cam) => cam,
        Err(_) => {
            // Fallback to RGB if MJPEG not supported
            let requested = RequestedFormat::new::<RgbFormat>(RequestedFormatType::AbsoluteHighestFrameRate);
            match Camera::new(index, requested) {
                Ok(cam) => cam,
                Err(_) => {
                    running.store(false, Ordering::SeqCst);
                    return;
                }
            }
        }
    };

    // Check if we got MJPEG format
    let camera_format = camera.camera_format();
    let is_mjpeg = camera_format.format() == FrameFormat::MJPEG;

    // Start camera stream
    if camera.open_stream().is_err() {
        running.store(false, Ordering::SeqCst);
        return;
    }

    let frame_interval = std::time::Duration::from_millis(1000 / fps as u64);
    let start_time = Instant::now();

    let mut frame_count = 0u32;
    let mut total_bytes = 0usize;
    let mut total_frame_ms = 0u128;

    while running.load(Ordering::SeqCst) {
        let frame_start = Instant::now();

        match camera.frame() {
            Ok(frame) => {
                let resolution = frame.resolution();
                let src_width = resolution.width();
                let src_height = resolution.height();

                // If MJPEG, use raw buffer directly - no decode/encode needed!
                let jpeg_data = if is_mjpeg {
                    Some(frame.buffer().to_vec())
                } else {
                    // RGB path - need to decode and encode
                    match frame.decode_image::<RgbFormat>() {
                        Ok(rgb) => {
                            let (out_width, out_height) = target_resolution.unwrap_or((src_width, src_height));
                            if src_width != out_width || src_height != out_height {
                                if let Some(img) = ImageBuffer::<Rgb<u8>, _>::from_raw(src_width, src_height, rgb.to_vec()) {
                                    let resized = image::imageops::resize(&img, out_width, out_height, FilterType::Nearest);
                                    encode_jpeg_from_image(&resized, quality)
                                } else {
                                    None
                                }
                            } else {
                                encode_jpeg(&rgb, src_width, src_height, quality)
                            }
                        }
                        Err(_) => None
                    }
                };

                if let Some(jpeg_data) = jpeg_data {
                    let timestamp_ms = start_time.elapsed().as_millis() as u64;
                    let (out_width, out_height) = if is_mjpeg {
                        (src_width, src_height)
                    } else {
                        target_resolution.unwrap_or((src_width, src_height))
                    };

                    let msg = BinaryMediaFrame {
                        timestamp_ms,
                        width: out_width as u16,
                        height: out_height as u16,
                        jpeg_data: jpeg_data.clone(),
                    };

                    if tx.send(msg).is_err() {
                        break;
                    }

                    frame_count += 1;
                    total_bytes += jpeg_data.len();
                }
            }
            Err(_) => {}
        }

        // Maintain frame rate
        let elapsed = frame_start.elapsed();
        if elapsed < frame_interval {
            std::thread::sleep(frame_interval - elapsed);
        }
    }

    // Cleanup
    let _ = camera.stop_stream();
    running.store(false, Ordering::SeqCst);
}

/// Encode RGB buffer to JPEG using mozjpeg (SIMD-optimized)
fn encode_jpeg(rgb_data: &[u8], width: u32, height: u32, quality: u8) -> Option<Vec<u8>> {
    std::panic::catch_unwind(|| {
        let mut comp = mozjpeg::Compress::new(mozjpeg::ColorSpace::JCS_RGB);
        comp.set_size(width as usize, height as usize);
        comp.set_quality(quality as f32);
        comp.set_fastest_defaults();

        let mut comp = comp.start_compress(Vec::new()).ok()?;
        comp.write_scanlines(rgb_data).ok()?;
        comp.finish().ok()
    }).ok().flatten()
}

/// Encode an RgbImage to JPEG
fn encode_jpeg_from_image(img: &RgbImage, quality: u8) -> Option<Vec<u8>> {
    encode_jpeg(img.as_raw(), img.width(), img.height(), quality)
}
