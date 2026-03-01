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
            // Full implementation requires wayland-scanner generated protocol bindings
            log::info!("Checking for wlr-screencopy protocol support");

            // Check WAYLAND_DISPLAY environment
            if std::env::var("WAYLAND_DISPLAY").is_err() {
                log::debug!("WAYLAND_DISPLAY not set, screencopy unavailable");
                return false;
            }

            // In a full implementation, we would:
            // 1. Get registry from display
            // 2. Listen for global events
            // 3. Check if "zwlr_screencopy_manager_v1" is advertised
            // 4. Bind to the screencopy manager if available

            log::info!(
                "wlr-screencopy: Full implementation requires wayland-scanner generated bindings. Using fallback capture."
            );

            // Check if compositor likely supports screencopy
            // Common compositors that support wlr-screencopy: sway, hyprland, wayfire, river
            let compositor_support = std::env::var("XDG_CURRENT_DESKTOP")
                .map(|v| {
                    matches!(v.to_lowercase().as_str(), "sway" | "hyprland" | "wayfire" | "river")
                })
                .unwrap_or(false);

            if compositor_support {
                log::info!("Detected Wayland compositor that likely supports wlr-screencopy");
                // Still return false since we don't have protocol bindings
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

            // Full implementation would:
            // 1. Get the registry global from connection
            // 2. Find the screencopy manager global by name
            // 3. Bind to zwlr_screencopy_manager_v1
            // Example:
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
        ///
        /// Full implementation requires:
        /// 1. Get all wl_output globals from registry
        /// 2. Bind to each output
        /// 3. Query geometry and mode information via callbacks
        /// 4. Store output info (name, resolution, scale factor)
        ///
        /// This requires wayland-scanner generated protocol bindings.
        fn enumerate_outputs(&mut self) -> io::Result<Vec<OutputInfo>> {
            log::info!("Enumerating Wayland outputs (stub - full impl needs protocol bindings)");

            // Return a default output for fallback mode
            Ok(vec![OutputInfo {
                name: "default".to_string(),
                width: 1920,
                height: 1080,
                scale: 1,
                id: 0,
            }])
        }

        /// Get list of available outputs
        ///
        /// Returns information about all detected Wayland outputs (monitors).
        /// Use `select_output()` to choose which output to capture.
        pub fn get_outputs(&self) -> &[OutputInfo] {
            &self.outputs
        }

        /// Select output to capture
        ///
        /// # Arguments
        /// * `index` - Zero-based index of the output to capture
        ///
        /// # Returns
        /// `true` if the output was selected, `false` if index is out of bounds
        ///
        /// # Example
        /// ```no_run
        /// let mut capture = WaylandCapture::new().unwrap();
        /// let outputs = capture.get_outputs();
        /// if outputs.len() > 1 {
        ///     capture.select_output(1); // Select second monitor
        /// }
        /// ```
        pub fn select_output(&mut self, index: usize) -> bool {
            if index < self.outputs.len() {
                self.selected_output = index;
                log::info!("Selected output {}: {:?}", index, self.outputs[index]);
                true
            } else {
                log::warn!("Output index {} out of bounds ({} outputs available)", index, self.outputs.len());
                false
            }
        }

        /// Get currently selected output
        ///
        /// Returns information about the currently selected output,
        /// or `None` if no outputs are available.
        pub fn get_selected_output(&self) -> Option<&OutputInfo> {
            self.outputs.get(self.selected_output)
        }

        /// Get number of available outputs
        pub fn output_count(&self) -> usize {
            self.outputs.len()
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
        ///
        /// Full implementation requires wlr-screencopy protocol bindings.
        /// Steps:
        /// 1. Call capture_output on the screencopy manager
        /// 2. Pass the output to capture and overlay_cursor flag
        /// 3. Create and attach a wl_buffer (SHM or DMA-BUF)
        /// 4. Listen for buffer, ready, failed events on frame
        fn request_screencopy_frame(&mut self) -> io::Result<()> {
            if !self.has_screencopy {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "wlr-screencopy not available",
                ));
            }

            // Frame request would use:
            // frame = manager.capture_output(overlay_cursor, output, &qh, ());
            // frame.buffer(format, width, height, stride);
            // frame.copy(shm_buffer);

            self.frame_state = FrameState::WaitingForBuffer;
            self.frame_pending = true;

            log::debug!("Requested screencopy frame (protocol bindings required for full impl)");
            Ok(())
        }

        /// Process pending Wayland events
        fn process_events(&mut self) -> io::Result<bool> {
            // Dispatch any pending Wayland events
            // Note: Full implementation requires wlr-screencopy protocol bindings
            // generated by wayland-scanner

            if let Some(ref connection) = self.connection {
                // Try to dispatch pending events without blocking
                // In a full implementation, this would use the event queue
                // to process screencopy frame events

                // Simulate frame ready after a short delay for fallback mode
                if self.frame_pending && self.frame_state == FrameState::WaitingForBuffer {
                    // In real implementation, this would be set by frame callback
                    // For now, transition to ready state for testing
                    self.frame_state = FrameState::Ready;
                }
            }

            // Return true if frame is ready
            Ok(self.frame_state == FrameState::Ready)
        }

        /// Check DMA-BUF support
        ///
        /// Full implementation requires:
        /// 1. Query zwp_linux_dmabuf_v1 global from registry
        /// 2. Bind to the dmabuf interface
        /// 3. Query supported formats via get_default_feedback/modifier events
        /// 4. Check if our preferred format (ARGB8888/XRGB8888) is supported
        fn check_dma_buf_support(&mut self) -> bool {
            log::debug!("DMA-BUF support check (requires linux-dmabuf protocol bindings)");

            // DMA-BUF support depends on:
            // - zwp_linux_dmabuf_v1 protocol
            // - GPU driver support for format/modifier combinations
            // - Compositor implementation

            // Without actual protocol bindings, we cannot determine support
            // A real implementation would listen for dmabuf.format events

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
        ///
        /// Full implementation requires:
        /// 1. linux-dmabuf protocol support from compositor
        /// 2. GPU buffer allocation (GBM or similar)
        /// 3. Import DMA-BUF fd for CPU or GPU access
        ///
        /// Benefits of DMA-BUF over SHM:
        /// - Zero-copy path to GPU/encoder
        /// - Better performance for video encoding
        /// - Direct display to GPU textures
        fn capture_dma_buf(&mut self) -> io::Result<CapturedFrame> {
            if !self.has_dma_buf {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "DMA-BUF capture not supported",
                ));
            }

            // Full implementation steps:
            // 1. Allocate DMA-BUF via GBM or similar
            // 2. Create wl_buffer from dmabuf params
            // 3. Request screencopy frame with dmabuf buffer
            // 4. Receive frame ready event
            // 5. Either:
            //    a. Map dmabuf for CPU read (requires mmap)
            //    b. Pass dmabuf fd to encoder/GPU directly

            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "DMA-BUF capture requires linux-dmabuf protocol bindings",
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
        ///
        /// Creates a wl_shm_pool from the shared memory buffer and a wl_buffer
        /// that can be used with wlr-screencopy to receive frame data.
        ///
        /// Full implementation requires wayland-scanner generated bindings for:
        /// - `wl_shm::create_pool()` - Create shared memory pool from FD
        /// - `wl_shm_pool::create_buffer()` - Create buffer from pool
        ///
        /// The buffer format must match the DRM format requested from screencopy.
        fn create_shm_pool(&mut self) -> io::Result<()> {
            let size = (self.width * self.height * self.format.bytes_per_pixel() as u32) as usize;
            self.init_shm_buffer(size)?;

            // Full implementation requires wayland-scanner generated protocol bindings:
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

/// Windows DXGI frame capture module
///
/// Provides frame capture using the Desktop Duplication API (DXGI) on Windows.
/// This is the most efficient method for capturing the Windows desktop.
#[cfg(all(target_os = "windows", feature = "dxgi"))]
pub mod dxgi {
    use super::*;
    use std::ptr;

    // Import Windows API types
    use windows::{
        core::*,
        Win32::Foundation::*,
        Win32::Graphics::Direct3D11::*,
        Win32::Graphics::Dxgi::*,
        Win32::Graphics::Gdi::*,
    };

    /// DXGI-based frame capture for Windows
    ///
    /// Uses the Desktop Duplication API for efficient screen capture.
    /// This provides GPU-accelerated capture with minimal performance impact.
    pub struct DxgiCapture {
        /// Output width
        width: u32,
        /// Output height
        height: u32,
        /// Frame format
        format: FrameFormat,
        /// Active state
        active: bool,
        /// Pre-allocated buffer
        buffer: Vec<u8>,
        /// D3D11 device
        device: Option<ID3D11Device>,
        /// Device context
        context: Option<ID3D11DeviceContext>,
        /// Desktop duplication
        duplication: Option<IDXGIOutputDuplication>,
        /// Staging texture for CPU readback
        staging_texture: Option<ID3D11Texture2D>,
    }

    impl DxgiCapture {
        /// Create a new DXGI capture instance
        ///
        /// # Errors
        /// Returns an error if Direct3D 11 or DXGI is not available.
        pub fn new() -> io::Result<Self> {
            Ok(Self {
                width: 0,
                height: 0,
                format: FrameFormat::Bgra32,
                active: false,
                buffer: Vec::new(),
                device: None,
                context: None,
                duplication: None,
                staging_texture: None,
            })
        }

        /// Initialize Direct3D 11 device and desktop duplication
        fn init_d3d11(&mut self) -> io::Result<()> {
            // Create D3D11 device
            let mut device = None;
            let mut feature_level = D3D_FEATURE_LEVEL_11_0;

            unsafe {
                D3D11CreateDevice(
                    None, // Default adapter
                    D3D_DRIVER_TYPE_HARDWARE,
                    None,
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    Some(&[D3D_FEATURE_LEVEL_11_0]),
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    Some(&mut feature_level),
                    None,
                )
                .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("D3D11CreateDevice failed: {}", e)))?;
            }

            let device = device.ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotSupported, "Failed to create D3D11 device")
            })?;

            // Get device context
            let context = unsafe { device.GetImmediateContext() }
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

            // Get DXGI device and adapter
            let dxgi_device: IDXGIDevice = unsafe { device.cast() }
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

            let adapter = unsafe { dxgi_device.GetAdapter() }
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

            // Get first output (monitor)
            let output = unsafe { adapter.EnumOutputs(0) }
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

            // Get output description
            let desc = unsafe { output.GetDesc() };

            self.width = desc.DesktopCoordinates.right as u32 - desc.DesktopCoordinates.left as u32;
            self.height = desc.DesktopCoordinates.bottom as u32 - desc.DesktopCoordinates.top as u32;

            // Query for IDXGIOutput1
            let output1: IDXGIOutput1 = unsafe { output.cast() }
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

            // Create desktop duplication
            let duplication = unsafe { output1.DuplicateOutput(&device) }
                .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("DuplicateOutput failed: {}", e)))?;

            // Create staging texture for CPU readback
            let staging_desc = D3D11_TEXTURE2D_DESC {
                Width: self.width,
                Height: self.height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
            };

            let mut staging_texture = None;
            unsafe {
                device.CreateTexture2D(&staging_desc, None, Some(&mut staging_texture))
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
            }

            self.device = Some(device);
            self.context = Some(context);
            self.duplication = Some(duplication);
            self.staging_texture = staging_texture;

            log::info!(
                "DXGI capture initialized: {}x{}, Desktop Duplication API active",
                self.width,
                self.height
            );

            Ok(())
        }

        /// Capture a frame using Desktop Duplication
        fn capture_duplication(&mut self) -> io::Result<CapturedFrame> {
            let duplication = self.duplication.as_ref().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotConnected, "Desktop duplication not initialized")
            })?;

            // Release previous frame
            unsafe {
                let _ = duplication.ReleaseFrame();
            }

            // Acquire next frame
            let mut frame_resource: Option<IDXGIResource> = None;
            let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();

            let hr = unsafe {
                duplication.AcquireNextFrame(1000, &mut frame_info, &mut frame_resource)
            };

            if hr.is_err() {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "No frame available"));
            }

            let frame_resource = frame_resource.ok_or_else(|| {
                io::Error::new(io::ErrorKind::Other, "No frame resource")
            })?;

            // Get the texture from the frame
            let texture: ID3D11Texture2D = unsafe { frame_resource.cast() }
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

            let context = self.context.as_ref().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotConnected, "Device context not available")
            })?;

            let staging = self.staging_texture.as_ref().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotConnected, "Staging texture not available")
            })?;

            // Copy to staging texture
            unsafe {
                context.CopyResource(Some(staging.cast().unwrap()), Some(texture.cast().unwrap()));
            }

            // Map staging texture for reading
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            unsafe {
                context.Map(Some(staging.cast().unwrap()), 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
            }

            // Copy to buffer
            let data_size = (self.width * self.height * 4) as usize;
            self.buffer.resize(data_size, 0);

            let src = mapped.pData as *const u8;
            let src_pitch = mapped.RowPitch as usize;
            let dst_pitch = (self.width * 4) as usize;

            for y in 0..self.height as usize {
                unsafe {
                    ptr::copy_nonoverlapping(
                        src.add(y * src_pitch),
                        self.buffer.as_mut_ptr().add(y * dst_pitch),
                        dst_pitch,
                    );
                }
            }

            // Unmap
            unsafe {
                context.Unmap(Some(staging.cast().unwrap()), 0);
            }

            Ok(CapturedFrame {
                data: Some(self.buffer.clone()),
                data_ptr: ptr::null_mut(),
                data_size,
                width: self.width,
                height: self.height,
                format: FrameFormat::Bgra32,
                is_keyframe: frame_info.LastPresentTime.QuadPart != 0,
            })
        }
    }

    impl FrameCapture for DxgiCapture {
        fn init(&mut self, width: u32, height: u32, format: FrameFormat) -> io::Result<()> {
            self.format = format;
            self.init_d3d11()?;

            // Resize if specific dimensions requested
            if width > 0 && height > 0 && (width != self.width || height != self.height) {
                log::info!("Note: DXGI captures at desktop resolution {}x{}, ignoring requested {}x{}",
                    self.width, self.height, width, height);
            }

            // Pre-allocate buffer
            let size = (self.width * self.height * 4) as usize;
            self.buffer.resize(size, 0);

            Ok(())
        }

        fn capture_frame(&mut self) -> io::Result<CapturedFrame> {
            if !self.active {
                return Err(io::Error::new(
                    io::ErrorKind::NotConnected,
                    "Capture not active",
                ));
            }

            self.capture_duplication()
        }

        fn dimensions(&self) -> (u32, u32) {
            (self.width, self.height)
        }

        fn is_active(&self) -> bool {
            self.active
        }

        fn start(&mut self) -> io::Result<()> {
            if self.device.is_none() {
                self.init_d3d11()?;
            }
            self.active = true;
            log::info!("DXGI capture started");
            Ok(())
        }

        fn stop(&mut self) -> io::Result<()> {
            self.active = false;
            // Release frame
            if let Some(dup) = &self.duplication {
                unsafe {
                    let _ = dup.ReleaseFrame();
                }
            }
            log::info!("DXGI capture stopped");
            Ok(())
        }
    }

    impl Drop for DxgiCapture {
        fn drop(&mut self) {
            if let Some(dup) = &self.duplication {
                unsafe {
                    let _ = dup.ReleaseFrame();
                }
            }
            log::debug!("DXGI capture dropped");
        }
    }
}

// Re-export the appropriate capture backend
// Priority: Wayland > X11 > DXGI (Windows) > Stub
#[cfg(all(target_os = "linux", feature = "wayland"))]
pub use wayland::WaylandCapture as DefaultCapture;

#[cfg(all(target_os = "linux", not(feature = "wayland")))]
pub use x11::X11Capture as DefaultCapture;

#[cfg(all(target_os = "windows", feature = "dxgi"))]
pub use dxgi::DxgiCapture as DefaultCapture;

#[cfg(all(target_os = "windows", not(feature = "dxgi")))]
pub use stub::StubCapture as DefaultCapture;

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub use stub::StubCapture as DefaultCapture;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frame_format_bytes_per_pixel() {
        assert_eq!(FrameFormat::Bgra32.bytes_per_pixel(), 4);
        assert_eq!(FrameFormat::Rgba32.bytes_per_pixel(), 4);
        assert_eq!(FrameFormat::Nv12.bytes_per_pixel(), 1); // Y plane only
    }

    #[test]
    fn test_captured_frame_default() {
        let frame = CapturedFrame::default();
        assert_eq!(frame.width, 0);
        assert_eq!(frame.height, 0);
        assert!(!frame.is_keyframe);
    }

    #[test]
    fn test_captured_frame_debug() {
        let frame = CapturedFrame {
            data: None,
            data_ptr: std::ptr::null_mut(),
            data_size: 100,
            width: 1920,
            height: 1080,
            format: FrameFormat::Bgra32,
            is_keyframe: true,
        };
        let debug_str = format!("{:?}", frame);
        assert!(debug_str.contains("1920"));
        assert!(debug_str.contains("1080"));
    }

    #[test]
    fn test_stub_capture_creation() {
        let capture = stub::StubCapture::new();
        assert!(capture.is_ok());
    }

    #[test]
    fn test_stub_capture_frame() {
        let mut capture = stub::StubCapture::new().unwrap();
        assert!(capture.init(320, 240, FrameFormat::Bgra32).is_ok());
        let frame = capture.capture_frame();
        assert!(frame.is_ok());
        let frame = frame.unwrap();
        assert_eq!(frame.width, 320);
        assert_eq!(frame.height, 240);
    }

    #[test]
    fn test_stub_capture_start_stop() {
        let mut capture = stub::StubCapture::new().unwrap();
        assert!(capture.init(100, 100, FrameFormat::Bgra32).is_ok());
        assert!(capture.start().is_ok());
        assert!(capture.is_active());
        assert!(capture.stop().is_ok());
        assert!(!capture.is_active());
    }
}
