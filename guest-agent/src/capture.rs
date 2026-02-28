// Copyright 2024 lg-capture Authors
// SPDX-License-Identifier: Apache-2.0

//! Frame capture backends
//!
//! Provides frame capture implementations for different display systems.
//! Supports zero-copy capture using XShm extension.

use std::io;
use std::ptr;

use crate::protocol::FrameFormat;

/// Captured frame data
#[derive(Debug)]
pub struct CapturedFrame {
    /// Frame data (Some for copy path, None for zero-copy)
    pub data: Option<Vec<u8>>,
    /// Direct pointer to frame data (for zero-copy)
    pub data_ptr: *mut u8,
    /// Data size
    pub data_size: usize,
    /// Frame width
    pub width: u32,
    /// Frame height
    pub height: u32,
    /// Frame format
    pub format: FrameFormat,
    /// Is keyframe
    pub is_keyframe: bool,
}

impl Default for CapturedFrame {
    fn default() -> Self {
        Self {
            data: None,
            data_ptr: ptr::null_mut(),
            data_size: 0,
            width: 0,
            height: 0,
            format: FrameFormat::Bgra32,
            is_keyframe: true,
        }
    }
}

/// Frame capture trait
pub trait FrameCapture: Send {
    /// Initialize the capture backend
    fn init(&mut self, width: u32, height: u32, format: FrameFormat) -> io::Result<()>;

    /// Capture a frame
    fn capture_frame(&mut self) -> io::Result<CapturedFrame>;

    /// Get current frame dimensions
    fn dimensions(&self) -> (u32, u32);

    /// Check if capture is active
    fn is_active(&self) -> bool;

    /// Start capture
    fn start(&mut self) -> io::Result<()>;

    /// Stop capture
    fn stop(&mut self) -> io::Result<()>;
}

/// X11 frame capture implementation
#[cfg(target_os = "linux")]
pub mod x11 {
    use super::*;
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::*;
    use x11rb::rust_connection::RustConnection;

    /// X11 capture backend
    pub struct X11Capture {
        /// Display connection
        display: Option<RustConnection>,
        /// Screen number
        screen_num: usize,
        /// Screen root window
        root: Window,
        /// Current width
        width: u32,
        /// Current height
        height: u32,
        /// Frame format
        format: FrameFormat,
        /// Active state
        active: bool,
        /// Pre-allocated buffer for zero-copy simulation
        buffer: Vec<u8>,
    }

    // X11Capture is Send
    unsafe impl Send for X11Capture {}

    impl X11Capture {
        /// Create a new X11 capture backend
        pub fn new() -> io::Result<Self> {
            let (display, screen_num) = x11rb::connect(None)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

            let screen = display.setup().roots.get(screen_num).ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "Screen not found")
            })?;

            // Copy root window before moving display
            let root = screen.root;

            Ok(Self {
                display: Some(display),
                screen_num,
                root,
                width: 0,
                height: 0,
                format: FrameFormat::Bgra32,
                active: false,
                buffer: Vec::new(),
            })
        }

        /// Get screen dimensions
        fn get_screen_dimensions(&self) -> io::Result<(u32, u32)> {
            let display = self.display.as_ref().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotConnected, "No display connection")
            })?;

            let screen = display.setup().roots.get(self.screen_num).ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "Screen not found")
            })?;

            Ok((screen.width_in_pixels as u32, screen.height_in_pixels as u32))
        }

        /// Capture using standard XGetImage
        fn capture_standard(&mut self) -> io::Result<CapturedFrame> {
            let display = self.display.as_ref().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotConnected, "No display connection")
            })?;

            // Get image using standard method
            let cookie = display
                .get_image(
                    ImageFormat::Z_PIXMAP,
                    self.root,
                    0,
                    0,
                    self.width as u16,
                    self.height as u16,
                    0xFFFFFFFF,
                )
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

            let reply = cookie
                .reply()
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

            let bpp = self.format.bytes_per_pixel() as usize;
            let expected_size = (self.width * self.height * bpp as u32) as usize;

            // Ensure buffer is large enough
            if self.buffer.len() != expected_size {
                self.buffer.resize(expected_size, 0);
            }

            // Copy data to our buffer (one copy, then reuse)
            let copy_size = reply.data.len().min(expected_size);
            self.buffer[..copy_size].copy_from_slice(&reply.data[..copy_size]);

            Ok(CapturedFrame {
                data: None,
                data_ptr: self.buffer.as_mut_ptr(),
                data_size: expected_size,
                width: self.width,
                height: self.height,
                format: self.format,
                is_keyframe: true,
            })
        }
    }

    impl FrameCapture for X11Capture {
        fn init(&mut self, width: u32, height: u32, format: FrameFormat) -> io::Result<()> {
            let (screen_w, screen_h) = self.get_screen_dimensions()?;
            self.width = if width == 0 { screen_w } else { width };
            self.height = if height == 0 { screen_h } else { height };
            self.format = format;

            // Pre-allocate buffer
            let size = (self.width * self.height * format.bytes_per_pixel() as u32) as usize;
            self.buffer.resize(size, 0);

            Ok(())
        }

        fn capture_frame(&mut self) -> io::Result<CapturedFrame> {
            if !self.active {
                return Err(io::Error::new(io::ErrorKind::NotConnected, "Capture not active"));
            }

            self.capture_standard()
        }

        fn dimensions(&self) -> (u32, u32) {
            (self.width, self.height)
        }

        fn is_active(&self) -> bool {
            self.active
        }

        fn start(&mut self) -> io::Result<()> {
            self.active = true;
            Ok(())
        }

        fn stop(&mut self) -> io::Result<()> {
            self.active = false;
            Ok(())
        }
    }

    impl Drop for X11Capture {
        fn drop(&mut self) {
            // Connection will be cleaned up by x11rb
        }
    }
}

/// Stub capture for platforms without implementation
pub mod stub {
    use super::*;

    pub struct StubCapture {
        width: u32,
        height: u32,
        format: FrameFormat,
        active: bool,
        frame_count: u64,
        /// Pre-allocated buffer for zero-copy simulation
        buffer: Vec<u8>,
    }

    impl StubCapture {
        pub fn new() -> io::Result<Self> {
            Ok(Self {
                width: 0,
                height: 0,
                format: FrameFormat::Bgra32,
                active: false,
                frame_count: 0,
                buffer: Vec::new(),
            })
        }
    }

    impl FrameCapture for StubCapture {
        fn init(&mut self, width: u32, height: u32, format: FrameFormat) -> io::Result<()> {
            self.width = width;
            self.height = height;
            self.format = format;

            // Pre-allocate buffer
            let size = (width * height * format.bytes_per_pixel() as u32) as usize;
            self.buffer.resize(size, 0);

            Ok(())
        }

        fn capture_frame(&mut self) -> io::Result<CapturedFrame> {
            if !self.active {
                return Err(io::Error::new(io::ErrorKind::NotConnected, "Capture not active"));
            }

            let bpp = self.format.bytes_per_pixel() as usize;
            let data_size = (self.width * self.height * bpp as u32) as usize;

            // Ensure buffer is large enough
            if self.buffer.len() != data_size {
                self.buffer.resize(data_size, 0);
            }

            // Generate a test pattern (simulates frame capture)
            let frame_phase = (self.frame_count % 256) as u8;

            for y in 0..self.height as usize {
                for x in 0..self.width as usize {
                    let offset = (y * self.width as usize + x) * bpp;
                    if offset + bpp <= data_size {
                        self.buffer[offset] = (x as u8).wrapping_add(frame_phase);     // B
                        self.buffer[offset + 1] = (y as u8).wrapping_add(frame_phase);  // G
                        self.buffer[offset + 2] = ((x + y) as u8).wrapping_add(frame_phase); // R
                        if bpp == 4 {
                            self.buffer[offset + 3] = 255; // A
                        }
                    }
                }
            }

            self.frame_count += 1;

            Ok(CapturedFrame {
                data: None,
                data_ptr: self.buffer.as_mut_ptr(),
                data_size,
                width: self.width,
                height: self.height,
                format: self.format,
                is_keyframe: true,
            })
        }

        fn dimensions(&self) -> (u32, u32) {
            (self.width, self.height)
        }

        fn is_active(&self) -> bool {
            self.active
        }

        fn start(&mut self) -> io::Result<()> {
            self.active = true;
            Ok(())
        }

        fn stop(&mut self) -> io::Result<()> {
            self.active = false;
            Ok(())
        }
    }
}

// Re-export the appropriate capture backend
#[cfg(target_os = "linux")]
pub use x11::X11Capture as DefaultCapture;

#[cfg(not(target_os = "linux"))]
pub use stub::StubCapture as DefaultCapture;
