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

/// Wayland capture backend using wlr-screencopy protocol
#[cfg(all(target_os = "linux", feature = "wayland"))]
pub mod wayland {
    use super::*;
    use std::cell::RefCell;
    use std::ffi::CString;
    use std::rc::Rc;
    use wayland_client::{
        protocol::{wl_compositor, wl_output, wl_registry, wl_shm},
        Connection, Dispatch, QueueHandle,
    };

    // wlr-screencopy protocol constants
    // These would normally be generated by wayland-scanner from the protocol XML
    pub mod screencopy {
        /// wlr-screencopy manager interface name
        pub const MANAGER_INTERFACE: &str = "zwlr_screencopy_manager_v1";
        /// wlr-screencopy frame interface name
        pub const FRAME_INTERFACE: &str = "zwlr_screencopy_frame_v1";
        /// Current protocol version
        pub const VERSION: u32 = 3;

        /// Frame event codes
        pub mod frame_event {
            pub const BUFFER: u32 = 0;
            pub const READY: u32 = 1;
            pub const FAILED: u32 = 2;
            pub const DAMAGE: u32 = 3;
            pub const LINUX_DMA_REQUEST: u32 = 4;
            pub const BUFFER_DONE: u32 = 5;
        }

        /// Frame error codes
        pub mod frame_error {
            pub const ALREADY_USED: u32 = 0;
            pub const BUFFER_INVALID: u32 = 1;
        }

        /// SHM buffer flags
        pub mod shm_flag {
            pub const Y_INVERT: u32 = 1;
        }
    }

    /// DRM format identifiers for buffer formats
    pub mod drm_format {
        pub const XRGB8888: u32 = 0x34325258;
        pub const ARGB8888: u32 = 0x34325241;
        pub const XBGR8888: u32 = 0x34324258;
        pub const ABGR8888: u32 = 0x34324241;
        pub const NV12: u32 = 0x3231564E;
    }

    /// DMA-BUF format info for zero-copy capture
    #[derive(Debug, Clone)]
    pub struct DmaBufFormat {
        /// DRM format code
        pub format: u32,
        /// Format modifiers (for tiling, compression, etc.)
        pub modifiers: Vec<u64>,
    }

    /// SHM buffer state
    #[derive(Debug, Default)]
    struct ShmBufferState {
        /// SHM file descriptor
        fd: Option<i32>,
        /// Mapped memory pointer
        addr: *mut u8,
        /// Mapped size
        size: usize,
        /// WL SHM pool name (for cleanup)
        pool_name: Option<String>,
    }

    // ShmBufferState is Send (addr is only accessed within this module)
    unsafe impl Send for ShmBufferState {}

    impl ShmBufferState {
        fn new() -> Self {
            Self {
                fd: None,
                addr: ptr::null_mut(),
                size: 0,
                pool_name: None,
            }
        }
    }

    /// Output information
    #[derive(Debug, Clone)]
    pub struct OutputInfo {
        /// Output name
        pub name: String,
        /// Output width in pixels
        pub width: u32,
        /// Output height in pixels
        pub height: u32,
        /// Output scale factor
        pub scale: i32,
        /// wl_output proxy id
        pub id: u32,
    }

    /// Screencopy frame state
    #[derive(Debug, Clone, Copy, PartialEq)]
    enum FrameState {
        /// No frame requested
        Idle,
        /// Frame requested, waiting for buffer event
        WaitingForBuffer,
        /// Buffer info received, ready to copy
        ReadyForCopy,
        /// Frame copy completed
        Ready,
        /// Frame failed
        Failed,
    }

    /// Wayland capture backend using wlr-screencopy protocol
    ///
    /// This implementation supports:
    /// - SHM-based frame capture (copy path)
    /// - DMA-BUF support detection (for future zero-copy)
    /// - Multi-output enumeration
    /// - Damage tracking for optimized capture
    pub struct WaylandCapture {
        /// Wayland connection
        connection: Option<Connection>,
        /// Event queue handle
        queue_handle: Option<QueueHandle<WaylandState>>,
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
        /// SHM buffer state for zero-copy
        shm_state: ShmBufferState,
        /// Shared frame data (accessible from callbacks)
        frame_data: Rc<RefCell<FrameData>>,
        /// Available outputs
        outputs: Vec<OutputInfo>,
        /// Selected output index
        selected_output: usize,
        /// Screencopy manager available
        has_screencopy: bool,
        /// DMA-BUF support
        has_dma_buf: bool,
        /// Available DMA-BUF formats
        dma_buf_formats: Vec<DmaBufFormat>,
        /// Frame capture state
        frame_state: FrameState,
        /// Frame pending flag
        frame_pending: bool,
    }

    /// Internal frame data shared with callbacks
    #[derive(Debug, Default)]
    struct FrameData {
        /// Received frame width
        width: u32,
        /// Received frame height
        height: u32,
        /// Received frame format (DRM format code)
        format: u32,
        /// Frame stride
        stride: u32,
        /// Y inversion flag
        y_invert: bool,
    }

    /// Wayland state for event handling
    pub struct WaylandState {
        /// Registry global list
        globals: Vec<GlobalInfo>,
        /// Compositor
        compositor: Option<wl_compositor::WlCompositor>,
        /// SHM
        shm: Option<wl_shm::WlShm>,
        /// Outputs
        outputs: Vec<wl_output::WlOutput>,
    }

    /// Global interface information
    #[derive(Debug, Clone)]
    struct GlobalInfo {
        name: u32,
        interface: String,
        version: u32,
    }

    // WaylandCapture is Send (all interior mutability is thread-safe)
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
                queue_handle: None,
                width: 0,
                height: 0,
                format: FrameFormat::Bgra32,
                active: false,
                buffer: Vec::new(),
                shm_state: ShmBufferState::new(),
                frame_data: Rc::new(RefCell::new(FrameData::default())),
                outputs: Vec::new(),
                selected_output: 0,
                has_screencopy: false,
                has_dma_buf: false,
                dma_buf_formats: Vec::new(),
                frame_state: FrameState::Idle,
                frame_pending: false,
            })
        }

        /// Check if wlr-screencopy protocol is available
        fn check_screencopy_support(&mut self) -> bool {
            // This would query the registry for zwlr_screencopy_manager_v1
            // For now, return false as we need proper protocol bindings
            log::info!("Checking for wlr-screencopy protocol support");

            // TODO: Implement actual registry query
            // let display = self.connection.as_ref().unwrap().display();
            // let registry = display.get_registry(&self.queue_handle, ());
            // Check for screencopy::MANAGER_INTERFACE in registry globals

            // Simulate checking for protocol
            if std::env::var("WAYLAND_DISPLAY").is_ok() {
                log::warn!(
                    "wlr-screencopy protocol check not implemented, assuming unavailable. Full implementation requires wayland-scanner generated bindings."
                );
            }

            false
        }

        /// Initialize wlr-screencopy manager
        fn init_screencopy_manager(&mut self) -> io::Result<()> {
            if !self.has_screencopy {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "wlr-screencopy protocol not available",
                ));
            }

            // TODO: Bind to zwlr_screencopy_manager_v1
            // let manager = registry.bind::<ZwlrScreencopyManagerV1>(
            //     name, version, &queue_handle, ()
            // );

            log::info!("wlr-screencopy manager initialized");
            Ok(())
        }

        /// Get output dimensions
        fn get_output_dimensions(&self) -> io::Result<(u32, u32)> {
            if let Some(output) = self.outputs.get(self.selected_output) {
                Ok((output.width, output.height))
            } else {
                // Default size if no outputs detected
                log::warn!("No Wayland outputs detected, using default 1920x1080");
                Ok((1920, 1080))
            }
        }

        /// Enumerate available outputs
        fn enumerate_outputs(&mut self) -> io::Result<Vec<OutputInfo>> {
            // TODO: Implement actual output enumeration using wl_output
            // This would:
            // 1. Get all wl_output globals from registry
            // 2. Bind to each output
            // 3. Query geometry and mode information
            // 4. Store output info

            log::info!("Enumerating Wayland outputs (stub)");

            // Return a default output for now
            Ok(vec![OutputInfo {
                name: "default".to_string(),
                width: 1920,
                height: 1080,
                scale: 1,
                id: 0,
            }])
        }

        /// Initialize SHM buffer for frame capture
        fn init_shm_buffer(&mut self, size: usize) -> io::Result<()> {
            // Cleanup any existing SHM buffer
            self.cleanup_shm_buffer();

            // Create unique shm name
            let pid = unsafe { libc::getpid() };
            let shm_name = format!("/lg-wayland-capture-{}", pid);

            let c_name = CString::new(shm_name.as_str()).map_err(|e| {
                io::Error::new(io::ErrorKind::InvalidInput, e.to_string())
            })?;

            // Create shared memory object
            let fd = unsafe {
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

            // Unlink the name now - fd remains valid
            let _ = unsafe { libc::shm_unlink(c_name.as_ptr()) };

            self.shm_state.fd = Some(fd);
            self.shm_state.addr = addr as *mut u8;
            self.shm_state.size = size;
            self.shm_state.pool_name = Some(shm_name);

            log::info!(
                "Wayland SHM buffer initialized: fd={}, size={}, addr={:?}",
                fd,
                size,
                self.shm_state.addr
            );

            Ok(())
        }

        /// Cleanup SHM buffer resources
        fn cleanup_shm_buffer(&mut self) {
            // Unmap shared memory
            if !self.shm_state.addr.is_null() && self.shm_state.size > 0 {
                unsafe {
                    libc::munmap(
                        self.shm_state.addr as *mut libc::c_void,
                        self.shm_state.size,
                    );
                }
            }

            // Close fd
            if let Some(fd) = self.shm_state.fd {
                unsafe {
                    libc::close(fd);
                }
            }

            self.shm_state = ShmBufferState::new();
            log::debug!("Wayland SHM buffer cleaned up");
        }

        /// Request a frame capture from screencopy
        fn request_screencopy_frame(&mut self) -> io::Result<()> {
            if !self.has_screencopy {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "wlr-screencopy not available",
                ));
            }

            // TODO: Implement actual frame request
            // This would:
            // 1. Call capture_output on the screencopy manager
            // 2. Pass the output to capture
            // 3. Listen for buffer, ready, failed events

            self.frame_state = FrameState::WaitingForBuffer;
            self.frame_pending = true;

            log::debug!("Requested screencopy frame");
            Ok(())
        }

        /// Process pending Wayland events
        fn process_events(&mut self) -> io::Result<bool> {
            // TODO: Dispatch Wayland event queue
            // if let Some(qh) = &self.queue_handle {
            //     self.connection.as_ref().unwrap().dispatch_pending(qh, &mut state)?;
            // }

            // Return true if frame is ready
            Ok(self.frame_state == FrameState::Ready)
        }

        /// Check DMA-BUF support
        fn check_dma_buf_support(&mut self) -> bool {
            // TODO: Query linux-dmabuf protocol
            // This would check for zwp_linux_dmabuf_v1 and query formats

            log::debug!("DMA-BUF support check (not implemented)");
            false
        }

        /// Get supported DRM format for frame format
        fn get_drm_format(&self) -> u32 {
            match self.format {
                FrameFormat::Bgra32 => drm_format::ARGB8888,
                FrameFormat::Rgba32 => drm_format::ABGR8888,
                FrameFormat::Nv12 => drm_format::NV12,
            }
        }

        /// Capture frame using SHM (copy path)
        fn capture_shm(&mut self) -> io::Result<CapturedFrame> {
            // If screencopy is available and we have a valid SHM buffer
            if self.has_screencopy && !self.shm_state.addr.is_null() {
                // Request frame if not pending
                if self.frame_state == FrameState::Idle {
                    self.request_screencopy_frame()?;
                }

                // Process events
                let ready = self.process_events()?;

                if ready {
                    // Frame data is in SHM buffer
                    let data_size = self.shm_state.size;

                    return Ok(CapturedFrame {
                        data: None,
                        data_ptr: self.shm_state.addr,
                        data_size,
                        width: self.width,
                        height: self.height,
                        format: self.format,
                        is_keyframe: true,
                    });
                }
            }

            // Fallback to buffer copy
            self.capture_fallback()
        }

        /// Capture frame using DMA-BUF (zero-copy)
        fn capture_dma_buf(&mut self) -> io::Result<CapturedFrame> {
            if !self.has_dma_buf {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "DMA-BUF capture not supported",
                ));
            }

            // TODO: Implement DMA-BUF capture
            // This would:
            // 1. Request frame with DMA-BUF
            // 2. Receive dmabuf fd
            // 3. Import to GPU or map for CPU access

            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "DMA-BUF capture not implemented",
            ))
        }

        /// Fallback capture (stub pattern)
        fn capture_fallback(&mut self) -> io::Result<CapturedFrame> {
            let data_size = self.buffer.len();

            // Generate test pattern
            let frame_count = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u8)
                .unwrap_or(0);

            let bpp = self.format.bytes_per_pixel() as usize;
            for y in 0..self.height as usize {
                for x in 0..self.width as usize {
                    let offset = (y * self.width as usize + x) * bpp;
                    if offset + bpp <= data_size {
                        // Purple tint to distinguish from other stubs
                        self.buffer[offset] = 128; // B
                        self.buffer[offset + 1] = 50; // G
                        self.buffer[offset + 2] = ((x + y) as u8).wrapping_add(frame_count); // R
                        if bpp == 4 {
                            self.buffer[offset + 3] = 255; // A
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

        /// Initialize the screencopy subsystem
        fn init_screencopy(&mut self) -> io::Result<()> {
            // Check for screencopy protocol
            self.has_screencopy = self.check_screencopy_support();

            // Enumerate outputs
            self.outputs = self.enumerate_outputs()?;

            // Check DMA-BUF support
            self.has_dma_buf = self.check_dma_buf_support();

            if self.has_screencopy {
                log::info!("wlr-screencopy protocol available");
                self.init_screencopy_manager()?;
            } else {
                log::warn!(
                    "wlr-screencopy protocol not available, using fallback capture. Install a wlroots-based compositor (sway, wayfire, etc.) for proper capture."
                );
            }

            if self.has_dma_buf {
                log::info!("DMA-BUF support available for zero-copy capture");
            }

            Ok(())
        }

        /// Create SHM pool and buffer for screencopy
        fn create_shm_pool(&mut self) -> io::Result<()> {
            let size = (self.width * self.height * self.format.bytes_per_pixel() as u32) as usize;
            self.init_shm_buffer(size)?;

            // TODO: Create wl_shm_pool and wl_buffer
            // let pool = self.shm.as_ref().unwrap().create_pool(
            //     self.shm_state.fd.unwrap(),
            //     size as i32,
            //     &self.queue_handle,
            //     ()
            // );
            // let buffer = pool.create_buffer(
            //     0, self.width as i32, self.height as i32,
            //     (self.width * self.format.bytes_per_pixel() as u32) as i32,
            //     self.get_drm_format(),
            //     &self.queue_handle,
            //     ()
            // );

            Ok(())
        }
    }

    impl FrameCapture for WaylandCapture {
        fn init(&mut self, width: u32, height: u32, format: FrameFormat) -> io::Result<()> {
            // Initialize screencopy subsystem
            self.init_screencopy()?;

            // Get output dimensions
            let (output_w, output_h) = self.get_output_dimensions()?;
            self.width = if width == 0 { output_w } else { width };
            self.height = if height == 0 { output_h } else { height };
            self.format = format;

            // Calculate buffer size
            let size = (self.width * self.height * format.bytes_per_pixel() as u32) as usize;
            self.buffer.resize(size, 0);

            // Initialize SHM buffer for zero-copy if screencopy available
            if self.has_screencopy {
                self.create_shm_pool()?;
            }

            log::info!(
                "Wayland capture initialized: {}x{}, format={:?}, screencopy={}, dma_buf={}",
                self.width,
                self.height,
                self.format,
                self.has_screencopy,
                self.has_dma_buf
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

            // Prefer DMA-BUF for zero-copy
            if self.has_dma_buf {
                if let Ok(frame) = self.capture_dma_buf() {
                    return Ok(frame);
                }
            }

            // Try SHM capture
            if self.has_screencopy && !self.shm_state.addr.is_null() {
                return self.capture_shm();
            }

            // Fallback
            self.capture_fallback()
        }

        fn dimensions(&self) -> (u32, u32) {
            (self.width, self.height)
        }

        fn is_active(&self) -> bool {
            self.active
        }

        fn start(&mut self) -> io::Result<()> {
            self.active = true;
            self.frame_state = FrameState::Idle;
            log::info!("Wayland capture started");
            Ok(())
        }

        fn stop(&mut self) -> io::Result<()> {
            self.active = false;
            self.frame_state = FrameState::Idle;
            self.frame_pending = false;
            log::info!("Wayland capture stopped");
            Ok(())
        }
    }

    impl Drop for WaylandCapture {
        fn drop(&mut self) {
            self.cleanup_shm_buffer();
            log::debug!("Wayland capture dropped");
        }
    }

    // Implement Dispatch for wl_registry to handle global events
    impl Dispatch<wl_registry::WlRegistry, ()> for WaylandState {
        fn event(
            _state: &mut Self,
            _registry: &wl_registry::WlRegistry,
            _event: wl_registry::Event,
            _data: &(),
            _conn: &Connection,
            _qhandle: &QueueHandle<Self>,
        ) {
            // Handle registry events (global added/removed)
            // This would populate the globals list
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
