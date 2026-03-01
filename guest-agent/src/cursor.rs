// Copyright 2024 lg-capture Authors
// SPDX-License-Identifier: Apache-2.0

//! Cursor capture module
//!
//! Provides cursor position and shape capture.

use std::io;

use crate::protocol::CursorShapeInfo;

/// Cursor capture trait
pub trait CursorCapture: Send {
    /// Get cursor position
    fn get_position(&self) -> io::Result<(i32, i32)>;

    /// Check if cursor is visible
    fn is_visible(&self) -> bool;

    /// Get cursor shape (if changed)
    fn get_shape(&mut self) -> io::Result<Option<CursorShape>>;

    /// Check if cursor shape has changed
    fn has_shape_changed(&self) -> bool;
}

/// Cursor shape data
#[derive(Debug, Clone)]
pub struct CursorShape {
    /// Shape info
    pub info: CursorShapeInfo,
    /// Pixel data (BGRA format)
    pub data: Vec<u8>,
}

/// X11 cursor capture
#[cfg(target_os = "linux")]
pub mod x11_cursor {
    use super::*;
    use x11rb::connection::Connection;
    use x11rb::protocol::xfixes::ConnectionExt as XfixesExt;
    use x11rb::protocol::xproto::{ConnectionExt as XprotoExt, Window};

    /// X11 cursor capture using RustConnection (thread-safe)
    pub struct X11CursorCapture {
        display: Option<x11rb::rust_connection::RustConnection>,
        screen_num: usize,
        root: Window,
        last_cursor_serial: u64,
    }

    // X11CursorCapture is Send because RustConnection is Send
    unsafe impl Send for X11CursorCapture {}

    impl X11CursorCapture {
        pub fn new() -> io::Result<Self> {
            let (display, screen_num) = x11rb::connect(None)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

            // Get root window
            let screen = display.setup().roots.get(screen_num).ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "Screen not found")
            })?;
            let root = screen.root;

            // Initialize XFixes extension
            let xfixes_cookie = display
                .query_extension(b"XFIXES\0")
                .map_err(|e: x11rb::errors::ConnectionError| {
                    io::Error::new(io::ErrorKind::Other, e.to_string())
                })?;
            let xfixes_reply = xfixes_cookie
                .reply()
                .map_err(|e: x11rb::errors::ReplyError| {
                    io::Error::new(io::ErrorKind::Other, e.to_string())
                })?;

            if !xfixes_reply.present {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "XFixes extension not available",
                ));
            }

            Ok(Self {
                display: Some(display),
                screen_num,
                root,
                last_cursor_serial: 0,
            })
        }
    }

    impl CursorCapture for X11CursorCapture {
        fn get_position(&self) -> io::Result<(i32, i32)> {
            let display = self.display.as_ref().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotConnected, "No display connection")
            })?;

            // Use XQueryPointer to get cursor position
            let cookie = display
                .query_pointer(self.root)
                .map_err(|e: x11rb::errors::ConnectionError| {
                    io::Error::new(io::ErrorKind::Other, e.to_string())
                })?;

            let reply = cookie
                .reply()
                .map_err(|e: x11rb::errors::ReplyError| {
                    io::Error::new(io::ErrorKind::Other, e.to_string())
                })?;

            Ok((reply.root_x as i32, reply.root_y as i32))
        }

        fn is_visible(&self) -> bool {
            // X11 doesn't provide a simple way to check if cursor is hidden
            // Most display servers keep the cursor visible
            true
        }

        fn get_shape(&mut self) -> io::Result<Option<CursorShape>> {
            let display = self.display.as_ref().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotConnected, "No display connection")
            })?;

            // Use XFixesGetCursorImageAndName to get cursor image
            let cookie = display
                .xfixes_get_cursor_image_and_name()
                .map_err(|e: x11rb::errors::ConnectionError| {
                    io::Error::new(io::ErrorKind::Other, e.to_string())
                })?;

            let reply = cookie
                .reply()
                .map_err(|e: x11rb::errors::ReplyError| {
                    io::Error::new(io::ErrorKind::Other, e.to_string())
                })?;

            // Check if cursor has changed
            if reply.cursor_serial as u64 == self.last_cursor_serial {
                return Ok(None);
            }

            self.last_cursor_serial = reply.cursor_serial as u64;

            // Convert cursor image to BGRA format
            let width = reply.width;
            let height = reply.height;
            let hot_x = reply.xhot as i16;
            let hot_y = reply.yhot as i16;

            // Cursor image from XFixes is in ARGB format (u32 per pixel)
            // Convert to BGRA format (u8 per channel)
            let src_data = reply.cursor_image;
            let mut data = vec![0u8; width as usize * height as usize * 4];

            for (i, &pixel) in src_data.iter().enumerate() {
                let offset = i * 4;
                if offset + 3 < data.len() {
                    // ARGB -> BGRA
                    data[offset] = ((pixel >> 16) & 0xFF) as u8; // B
                    data[offset + 1] = ((pixel >> 8) & 0xFF) as u8; // G
                    data[offset + 2] = (pixel & 0xFF) as u8; // R
                    data[offset + 3] = ((pixel >> 24) & 0xFF) as u8; // A
                }
            }

            Ok(Some(CursorShape {
                info: CursorShapeInfo {
                    width,
                    height,
                    hot_x,
                    hot_y,
                    data_size: (width as u32 * height as u32 * 4),
                },
                data,
            }))
        }

        fn has_shape_changed(&self) -> bool {
            // This is checked in get_shape by comparing cursor_serial
            // We return false here since the actual check happens in get_shape
            false
        }
    }
}

/// Stub cursor capture
pub mod stub_cursor {
    use super::*;

    pub struct StubCursorCapture {
        x: i32,
        y: i32,
    }

    impl StubCursorCapture {
        pub fn new() -> io::Result<Self> {
            Ok(Self { x: 0, y: 0 })
        }
    }

    impl CursorCapture for StubCursorCapture {
        fn get_position(&self) -> io::Result<(i32, i32)> {
            Ok((self.x, self.y))
        }

        fn is_visible(&self) -> bool {
            true
        }

        fn get_shape(&mut self) -> io::Result<Option<CursorShape>> {
            // Return a simple 32x32 cursor
            let size = 32u16;
            let bpp = 4;
            let mut data = vec![0u8; size as usize * size as usize * bpp];

            // Simple arrow cursor pattern
            for y in 0..size as usize {
                for x in 0..size as usize {
                    if x == y || (x < 8 && y < 8 && x >= y) {
                        let offset = (y * size as usize + x) * bpp;
                        if offset + bpp <= data.len() {
                            data[offset] = 255;     // B
                            data[offset + 1] = 255; // G
                            data[offset + 2] = 255; // R
                            data[offset + 3] = 255; // A
                        }
                    }
                }
            }

            Ok(Some(CursorShape {
                info: CursorShapeInfo {
                    width: size,
                    height: size,
                    hot_x: 0,
                    hot_y: 0,
                    data_size: (size * size * bpp as u16) as u32,
                },
                data,
            }))
        }

        fn has_shape_changed(&self) -> bool {
            false
        }
    }
}

#[cfg(target_os = "linux")]
pub use x11_cursor::X11CursorCapture as DefaultCursorCapture;

#[cfg(not(target_os = "linux"))]
pub use stub_cursor::StubCursorCapture as DefaultCursorCapture;
