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
    use std::ffi::CString;
    use x11rb::connection::Connection;
    use x11rb::protocol::shm;
    use x11rb::protocol::xproto::*;
    use x11rb::rust_connection::RustConnection;

    /// X11 capture backend with XShm support
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
        /// Pre-allocated buffer for fallback
        buffer: Vec<u8>,
        /// XShm segment ID (if available)
        shmseg: Option<shm::Seg>,
        /// XShm shared memory fd
        shm_fd: Option<i32>,
        /// XShm mapped memory pointer
        shm_addr: *mut u8,
        /// XShm size
        shm_size: usize,
        /// Has XShm extension
        has_shm: bool,
    }

    // X11Capture is Send (shm_addr is only accessed within this struct)
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
                shmseg: None,
                shm_fd: None,
                shm_addr: ptr::null_mut(),
                shm_size: 0,
                has_shm: false,
            })
        }

        /// Check if XShm extension is available
        fn check_shm_extension(&self) -> bool {
            let display = match &self.display {
                Some(d) => d,
                None => return false,
            };

            // Query SHM extension version
            match shm::query_version(display) {
                Ok(cookie) => {
                    if let Ok(_reply) = cookie.reply() {
                        log::info!("XShm extension available");
                        return true;
                    }
                }
                Err(e) => {
                    log::debug!("XShm query_version failed: {}", e);
                }
            }
            false
        }

        /// Initialize XShm segment
        fn init_shm(&mut self, size: usize) -> io::Result<()> {
            let display = self.display.as_ref().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotConnected, "No display connection")
            })?;

            // Create unique shm name
            let pid = unsafe { libc::getpid() };
            let shm_name = format!("/lg-capture-{}", pid);

            // Create shared memory object
            let c_name = CString::new(shm_name.as_str()).map_err(|e| {
                io::Error::new(io::ErrorKind::InvalidInput, e.to_string())
            })?;

            let fd = unsafe {
                // Create with read/write, create if not exists, truncate if exists
                libc::shm_open(
                    c_name.as_ptr(),
                    libc::O_CREAT | libc::O_RDWR | libc::O_EXCL,
                    0o600,
                )
            };

            if fd < 0 {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!("shm_open failed: {}", std::io::Error::last_os_error()),
                ));
            }

            // Set size
            let result = unsafe { libc::ftruncate(fd, size as libc::off_t) };
            if result < 0 {
                unsafe { libc::close(fd) };
                let _ = unsafe { libc::shm_unlink(c_name.as_ptr()) };
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!("ftruncate failed: {}", std::io::Error::last_os_error()),
                ));
            }

            // Map shared memory
            let addr = unsafe {
                libc::mmap(
                    ptr::null_mut(),
                    size,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    fd,
                    0,
                )
            };

            if addr == libc::MAP_FAILED {
                unsafe { libc::close(fd) };
                let _ = unsafe { libc::shm_unlink(c_name.as_ptr()) };
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!("mmap failed: {}", std::io::Error::last_os_error()),
                ));
            }

            // Allocate a new XShm segment ID
            let seg_id = display
                .generate_id()
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

            // Duplicate fd for X11 (it takes ownership)
            let fd_for_x11 = unsafe { libc::dup(fd) };
            if fd_for_x11 < 0 {
                unsafe {
                    libc::munmap(addr, size);
                    libc::close(fd);
                    libc::shm_unlink(c_name.as_ptr());
                }
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!("dup failed: {}", std::io::Error::last_os_error()),
                ));
            }

            // Create RawFdContainer from fd
            let raw_fd = x11rb::utils::RawFdContainer::new(fd_for_x11);

            // Attach the shared memory segment to X server
            shm::attach_fd(display, seg_id, raw_fd, false).map_err(|e| {
                unsafe {
                    libc::munmap(addr, size);
                    libc::close(fd);
                    libc::shm_unlink(c_name.as_ptr());
                }
                io::Error::new(io::ErrorKind::Other, e.to_string())
            })?;

            // Unlink the name now - the fd remains valid
            let _ = unsafe { libc::shm_unlink(c_name.as_ptr()) };

            self.shmseg = Some(seg_id);
            self.shm_fd = Some(fd);
            self.shm_addr = addr as *mut u8;
            self.shm_size = size;
            self.has_shm = true;

            log::info!(
                "XShm segment initialized: seg={}, size={}, fd={}, addr={:?}",
                seg_id,
                size,
                fd,
                self.shm_addr
            );

            Ok(())
        }

        /// Capture using XShm (zero-copy)
        fn capture_shm(&mut self) -> io::Result<CapturedFrame> {
            let display = self.display.as_ref().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotConnected, "No display connection")
            })?;

            let shmseg_id = self.shmseg.ok_or_else(|| {
                io::Error::new(io::ErrorKind::Other, "XShm segment not initialized")
            })?;

            // Get image using XShm
            let cookie = shm::get_image(
                display,
                self.root,
                0,
                0,
                self.width as u16,
                self.height as u16,
                0xFFFFFFFF,
                ImageFormat::Z_PIXMAP.into(),
                shmseg_id,
                0, // offset
            )
            .map_err(|e: x11rb::errors::ConnectionError| {
                io::Error::new(io::ErrorKind::Other, e.to_string())
            })?;

            // Wait for completion (XShm GetImage returns depth info)
            let _reply = cookie.reply().map_err(|e| {
                io::Error::new(io::ErrorKind::Other, format!("XShm GetImage reply error: {}", e))
            })?;

            let data_size = self.shm_size;

            Ok(CapturedFrame {
                data: None,
                data_ptr: self.shm_addr,
                data_size,
                width: self.width,
                height: self.height,
                format: self.format,
                is_keyframe: true,
            })
        }

        /// Cleanup XShm resources
        fn cleanup_shm(&mut self) {
            if !self.has_shm {
                return;
            }

            // Detach from X server
            if let (Some(display), Some(shmseg_id)) = (&self.display, self.shmseg) {
                let _ = shm::detach(display, shmseg_id);
            }

            self.shmseg = None;

            // Unmap shared memory
            if !self.shm_addr.is_null() && self.shm_size > 0 {
                unsafe {
                    libc::munmap(self.shm_addr as *mut libc::c_void, self.shm_size);
                }
            }

            // Close fd
            if let Some(fd) = self.shm_fd {
                unsafe {
                    libc::close(fd);
                }
            }

            self.shm_fd = None;
            self.shm_addr = ptr::null_mut();
            self.shm_size = 0;
            self.has_shm = false;

            log::debug!("XShm resources cleaned up");
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
            // Cleanup existing XShm resources if any
            self.cleanup_shm();

            let (screen_w, screen_h) = self.get_screen_dimensions()?;
            self.width = if width == 0 { screen_w } else { width };
            self.height = if height == 0 { screen_h } else { height };
            self.format = format;

            // Calculate required size
            let size = (self.width * self.height * format.bytes_per_pixel() as u32) as usize;

            // Try to init XShm for zero-copy
            if self.check_shm_extension() {
                match self.init_shm(size) {
                    Ok(()) => {
                        log::info!("XShm extension enabled for zero-copy capture");
                    }
                    Err(e) => {
                        log::warn!("XShm init failed, falling back to standard capture: {}", e);
                    }
                }
            }

            // Always allocate fallback buffer
            self.buffer.resize(size, 0);

            Ok(())
        }

        fn capture_frame(&mut self) -> io::Result<CapturedFrame> {
            if !self.active {
                return Err(io::Error::new(io::ErrorKind::NotConnected, "Capture not active"));
            }

            // Prefer XShm if available
            if self.has_shm && self.shmseg.is_some() {
                self.capture_shm()
            } else {
                self.capture_standard()
            }
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
            self.cleanup_shm();
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

/// Wayland capture backend
#[cfg(all(target_os = "linux", feature = "wayland"))]
pub mod wayland {
    use super::*;
    use wayland_client::Connection;

    /// Wayland capture backend using wlr-screencopy protocol
    ///
    /// This is a framework implementation. Full implementation requires:
    /// - wlr-screencopy protocol binding
    /// - DMA-BUF support for true zero-copy
    /// - wl_output for display enumeration
    /// - wl_surface for capture target
    pub struct WaylandCapture {
        /// Wayland connection
        connection: Option<Connection>,
        /// Current width
        width: u32,
        /// Current height
        height: u32,
        /// Frame format
        format: FrameFormat,
        /// Active state
        active: bool,
        /// Pre-allocated buffer for frame data
        buffer: Vec<u8>,
    }

    // WaylandCapture is Send (buffer is only accessed within this struct)
    unsafe impl Send for WaylandCapture {}

    impl WaylandCapture {
        /// Create a new Wayland capture backend
        pub fn new() -> io::Result<Self> {
            // Connect to Wayland compositor
            let connection = Connection::connect_to_env().map_err(|e| {
                io::Error::new(io::ErrorKind::ConnectionRefused, format!("{}", e))
            })?;

            log::info!("Connected to Wayland compositor");

            Ok(Self {
                connection: Some(connection),
                width: 0,
                height: 0,
                format: FrameFormat::Bgra32,
                active: false,
                buffer: Vec::new(),
            })
        }

        /// Get output dimensions (stub - requires wl_output implementation)
        fn get_output_dimensions(&self) -> io::Result<(u32, u32)> {
            // TODO: Implement using wl_output
            // For now, return a default size
            log::warn!("Wayland output dimensions not implemented, using default 1920x1080");
            Ok((1920, 1080))
        }

        /// Initialize wlr-screencopy frame (stub - requires protocol implementation)
        fn init_screencopy(&mut self) -> io::Result<()> {
            // TODO: Register wlr-screencopy frame
            // This would:
            // 1. Get wlr-screencopy manager from registry
            // 2. Create a capture frame for the output
            // 3. Set up buffer for the frame (DMA-BUF or shared memory)
            log::info!("wlr-screencopy initialization (stub)");
            Ok(())
        }
    }

    impl FrameCapture for WaylandCapture {
        fn init(&mut self, width: u32, height: u32, format: FrameFormat) -> io::Result<()> {
            let (output_w, output_h) = self.get_output_dimensions()?;
            self.width = if width == 0 { output_w } else { width };
            self.height = if height == 0 { output_h } else { height };
            self.format = format;

            // Calculate buffer size
            let size = (self.width * self.height * format.bytes_per_pixel() as u32) as usize;
            self.buffer.resize(size, 0);

            // Initialize screencopy
            self.init_screencopy()?;

            log::info!(
                "Wayland capture initialized: {}x{}, format={:?}",
                self.width,
                self.height,
                self.format
            );

            Ok(())
        }

        fn capture_frame(&mut self) -> io::Result<CapturedFrame> {
            if !self.active {
                return Err(io::Error::new(
                    io::ErrorKind::NotConnected,
                    "Capture not active",
                ));
            }

            // TODO: Implement actual frame capture using wlr-screencopy
            // For now, return the buffer (would be filled by screencopy callback)
            let data_size = self.buffer.len();

            // Fill with a test pattern to indicate stub implementation
            let frame_count = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as u8)
                .unwrap_or(0);

            let bpp = self.format.bytes_per_pixel() as usize;
            for y in 0..self.height as usize {
                for x in 0..self.width as usize {
                    let offset = (y * self.width as usize + x) * bpp;
                    if offset + bpp <= data_size {
                        // Purple tint to distinguish from stub
                        self.buffer[offset] = 128;           // B
                        self.buffer[offset + 1] = 50;        // G
                        self.buffer[offset + 2] = ((x + y) as u8).wrapping_add(frame_count); // R
                        if bpp == 4 {
                            self.buffer[offset + 3] = 255;   // A
                        }
                    }
                }
            }

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
            log::info!("Wayland capture started");
            Ok(())
        }

        fn stop(&mut self) -> io::Result<()> {
            self.active = false;
            log::info!("Wayland capture stopped");
            Ok(())
        }
    }

    impl Drop for WaylandCapture {
        fn drop(&mut self) {
            log::debug!("Wayland capture dropped");
        }
    }
}

// Re-export the appropriate capture backend
// Priority: Wayland > X11 > Stub
#[cfg(all(target_os = "linux", feature = "wayland"))]
pub use wayland::WaylandCapture as DefaultCapture;

#[cfg(all(target_os = "linux", not(feature = "wayland")))]
pub use x11::X11Capture as DefaultCapture;

#[cfg(not(target_os = "linux"))]
pub use stub::StubCapture as DefaultCapture;
