//! DXGI Desktop Duplication API for high-performance screen capture.
//!
//! Uses Windows Desktop Duplication API for GPU-accelerated screen capture
//! with dirty rectangle tracking for incremental updates.

use std::ptr;
use std::mem;
use std::slice;

use windows::core::Interface;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
    D3D11_CPU_ACCESS_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_STAGING, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput,
    IDXGIOutput1, IDXGIResource, IDXGISurface,
    DXGI_ERROR_WAIT_TIMEOUT, DXGI_ERROR_ACCESS_LOST, DXGI_MAP_READ,
    DXGI_RESOURCE_PRIORITY_MAXIMUM,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_MODE_ROTATION, DXGI_SAMPLE_DESC,
};

/// A dirty rectangle representing a changed region
#[derive(Debug, Clone, Copy)]
pub struct DirtyRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl DirtyRect {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }

    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }

    pub fn from_rect(rect: &RECT) -> Self {
        Self {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        }
    }
}

/// Frame data returned by the capturer
pub struct CapturedFrame<'a> {
    /// BGRA pixel data (borrowed directly from mapped GPU memory - zero copy!)
    pub data: &'a [u8],
    /// Frame width
    pub width: u32,
    /// Frame height
    pub height: u32,
    /// Row stride in bytes (may differ from width*4 due to GPU alignment)
    pub stride: u32,
    /// Dirty rectangles (changed regions)
    pub dirty_rects: Vec<DirtyRect>,
    /// Whether this is a full frame (no dirty rect info available)
    pub is_full_frame: bool,
}

/// DXGI Desktop Duplication capturer
pub struct DxgiCapturer {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    duplication: windows::Win32::Graphics::Dxgi::IDXGIOutputDuplication,
    width: u32,
    height: u32,
    rotation: DXGI_MODE_ROTATION,
    /// Pre-allocated staging texture for CPU readback
    staging_texture: Option<ID3D11Texture2D>,
    /// Whether desktop image is in system memory (fast path)
    fastlane: bool,
    /// Whether we currently have a frame mapped
    frame_mapped: bool,
    /// Surface for fastlane unmapping
    mapped_surface: Option<IDXGISurface>,
    /// Cached pointer to mapped data (valid while frame_mapped is true)
    mapped_ptr: *const u8,
    /// Cached stride of mapped data
    mapped_stride: u32,
}

impl DxgiCapturer {
    /// Create a new DXGI capturer for the primary display
    pub fn new() -> Result<Self, String> {
        Self::new_for_display(0)
    }

    /// Create a new DXGI capturer for a specific display
    pub fn new_for_display(display_index: u32) -> Result<Self, String> {
        unsafe {
            // Create DXGI Factory
            let factory: IDXGIFactory1 = CreateDXGIFactory1()
                .map_err(|e| format!("Failed to create DXGI factory: {}", e))?;

            // Find the adapter and output for the requested display
            let (adapter, output, output_desc) = Self::find_display(&factory, display_index)?;

            // Create D3D11 device
            let mut device: Option<ID3D11Device> = None;
            let mut context: Option<ID3D11DeviceContext> = None;

            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                windows::Win32::Foundation::HMODULE::default(),
                Default::default(),
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            ).map_err(|e| format!("Failed to create D3D11 device: {}", e))?;

            let device = device.ok_or("D3D11 device is null")?;
            let context = context.ok_or("D3D11 context is null")?;

            // Get IDXGIOutput1 for desktop duplication
            let output1: IDXGIOutput1 = output.cast()
                .map_err(|e| format!("Failed to cast to IDXGIOutput1: {}", e))?;

            // Create desktop duplication
            let duplication = output1.DuplicateOutput(&device)
                .map_err(|e| format!("Failed to create desktop duplication: {} (try running as admin or check if another app is capturing)", e))?;

            // Get duplication description
            let dup_desc = duplication.GetDesc();

            let width = (output_desc.DesktopCoordinates.right - output_desc.DesktopCoordinates.left) as u32;
            let height = (output_desc.DesktopCoordinates.bottom - output_desc.DesktopCoordinates.top) as u32;
            let fastlane = dup_desc.DesktopImageInSystemMemory.as_bool();

            // Pre-create staging texture if not using fastlane
            let staging_texture = if !fastlane {
                Self::create_staging_texture(&device, width, height)?
            } else {
                None
            };

            Ok(Self {
                device,
                context,
                duplication,
                width,
                height,
                rotation: output_desc.Rotation,
                staging_texture,
                fastlane,
                frame_mapped: false,
                mapped_surface: None,
                mapped_ptr: ptr::null(),
                mapped_stride: 0,
            })
        }
    }

    /// Create staging texture once
    unsafe fn create_staging_texture(device: &ID3D11Device, width: u32, height: u32) -> Result<Option<ID3D11Texture2D>, String> {
        let staging_desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: Default::default(),
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: Default::default(),
        };

        let mut staging: Option<ID3D11Texture2D> = None;
        device.CreateTexture2D(&staging_desc, None, Some(&mut staging))
            .map_err(|e| format!("Failed to create staging texture: {}", e))?;

        Ok(staging)
    }

    /// Find a display adapter and output
    unsafe fn find_display(
        factory: &IDXGIFactory1,
        display_index: u32,
    ) -> Result<(IDXGIAdapter1, IDXGIOutput, windows::Win32::Graphics::Dxgi::DXGI_OUTPUT_DESC), String> {
        let mut current_display = 0u32;

        // Enumerate adapters
        let mut adapter_index = 0u32;
        loop {
            let adapter = match factory.EnumAdapters1(adapter_index) {
                Ok(a) => a,
                Err(_) => break,
            };

            // Enumerate outputs for this adapter
            let mut output_index = 0u32;
            loop {
                let output = match adapter.EnumOutputs(output_index) {
                    Ok(o) => o,
                    Err(_) => break,
                };

                let desc = output.GetDesc().map_err(|e| format!("GetDesc failed: {}", e))?;

                if desc.AttachedToDesktop.as_bool() {
                    if current_display == display_index {
                        return Ok((adapter, output, desc));
                    }
                    current_display += 1;
                }

                output_index += 1;
            }

            adapter_index += 1;
        }

        Err(format!("Display {} not found", display_index))
    }

    /// Release any previously held frame
    fn release_frame(&mut self) {
        unsafe {
            if self.frame_mapped {
                if self.fastlane {
                    let _ = self.duplication.UnMapDesktopSurface();
                } else if let Some(ref surface) = self.mapped_surface {
                    let _ = surface.Unmap();
                }
                self.frame_mapped = false;
                self.mapped_surface = None;
            }
            let _ = self.duplication.ReleaseFrame();
        }
    }

    /// Capture a frame with dirty rectangle information
    /// Returns borrowed data directly from GPU memory - ZERO COPY!
    pub fn capture_frame(&mut self, timeout_ms: u32) -> Result<CapturedFrame<'_>, CaptureError> {
        unsafe {
            // Release any previous frame
            self.release_frame();

            // Acquire next frame
            let mut frame_info = Default::default();
            let mut resource: Option<IDXGIResource> = None;

            self.duplication
                .AcquireNextFrame(timeout_ms, &mut frame_info, &mut resource)
                .map_err(|e| {
                    if e.code() == DXGI_ERROR_WAIT_TIMEOUT {
                        CaptureError::Timeout
                    } else if e.code() == DXGI_ERROR_ACCESS_LOST {
                        CaptureError::AccessLost
                    } else {
                        CaptureError::Other(format!("AcquireNextFrame failed: {}", e))
                    }
                })?;

            let resource = resource.ok_or(CaptureError::Other("Resource is null".into()))?;

            // Check if frame is valid
            if frame_info.LastPresentTime == 0 {
                return Err(CaptureError::NoFrame);
            }

            // Get dirty rectangles
            let dirty_rects = self.get_dirty_rects(&frame_info)?;
            let is_full_frame = dirty_rects.is_empty() || frame_info.TotalMetadataBufferSize == 0;

            // Get frame data - use fastlane if available (ZERO COPY - borrow directly from GPU)
            let (data_ptr, stride) = if self.fastlane {
                self.map_fastlane()?
            } else {
                self.copy_to_staging(&resource)?
            };

            self.frame_mapped = true;
            self.mapped_ptr = data_ptr;
            self.mapped_stride = stride;

            // Calculate data length based on stride
            let data_len = stride as usize * self.height as usize;

            Ok(CapturedFrame {
                // ZERO COPY: Return slice directly from mapped GPU memory
                data: slice::from_raw_parts(data_ptr, data_len),
                width: self.width,
                height: self.height,
                stride,
                dirty_rects: if is_full_frame {
                    vec![DirtyRect {
                        left: 0,
                        top: 0,
                        right: self.width as i32,
                        bottom: self.height as i32,
                    }]
                } else {
                    dirty_rects
                },
                is_full_frame,
            })
        }
    }

    /// Map desktop surface directly (fastlane path)
    unsafe fn map_fastlane(&mut self) -> Result<(*const u8, u32), CaptureError> {
        let mapped_rect = self.duplication.MapDesktopSurface()
            .map_err(|e| CaptureError::Other(format!("MapDesktopSurface failed: {}", e)))?;

        Ok((mapped_rect.pBits, mapped_rect.Pitch as u32))
    }

    /// Copy frame to staging texture and map it
    unsafe fn copy_to_staging(&mut self, resource: &IDXGIResource) -> Result<(*const u8, u32), CaptureError> {
        // Get the frame texture
        let texture: ID3D11Texture2D = resource.cast()
            .map_err(|e| CaptureError::Other(format!("Failed to get texture: {}", e)))?;

        // Get staging texture
        let staging = self.staging_texture.as_ref()
            .ok_or(CaptureError::Other("No staging texture".into()))?;

        // Copy from GPU texture to staging texture
        self.context.CopyResource(staging, &texture);

        // Get surface for mapping
        let surface: IDXGISurface = staging.cast()
            .map_err(|e| CaptureError::Other(format!("Failed to get surface: {}", e)))?;

        // Map the surface
        let mut mapped_rect = Default::default();
        surface.Map(&mut mapped_rect, DXGI_MAP_READ)
            .map_err(|e| CaptureError::Other(format!("Failed to map surface: {}", e)))?;

        self.mapped_surface = Some(surface);

        Ok((mapped_rect.pBits, mapped_rect.Pitch as u32))
    }

    /// Get dirty rectangles from frame info
    unsafe fn get_dirty_rects(
        &self,
        frame_info: &windows::Win32::Graphics::Dxgi::DXGI_OUTDUPL_FRAME_INFO,
    ) -> Result<Vec<DirtyRect>, CaptureError> {
        if frame_info.TotalMetadataBufferSize == 0 {
            return Ok(Vec::new());
        }

        // Get dirty rects
        let mut dirty_rects_size = frame_info.TotalMetadataBufferSize;
        let max_rects = (dirty_rects_size / mem::size_of::<RECT>() as u32) as usize;

        if max_rects == 0 {
            return Ok(Vec::new());
        }

        let mut dirty_rects_buffer: Vec<RECT> = vec![Default::default(); max_rects];

        match self.duplication.GetFrameDirtyRects(
            dirty_rects_size,
            dirty_rects_buffer.as_mut_ptr(),
            &mut dirty_rects_size,
        ) {
            Ok(_) => {
                let count = dirty_rects_size as usize / mem::size_of::<RECT>();
                Ok(dirty_rects_buffer[..count]
                    .iter()
                    .map(|r| DirtyRect::from_rect(r))
                    .collect())
            }
            Err(_) => Ok(Vec::new()), // Fall back to full frame
        }
    }

    /// Get screen dimensions
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Get rotation
    pub fn rotation(&self) -> DXGI_MODE_ROTATION {
        self.rotation
    }

    /// Check if using fast path (desktop in system memory)
    pub fn is_fastlane(&self) -> bool {
        self.fastlane
    }
}

impl Drop for DxgiCapturer {
    fn drop(&mut self) {
        self.release_frame();
    }
}

/// Capture error types
#[derive(Debug)]
pub enum CaptureError {
    /// Timeout waiting for frame
    Timeout,
    /// No new frame available
    NoFrame,
    /// Access lost - need to recreate capturer
    AccessLost,
    /// Other error
    Other(String),
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CaptureError::Timeout => write!(f, "Timeout"),
            CaptureError::NoFrame => write!(f, "No frame"),
            CaptureError::AccessLost => write!(f, "Access lost"),
            CaptureError::Other(s) => write!(f, "{}", s),
        }
    }
}
