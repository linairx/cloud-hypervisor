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

    /// X11 cursor capture using RustConnection (thread-safe)
    pub struct X11CursorCapture {
        display: Option<x11rb::rust_connection::RustConnection>,
        last_shape_serial: u64,
    }

    // X11CursorCapture is Send because RustConnection is Send
    unsafe impl Send for X11CursorCapture {}

    impl X11CursorCapture {
        pub fn new() -> io::Result<Self> {
            let (display, _screen_num) = x11rb::connect(None)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

            Ok(Self {
                display: Some(display),
                last_shape_serial: 0,
            })
        }
    }

    impl CursorCapture for X11CursorCapture {
        fn get_position(&self) -> io::Result<(i32, i32)> {
            // Would use XQueryPointer
            Ok((0, 0))
        }

        fn is_visible(&self) -> bool {
            true
        }

        fn get_shape(&mut self) -> io::Result<Option<CursorShape>> {
            // Would use XFixesGetCursorImage
            Ok(None)
        }

        fn has_shape_changed(&self) -> bool {
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
