//! Hardware-accelerated H.264 encoder using Windows Media Foundation.
//!
//! Uses the MFT (Media Foundation Transform) pipeline to leverage hardware
//! encoders (Intel QSV, NVIDIA NVENC, AMD AMF) when available, with automatic
//! fallback to software encoding.
//!
//! The encoder accepts BGRA frames from DXGI capture and outputs H.264 NAL units.

use std::ptr;

use windows::core::{Interface, GUID};
#[allow(unused_imports)]
use windows::Win32::Foundation::{E_NOTIMPL, S_OK};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

/// H.264 encoder configuration
#[derive(Debug, Clone)]
pub struct EncoderConfig {
    /// Frame width
    pub width: u32,
    /// Frame height
    pub height: u32,
    /// Target frames per second
    pub fps: u32,
    /// Target bitrate in bits per second (e.g., 4_000_000 for 4 Mbps)
    pub bitrate: u32,
    /// Keyframe interval in frames (GOP size)
    pub keyframe_interval: u32,
    /// Use low-latency mode (reduces buffering, important for remote desktop)
    pub low_latency: bool,
    /// Prefer hardware encoder
    pub prefer_hardware: bool,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            fps: 30,
            bitrate: 4_000_000, // 4 Mbps
            keyframe_interval: 60, // Keyframe every 2 seconds at 30fps
            low_latency: true,
            prefer_hardware: true,
        }
    }
}

/// Encoded H.264 frame
#[derive(Debug, Clone)]
pub struct EncodedFrame {
    /// NAL unit data (may contain multiple NAL units)
    pub data: Vec<u8>,
    /// Whether this is a keyframe (IDR)
    pub is_keyframe: bool,
    /// Presentation timestamp in 100-nanosecond units
    pub timestamp: i64,
    /// Duration in 100-nanosecond units
    pub duration: i64,
}

/// H.264 encoder errors
#[derive(Debug)]
pub enum EncoderError {
    /// COM/MF initialization failed
    InitFailed(String),
    /// No suitable encoder found
    NoEncoder(String),
    /// Encoder configuration failed
    ConfigFailed(String),
    /// Encoding failed
    EncodeFailed(String),
    /// Input format not supported
    FormatNotSupported(String),
}

impl std::fmt::Display for EncoderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EncoderError::InitFailed(s) => write!(f, "Init failed: {}", s),
            EncoderError::NoEncoder(s) => write!(f, "No encoder: {}", s),
            EncoderError::ConfigFailed(s) => write!(f, "Config failed: {}", s),
            EncoderError::EncodeFailed(s) => write!(f, "Encode failed: {}", s),
            EncoderError::FormatNotSupported(s) => write!(f, "Format not supported: {}", s),
        }
    }
}

/// Hardware H.264 encoder using Media Foundation
pub struct H264Encoder {
    transform: IMFTransform,
    input_type: IMFMediaType,
    output_type: IMFMediaType,
    config: EncoderConfig,
    frame_count: u64,
    is_hardware: bool,
    started: bool,
}

// MF GUIDs we need
const MF_MT_MAJOR_TYPE: GUID = GUID::from_u128(0x48eba18e_f8c9_4687_bf11_0a74c9f96a8f);
const MF_MT_SUBTYPE: GUID = GUID::from_u128(0xf7e34c9a_42e8_4714_b74b_cb29d72c35e5);
const MF_MT_AVG_BITRATE: GUID = GUID::from_u128(0x20332624_fb0d_4d9e_bd0d_cbf6786c102e);
const MF_MT_INTERLACE_MODE: GUID = GUID::from_u128(0xe2724bb8_e676_4806_b4b2_a8d6efb44ccd);
const MF_MT_FRAME_SIZE: GUID = GUID::from_u128(0x1652c33d_d6b2_4012_b834_72030849a37d);
const MF_MT_FRAME_RATE: GUID = GUID::from_u128(0xc459a2e8_3d2c_4e44_b132_fee5156c7bb0);
const MF_MT_PIXEL_ASPECT_RATIO: GUID = GUID::from_u128(0xc6376a1e_8d0a_4027_be45_6d9a0ad39bb6);

const MFMediaType_Video: GUID = GUID::from_u128(0x73646976_0000_0010_8000_00aa00389b71);
const MFVideoFormat_H264: GUID = GUID::from_u128(0x34363248_0000_0010_8000_00aa00389b71);
const MFVideoFormat_NV12: GUID = GUID::from_u128(0x3231564e_0000_0010_8000_00aa00389b71);
#[allow(dead_code)]
const MFVideoFormat_ARGB32: GUID = GUID::from_u128(0x00000015_0000_0010_8000_00aa00389b71);
#[allow(dead_code)]
const MFVideoFormat_RGB32: GUID = GUID::from_u128(0x00000016_0000_0010_8000_00aa00389b71);

// ICodecAPI GUIDs - kept for future use when PROPVARIANT construction is added
#[allow(dead_code)]
const CODECAPI_AVEncCommonRateControlMode: GUID = GUID::from_u128(0x1c0608e9_370c_4710_8a58_cb6181c42423);
#[allow(dead_code)]
const CODECAPI_AVEncCommonQuality: GUID = GUID::from_u128(0xfcbf57a3_7ea5_4b0c_9644_69b40c39c391);
#[allow(dead_code)]
const CODECAPI_AVEncCommonLowLatency: GUID = GUID::from_u128(0x9d3ecd55_89e8_490a_970a_0c9548d5a56e);
#[allow(dead_code)]
const CODECAPI_AVEncMPVGOPSize: GUID = GUID::from_u128(0x95f31b26_95a4_41aa_9303_246a7fc6eef1);

#[allow(dead_code)]
const EAVENC_COMMON_RATE_CONTROL_MODE_CBR: u32 = 1;
#[allow(dead_code)]
const EAVENC_COMMON_RATE_CONTROL_MODE_QUALITY: u32 = 2;

impl H264Encoder {
    /// Create a new H.264 encoder with the given configuration
    pub fn new(config: EncoderConfig) -> Result<Self, EncoderError> {
        unsafe {
            // Initialize COM
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

            // Initialize Media Foundation
            MFStartup(MF_VERSION, MFSTARTUP_FULL)
                .map_err(|e| EncoderError::InitFailed(format!("MFStartup failed: {:?}", e)))?;

            // Find H.264 encoder
            let (transform, is_hardware) = Self::find_encoder(&config)?;

            // Create media types
            let input_type: IMFMediaType = MFCreateMediaType()
                .map_err(|e| EncoderError::InitFailed(format!("Failed to create input type: {:?}", e)))?;
            let output_type: IMFMediaType = MFCreateMediaType()
                .map_err(|e| EncoderError::InitFailed(format!("Failed to create output type: {:?}", e)))?;

            let mut encoder = Self {
                transform,
                input_type,
                output_type,
                config,
                frame_count: 0,
                is_hardware,
                started: false,
            };

            encoder.configure()?;

            Ok(encoder)
        }
    }

    /// Find a suitable H.264 encoder (prefer hardware)
    unsafe fn find_encoder(config: &EncoderConfig) -> Result<(IMFTransform, bool), EncoderError> {
        // Set up search parameters for H.264 video encoder
        let mut output_type: MFT_REGISTER_TYPE_INFO = MFT_REGISTER_TYPE_INFO {
            guidMajorType: MFMediaType_Video,
            guidSubtype: MFVideoFormat_H264,
        };

        let flags = if config.prefer_hardware {
            (MFT_ENUM_FLAG_HARDWARE.0 | MFT_ENUM_FLAG_SORTANDFILTER.0) as u32
        } else {
            MFT_ENUM_FLAG_SORTANDFILTER.0 as u32
        };

        // Enumerate encoders
        let mut count: u32 = 0;
        let mut clsids: *mut GUID = ptr::null_mut();

        MFTEnum(
            MFT_CATEGORY_VIDEO_ENCODER,
            flags,
            None,
            Some(&output_type),
            None,
            &mut clsids,
            &mut count,
        ).map_err(|e| EncoderError::NoEncoder(format!("MFTEnum failed: {:?}", e)))?;

        if count == 0 || clsids.is_null() {
            return Err(EncoderError::NoEncoder("No H.264 encoders found".into()));
        }

        // Try each encoder until one works
        let clsid_slice = std::slice::from_raw_parts(clsids, count as usize);
        let mut last_error = String::new();
        let mut is_hardware = config.prefer_hardware;

        for (i, clsid) in clsid_slice.iter().enumerate() {
            match Self::create_transform(clsid) {
                Ok(transform) => {
                    // Check if this is actually a hardware encoder
                    is_hardware = i == 0 && config.prefer_hardware;

                    // Free the CLSID array
                    windows::Win32::System::Com::CoTaskMemFree(Some(clsids as *const _));

                    return Ok((transform, is_hardware));
                }
                Err(e) => {
                    last_error = e;
                    continue;
                }
            }
        }

        // Free the CLSID array
        windows::Win32::System::Com::CoTaskMemFree(Some(clsids as *const _));

        // If hardware failed and we prefer hardware, try software
        if config.prefer_hardware {
            let sw_flags = MFT_ENUM_FLAG_SORTANDFILTER.0 as u32;

            MFTEnum(
                MFT_CATEGORY_VIDEO_ENCODER,
                sw_flags,
                None,
                Some(&output_type),
                None,
                &mut clsids,
                &mut count,
            ).map_err(|e| EncoderError::NoEncoder(format!("MFTEnum (software) failed: {:?}", e)))?;

            if count > 0 && !clsids.is_null() {
                let clsid_slice = std::slice::from_raw_parts(clsids, count as usize);
                for clsid in clsid_slice {
                    if let Ok(transform) = Self::create_transform(clsid) {
                        windows::Win32::System::Com::CoTaskMemFree(Some(clsids as *const _));
                        return Ok((transform, false));
                    }
                }
                windows::Win32::System::Com::CoTaskMemFree(Some(clsids as *const _));
            }
        }

        Err(EncoderError::NoEncoder(format!("All encoders failed: {}", last_error)))
    }

    /// Create transform from CLSID
    unsafe fn create_transform(clsid: &GUID) -> Result<IMFTransform, String> {
        let transform: IMFTransform = windows::Win32::System::Com::CoCreateInstance(
            clsid,
            None,
            windows::Win32::System::Com::CLSCTX_INPROC_SERVER,
        ).map_err(|e| format!("CoCreateInstance failed: {:?}", e))?;

        Ok(transform)
    }

    /// Configure the encoder with input/output types
    unsafe fn configure(&mut self) -> Result<(), EncoderError> {
        let width = self.config.width;
        let height = self.config.height;
        let fps = self.config.fps;
        let bitrate = self.config.bitrate;

        // Configure output type (H.264)
        self.output_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(|e| EncoderError::ConfigFailed(format!("SetGUID major type: {:?}", e)))?;
        self.output_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)
            .map_err(|e| EncoderError::ConfigFailed(format!("SetGUID subtype: {:?}", e)))?;
        self.output_type.SetUINT32(&MF_MT_AVG_BITRATE, bitrate)
            .map_err(|e| EncoderError::ConfigFailed(format!("SetUINT32 bitrate: {:?}", e)))?;
        self.output_type.SetUINT32(&MF_MT_INTERLACE_MODE, 2) // MFVideoInterlace_Progressive
            .map_err(|e| EncoderError::ConfigFailed(format!("SetUINT32 interlace: {:?}", e)))?;

        // Pack frame size (width in high 32 bits, height in low 32 bits)
        let frame_size = ((width as u64) << 32) | (height as u64);
        self.output_type.SetUINT64(&MF_MT_FRAME_SIZE, frame_size)
            .map_err(|e| EncoderError::ConfigFailed(format!("SetUINT64 frame size: {:?}", e)))?;

        // Pack frame rate (numerator in high 32 bits, denominator in low 32 bits)
        let frame_rate = ((fps as u64) << 32) | 1u64;
        self.output_type.SetUINT64(&MF_MT_FRAME_RATE, frame_rate)
            .map_err(|e| EncoderError::ConfigFailed(format!("SetUINT64 frame rate: {:?}", e)))?;

        // Pixel aspect ratio 1:1
        let par = (1u64 << 32) | 1u64;
        self.output_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, par)
            .map_err(|e| EncoderError::ConfigFailed(format!("SetUINT64 PAR: {:?}", e)))?;

        // Set output type on transform
        self.transform.SetOutputType(0, &self.output_type, 0)
            .map_err(|e| EncoderError::ConfigFailed(format!("SetOutputType: {:?}", e)))?;

        // Configure input type (NV12 - most hardware encoders prefer this)
        self.input_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(|e| EncoderError::ConfigFailed(format!("SetGUID input major: {:?}", e)))?;
        self.input_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)
            .map_err(|e| EncoderError::ConfigFailed(format!("SetGUID input subtype: {:?}", e)))?;
        self.input_type.SetUINT32(&MF_MT_INTERLACE_MODE, 2)
            .map_err(|e| EncoderError::ConfigFailed(format!("SetUINT32 input interlace: {:?}", e)))?;
        self.input_type.SetUINT64(&MF_MT_FRAME_SIZE, frame_size)
            .map_err(|e| EncoderError::ConfigFailed(format!("SetUINT64 input frame size: {:?}", e)))?;
        self.input_type.SetUINT64(&MF_MT_FRAME_RATE, frame_rate)
            .map_err(|e| EncoderError::ConfigFailed(format!("SetUINT64 input frame rate: {:?}", e)))?;
        self.input_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, par)
            .map_err(|e| EncoderError::ConfigFailed(format!("SetUINT64 input PAR: {:?}", e)))?;

        // Set input type on transform
        self.transform.SetInputType(0, &self.input_type, 0)
            .map_err(|e| EncoderError::ConfigFailed(format!("SetInputType: {:?}", e)))?;

        // Try to set encoder properties via ICodecAPI
        // Note: ICodecAPI::SetValue requires VARIANT, which is complex to construct.
        // For now, we rely on the MFT defaults and output type settings.
        // The low-latency and GOP size settings are best-effort.
        let _ = self.transform.cast::<ICodecAPI>(); // Just check if available

        Ok(())
    }

    /// Start the encoder (must be called before encoding frames)
    pub fn start(&mut self) -> Result<(), EncoderError> {
        if self.started {
            return Ok(());
        }

        unsafe {
            // Get stream info to verify encoder is ready
            let mut input_info = MFT_INPUT_STREAM_INFO::default();
            self.transform.GetInputStreamInfo(0, &mut input_info)
                .map_err(|e| EncoderError::InitFailed(format!("GetInputStreamInfo: {:?}", e)))?;
            let _output_info = self.transform.GetOutputStreamInfo(0)
                .map_err(|e| EncoderError::InitFailed(format!("GetOutputStreamInfo: {:?}", e)))?;
            let _ = input_info; // suppress unused warning

            // Send stream begin message
            self.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .map_err(|e| EncoderError::InitFailed(format!("BEGIN_STREAMING: {:?}", e)))?;
            self.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                .map_err(|e| EncoderError::InitFailed(format!("START_OF_STREAM: {:?}", e)))?;
        }

        self.started = true;
        Ok(())
    }

    /// Encode a BGRA frame to H.264
    ///
    /// The input is expected to be BGRA data (4 bytes per pixel) with the specified
    /// width and height. The encoder will convert to NV12 internally.
    pub fn encode_bgra(&mut self, bgra_data: &[u8], force_keyframe: bool) -> Result<Option<EncodedFrame>, EncoderError> {
        if !self.started {
            self.start()?;
        }

        let width = self.config.width as usize;
        let height = self.config.height as usize;
        let expected_size = width * height * 4;

        if bgra_data.len() < expected_size {
            return Err(EncoderError::EncodeFailed(format!(
                "BGRA data too small: {} < {}", bgra_data.len(), expected_size
            )));
        }

        // Convert BGRA to NV12
        let nv12_data = bgra_to_nv12(bgra_data, width, height);

        // Encode NV12
        self.encode_nv12(&nv12_data, force_keyframe)
    }

    /// Encode an NV12 frame to H.264
    pub fn encode_nv12(&mut self, nv12_data: &[u8], force_keyframe: bool) -> Result<Option<EncodedFrame>, EncoderError> {
        if !self.started {
            self.start()?;
        }

        let width = self.config.width as usize;
        let height = self.config.height as usize;
        let expected_size = width * height * 3 / 2; // NV12: Y plane + interleaved UV

        if nv12_data.len() < expected_size {
            return Err(EncoderError::EncodeFailed(format!(
                "NV12 data too small: {} < {}", nv12_data.len(), expected_size
            )));
        }

        unsafe {
            // Create input sample
            let sample: IMFSample = MFCreateSample()
                .map_err(|e| EncoderError::EncodeFailed(format!("MFCreateSample: {:?}", e)))?;

            // Create media buffer
            let buffer: IMFMediaBuffer = MFCreateMemoryBuffer(nv12_data.len() as u32)
                .map_err(|e| EncoderError::EncodeFailed(format!("MFCreateMemoryBuffer: {:?}", e)))?;

            // Lock buffer and copy data
            let mut buffer_ptr: *mut u8 = ptr::null_mut();
            let mut max_len: u32 = 0;
            let mut current_len: u32 = 0;

            buffer.Lock(&mut buffer_ptr, Some(&mut max_len), Some(&mut current_len))
                .map_err(|e| EncoderError::EncodeFailed(format!("Lock: {:?}", e)))?;

            ptr::copy_nonoverlapping(nv12_data.as_ptr(), buffer_ptr, nv12_data.len());

            buffer.Unlock()
                .map_err(|e| EncoderError::EncodeFailed(format!("Unlock: {:?}", e)))?;

            buffer.SetCurrentLength(nv12_data.len() as u32)
                .map_err(|e| EncoderError::EncodeFailed(format!("SetCurrentLength: {:?}", e)))?;

            // Add buffer to sample
            sample.AddBuffer(&buffer)
                .map_err(|e| EncoderError::EncodeFailed(format!("AddBuffer: {:?}", e)))?;

            // Set sample time (100-nanosecond units)
            let fps = self.config.fps as i64;
            let frame_duration = 10_000_000 / fps; // 100ns units per frame
            let sample_time = self.frame_count as i64 * frame_duration;

            sample.SetSampleTime(sample_time)
                .map_err(|e| EncoderError::EncodeFailed(format!("SetSampleTime: {:?}", e)))?;
            sample.SetSampleDuration(frame_duration)
                .map_err(|e| EncoderError::EncodeFailed(format!("SetSampleDuration: {:?}", e)))?;

            self.frame_count += 1;

            // Request keyframe if needed
            if force_keyframe {
                // Try to force keyframe via sample attribute
                // MFSampleExtension_CleanPoint
                let clean_point_guid = GUID::from_u128(0x9cdf01d8_a0f0_43ba_b077_eaa06cbd728a);
                let _ = sample.SetUINT32(&clean_point_guid, 1);
            }

            // Process input
            let result = self.transform.ProcessInput(0, &sample, 0);
            if let Err(e) = result {
                // MF_E_NOTACCEPTING means we need to drain output first
                if e.code().0 as u32 != 0xC00D36B5 { // MF_E_NOTACCEPTING
                    return Err(EncoderError::EncodeFailed(format!("ProcessInput: {:?}", e)));
                }
            }

            // Try to get output
            self.get_output()
        }
    }

    /// Get encoded output from the encoder
    unsafe fn get_output(&mut self) -> Result<Option<EncodedFrame>, EncoderError> {
        use std::mem::ManuallyDrop;

        // Get output stream info
        let output_info = self.transform.GetOutputStreamInfo(0)
            .map_err(|e| EncoderError::EncodeFailed(format!("GetOutputStreamInfo: {:?}", e)))?;

        // Check if we need to provide the buffer
        let provides_samples = (output_info.dwFlags & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32)) != 0;

        // Create output buffer structure
        let mut output_buffer = MFT_OUTPUT_DATA_BUFFER {
            dwStreamID: 0,
            pSample: ManuallyDrop::new(None),
            dwStatus: 0,
            pEvents: ManuallyDrop::new(None),
        };

        if !provides_samples {
            // We need to provide the sample
            let sample: IMFSample = MFCreateSample()
                .map_err(|e| EncoderError::EncodeFailed(format!("MFCreateSample output: {:?}", e)))?;

            let buffer_size = output_info.cbSize.max(1024 * 1024); // At least 1MB
            let buffer: IMFMediaBuffer = MFCreateMemoryBuffer(buffer_size)
                .map_err(|e| EncoderError::EncodeFailed(format!("MFCreateMemoryBuffer output: {:?}", e)))?;

            sample.AddBuffer(&buffer)
                .map_err(|e| EncoderError::EncodeFailed(format!("AddBuffer output: {:?}", e)))?;

            output_buffer.pSample = ManuallyDrop::new(Some(sample));
        }

        // Get output
        let mut status: u32 = 0;
        let mut output_buffers = [output_buffer];
        let result = self.transform.ProcessOutput(
            0,
            &mut output_buffers,
            &mut status,
        );

        match result {
            Ok(_) => {
                // Got output - extract the data
                // Take the sample out of the ManuallyDrop wrapper
                let sample_opt = std::mem::replace(
                    &mut *output_buffers[0].pSample,
                    None
                );
                if let Some(sample) = sample_opt {
                    let buffer_count = sample.GetBufferCount()
                        .map_err(|e| EncoderError::EncodeFailed(format!("GetBufferCount: {:?}", e)))?;

                    if buffer_count == 0 {
                        return Ok(None);
                    }

                    let buffer: IMFMediaBuffer = sample.GetBufferByIndex(0)
                        .map_err(|e| EncoderError::EncodeFailed(format!("GetBufferByIndex: {:?}", e)))?;

                    let mut data_ptr: *mut u8 = ptr::null_mut();
                    let mut max_len: u32 = 0;
                    let mut current_len: u32 = 0;

                    buffer.Lock(&mut data_ptr, Some(&mut max_len), Some(&mut current_len))
                        .map_err(|e| EncoderError::EncodeFailed(format!("Lock output: {:?}", e)))?;

                    let data = std::slice::from_raw_parts(data_ptr, current_len as usize).to_vec();

                    buffer.Unlock()
                        .map_err(|e| EncoderError::EncodeFailed(format!("Unlock output: {:?}", e)))?;

                    // Get sample time and duration
                    let timestamp = sample.GetSampleTime().unwrap_or(0);
                    let duration = sample.GetSampleDuration().unwrap_or(0);

                    // Check if keyframe (look for IDR NAL unit or MFSampleExtension_CleanPoint)
                    // IMFSample inherits from IMFAttributes, so we can call GetUINT32
                    let clean_point_guid = GUID::from_u128(0x9cdf01d8_a0f0_43ba_b077_eaa06cbd728a);
                    let is_keyframe_attr = if let Ok(attrs) = sample.cast::<IMFAttributes>() {
                        attrs.GetUINT32(&clean_point_guid).unwrap_or(0) != 0
                    } else {
                        false
                    };
                    let is_keyframe = is_keyframe_attr || is_h264_keyframe(&data);

                    return Ok(Some(EncodedFrame {
                        data,
                        is_keyframe,
                        timestamp,
                        duration,
                    }));
                }
                Ok(None)
            }
            Err(e) => {
                // MF_E_TRANSFORM_NEED_MORE_INPUT is expected
                if e.code().0 as u32 == 0xC00D6D72 { // MF_E_TRANSFORM_NEED_MORE_INPUT
                    return Ok(None);
                }
                Err(EncoderError::EncodeFailed(format!("ProcessOutput: {:?}", e)))
            }
        }
    }

    /// Flush the encoder and get any remaining frames
    pub fn flush(&mut self) -> Result<Vec<EncodedFrame>, EncoderError> {
        if !self.started {
            return Ok(Vec::new());
        }

        unsafe {
            // Send drain message
            let _ = self.transform.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0);

            // Collect all remaining output
            let mut frames = Vec::new();
            loop {
                match self.get_output() {
                    Ok(Some(frame)) => frames.push(frame),
                    Ok(None) => break,
                    Err(_) => break,
                }
            }

            Ok(frames)
        }
    }

    /// Check if using hardware encoder
    pub fn is_hardware(&self) -> bool {
        self.is_hardware
    }

    /// Get encoder configuration
    pub fn config(&self) -> &EncoderConfig {
        &self.config
    }
}

impl Drop for H264Encoder {
    fn drop(&mut self) {
        unsafe {
            if self.started {
                let _ = self.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
                let _ = self.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
            }
        }
    }
}

/// Convert BGRA to NV12 format
/// NV12: Full Y plane followed by interleaved UV plane (half resolution)
fn bgra_to_nv12(bgra: &[u8], width: usize, height: usize) -> Vec<u8> {
    let y_size = width * height;
    let uv_size = width * height / 2;
    let mut nv12 = vec![0u8; y_size + uv_size];

    // Convert to Y plane
    for y in 0..height {
        for x in 0..width {
            let bgra_idx = (y * width + x) * 4;
            let b = bgra[bgra_idx] as f32;
            let g = bgra[bgra_idx + 1] as f32;
            let r = bgra[bgra_idx + 2] as f32;

            // BT.601 RGB to Y conversion
            let y_val = (0.299 * r + 0.587 * g + 0.114 * b) as u8;
            nv12[y * width + x] = y_val;
        }
    }

    // Convert to interleaved UV plane (2x2 subsampling)
    let uv_offset = y_size;
    for y in (0..height).step_by(2) {
        for x in (0..width).step_by(2) {
            // Average 2x2 block
            let mut r_sum = 0f32;
            let mut g_sum = 0f32;
            let mut b_sum = 0f32;

            for dy in 0..2 {
                for dx in 0..2 {
                    let bgra_idx = ((y + dy) * width + (x + dx)) * 4;
                    b_sum += bgra[bgra_idx] as f32;
                    g_sum += bgra[bgra_idx + 1] as f32;
                    r_sum += bgra[bgra_idx + 2] as f32;
                }
            }

            let r = r_sum / 4.0;
            let g = g_sum / 4.0;
            let b = b_sum / 4.0;

            // BT.601 RGB to UV conversion
            let u = ((-0.169 * r - 0.331 * g + 0.500 * b) + 128.0).clamp(0.0, 255.0) as u8;
            let v = ((0.500 * r - 0.419 * g - 0.081 * b) + 128.0).clamp(0.0, 255.0) as u8;

            let uv_idx = uv_offset + (y / 2) * width + (x / 2) * 2;
            nv12[uv_idx] = u;
            nv12[uv_idx + 1] = v;
        }
    }

    nv12
}

/// Check if H.264 data contains a keyframe (IDR NAL unit)
fn is_h264_keyframe(data: &[u8]) -> bool {
    // Look for NAL unit type 5 (IDR) or 7 (SPS) which indicates a keyframe
    let mut i = 0;
    while i < data.len().saturating_sub(4) {
        // Look for start code (0x00 0x00 0x01 or 0x00 0x00 0x00 0x01)
        if data[i] == 0 && data[i + 1] == 0 {
            let (nal_start, start_code_len) = if data[i + 2] == 1 {
                (i + 3, 3)
            } else if data[i + 2] == 0 && i + 3 < data.len() && data[i + 3] == 1 {
                (i + 4, 4)
            } else {
                i += 1;
                continue;
            };

            if nal_start < data.len() {
                let nal_type = data[nal_start] & 0x1F;
                // NAL type 5 = IDR slice, type 7 = SPS
                if nal_type == 5 || nal_type == 7 {
                    return true;
                }
            }

            i = nal_start;
        } else {
            i += 1;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bgra_to_nv12_size() {
        let width = 1920;
        let height = 1080;
        let bgra = vec![0u8; width * height * 4];
        let nv12 = bgra_to_nv12(&bgra, width, height);

        // NV12 should be 1.5x the pixel count
        assert_eq!(nv12.len(), width * height * 3 / 2);
    }

    #[test]
    fn test_is_h264_keyframe() {
        // IDR NAL unit with 4-byte start code
        let idr_frame = vec![0x00, 0x00, 0x00, 0x01, 0x65, 0x00, 0x00];
        assert!(is_h264_keyframe(&idr_frame));

        // SPS NAL unit
        let sps = vec![0x00, 0x00, 0x00, 0x01, 0x67, 0x00, 0x00];
        assert!(is_h264_keyframe(&sps));

        // P-frame (NAL type 1)
        let p_frame = vec![0x00, 0x00, 0x00, 0x01, 0x41, 0x00, 0x00];
        assert!(!is_h264_keyframe(&p_frame));
    }
}
