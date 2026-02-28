// Copyright 2024 Cloud Hypervisor Authors. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Input Backend Abstraction
//!
//! This module defines the trait and types for input backends.
//! Each backend (PS/2, virtio-input, USB HID) implements this trait.

use super::event::{InputEvent, KeyboardEvent, MouseEvent};
use super::{InputError, Result};

/// Stealth level indicates how detectable the input backend is
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum StealthLevel {
    /// Easily detected as virtual device (e.g., virtio-input with Red Hat vendor ID)
    Low,
    /// May be detected with careful inspection (e.g., USB HID)
    Medium,
    /// Very hard to detect (e.g., PS/2 emulation)
    High,
}

/// Capabilities of an input backend
#[derive(Clone, Debug)]
pub struct InputCapabilities {
    /// Maximum keyboard rate (events per second)
    pub max_keyboard_rate: u32,
    /// Supports absolute mouse positioning
    pub supports_absolute_mouse: bool,
    /// Supports multi-touch
    pub supports_multi_touch: bool,
    /// Supports scroll wheel
    pub supports_scroll_wheel: bool,
    /// Maximum scroll wheel range
    pub max_scroll_range: i8,
    /// Stealth level of this backend
    pub stealth_level: StealthLevel,
    /// Backend name
    pub name: &'static str,
    /// Backend description
    pub description: &'static str,
}

impl Default for InputCapabilities {
    fn default() -> Self {
        Self {
            max_keyboard_rate: 500,
            supports_absolute_mouse: false,
            supports_multi_touch: false,
            supports_scroll_wheel: true,
            max_scroll_range: 8,
            stealth_level: StealthLevel::Low,
            name: "unknown",
            description: "Unknown input backend",
        }
    }
}

/// Input backend trait
///
/// All input backends must implement this trait to provide
/// unified input injection functionality.
pub trait InputBackend: Send {
    /// Get the backend name
    fn name(&self) -> &'static str;

    /// Get backend capabilities
    fn capabilities(&self) -> InputCapabilities;

    /// Check if the backend is ready to accept input
    fn is_ready(&self) -> bool;

    /// Inject a keyboard event
    fn inject_keyboard(&mut self, event: &KeyboardEvent) -> Result<()>;

    /// Inject a mouse event
    fn inject_mouse(&mut self, event: &MouseEvent) -> Result<()>;

    /// Inject a generic input event
    fn inject(&mut self, event: &InputEvent) -> Result<()> {
        match event {
            InputEvent::Keyboard(kb) => self.inject_keyboard(kb),
            InputEvent::Mouse(m) => self.inject_mouse(m),
        }
    }

    /// Flush any pending events (optional)
    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

// ============================================================================
// PS/2 Backend (i8042)
// ============================================================================

/// PS/2 input backend using i8042 device
pub struct Ps2Backend {
    capabilities: InputCapabilities,
}

impl Ps2Backend {
    /// Create a new PS/2 backend
    pub fn new() -> Self {
        Self {
            capabilities: InputCapabilities {
                max_keyboard_rate: 500,
                supports_absolute_mouse: false,
                supports_multi_touch: false,
                supports_scroll_wheel: true,
                max_scroll_range: 8,
                stealth_level: StealthLevel::High,
                name: "ps2",
                description: "PS/2 keyboard and mouse (i8042)",
            },
        }
    }
}

impl Default for Ps2Backend {
    fn default() -> Self {
        Self::new()
    }
}

impl InputBackend for Ps2Backend {
    fn name(&self) -> &'static str {
        self.capabilities.name
    }

    fn capabilities(&self) -> InputCapabilities {
        self.capabilities.clone()
    }

    fn is_ready(&self) -> bool {
        // PS/2 is always ready once the device is created
        true
    }

    fn inject_keyboard(&mut self, _event: &KeyboardEvent) -> Result<()> {
        // This will be connected to the actual i8042 device
        // For now, return Ok to indicate the backend structure is correct
        Ok(())
    }

    fn inject_mouse(&mut self, _event: &MouseEvent) -> Result<()> {
        // This will be connected to the actual i8042 device
        Ok(())
    }
}

// ============================================================================
// VirtIO Input Backend (Placeholder)
// ============================================================================

/// VirtIO input backend
pub struct VirtioInputBackend {
    capabilities: InputCapabilities,
    ready: bool,
}

impl VirtioInputBackend {
    /// Create a new virtio-input backend
    pub fn new() -> Self {
        Self {
            capabilities: InputCapabilities {
                max_keyboard_rate: 1000,
                supports_absolute_mouse: true,
                supports_multi_touch: true,
                supports_scroll_wheel: true,
                max_scroll_range: 127,
                stealth_level: StealthLevel::Low,
                name: "virtio",
                description: "VirtIO input device",
            },
            ready: false,
        }
    }

    /// Set ready state
    pub fn set_ready(&mut self, ready: bool) {
        self.ready = ready;
    }
}

impl Default for VirtioInputBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl InputBackend for VirtioInputBackend {
    fn name(&self) -> &'static str {
        self.capabilities.name
    }

    fn capabilities(&self) -> InputCapabilities {
        self.capabilities.clone()
    }

    fn is_ready(&self) -> bool {
        self.ready
    }

    fn inject_keyboard(&mut self, _event: &KeyboardEvent) -> Result<()> {
        if !self.ready {
            return Err(InputError::DeviceNotReady);
        }
        // TODO: Implement virtio-input injection
        Err(InputError::UnsupportedAction(
            "virtio-input not yet implemented".to_string(),
        ))
    }

    fn inject_mouse(&mut self, _event: &MouseEvent) -> Result<()> {
        if !self.ready {
            return Err(InputError::DeviceNotReady);
        }
        // TODO: Implement virtio-input injection
        Err(InputError::UnsupportedAction(
            "virtio-input not yet implemented".to_string(),
        ))
    }
}

// ============================================================================
// Backend Factory
// ============================================================================

/// Available backend types
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendType {
    Ps2,
    Virtio,
    UsbHid,
}

impl Default for BackendType {
    fn default() -> Self {
        BackendType::Ps2
    }
}

impl BackendType {
    /// Get backend name
    pub fn name(&self) -> &'static str {
        match self {
            BackendType::Ps2 => "ps2",
            BackendType::Virtio => "virtio",
            BackendType::UsbHid => "usb",
        }
    }

    /// Parse from string
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_lowercase().as_str() {
            "ps2" | "ps/2" => Some(BackendType::Ps2),
            "virtio" | "virtio-input" => Some(BackendType::Virtio),
            "usb" | "usb-hid" | "hid" => Some(BackendType::UsbHid),
            _ => None,
        }
    }
}
