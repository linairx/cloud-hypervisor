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

// ============================================================================
// USB HID Backend
// ============================================================================

/// USB HID input backend
///
/// This backend emulates a USB Human Interface Device (HID) for keyboard
/// and mouse input. It provides medium stealth level as USB devices are
/// common but can be identified as emulated with careful inspection.
pub struct UsbHidBackend {
    capabilities: InputCapabilities,
    ready: bool,
    /// Keyboard LED state
    keyboard_leds: u8,
    /// Mouse button state
    mouse_buttons: u8,
}

impl UsbHidBackend {
    /// Create a new USB HID backend
    pub fn new() -> Self {
        Self {
            capabilities: InputCapabilities {
                max_keyboard_rate: 1000,
                supports_absolute_mouse: true,
                supports_multi_touch: false,
                supports_scroll_wheel: true,
                max_scroll_range: 127,
                stealth_level: StealthLevel::Medium,
                name: "usb-hid",
                description: "USB Human Interface Device (keyboard + mouse)",
            },
            ready: false,
            keyboard_leds: 0,
            mouse_buttons: 0,
        }
    }

    /// Set ready state
    pub fn set_ready(&mut self, ready: bool) {
        self.ready = ready;
    }

    /// Get keyboard LED state
    pub fn keyboard_leds(&self) -> u8 {
        self.keyboard_leds
    }

    /// Get mouse button state
    pub fn mouse_buttons(&self) -> u8 {
        self.mouse_buttons
    }

    /// Convert keyboard event to USB HID report
    fn keyboard_to_hid_report(&self, event: &KeyboardEvent) -> [u8; 8] {
        // Standard USB HID keyboard report: modifier + reserved + 6 key codes
        let mut report = [0u8; 8];

        // Modifier byte (bit 0-7: LCtrl, LShift, LAlt, LGui, RCtrl, RShift, RAlt, RGui)
        if event.modifiers.ctrl {
            report[0] |= 0x01; // Left Ctrl
        }
        if event.modifiers.shift {
            report[0] |= 0x02; // Left Shift
        }
        if event.modifiers.alt {
            report[0] |= 0x04; // Left Alt
        }
        if event.modifiers.meta {
            report[0] |= 0x08; // Left GUI
        }

        // Key code (position 2-7 can hold up to 6 simultaneous keys)
        // Only set key on press, clear on release
        match event.action {
            super::event::KeyboardAction::Press | super::event::KeyboardAction::Type => {
                // Convert scancode to HID usage code (simplified)
                let hid_code = self.scancode_to_hid(event.code);
                report[2] = hid_code;
            }
            super::event::KeyboardAction::Release => {
                // Release: no key code in report
            }
        }

        report
    }

    /// Convert PS/2 scancode to USB HID usage code
    fn scancode_to_hid(&self, scancode: u16) -> u8 {
        // Simplified mapping - PS/2 Set 1 to USB HID
        match scancode {
            0x01 => 0x29, // Escape
            0x02..=0x0B => (scancode - 0x02 + 0x1E) as u8, // 1-0
            0x0E => 0x2A, // Backspace
            0x0F => 0x2B, // Tab
            0x1E..=0x26 => (scancode - 0x1E + 0x04) as u8, // Q-P
            0x1C => 0x28, // Enter
            0x2A => 0xE1, // Left Shift
            0x1D => 0xE0, // Left Ctrl
            0x38 => 0xE2, // Left Alt
            0x39 => 0x2C, // Space
            _ => scancode as u8, // Fallback
        }
    }

    /// Convert mouse event to USB HID report
    fn mouse_to_hid_report(&mut self, event: &MouseEvent) -> [u8; 6] {
        // Standard USB HID mouse report: buttons + X + Y + wheel
        let mut report = [0u8; 6];

        // Button byte
        if event.buttons.left {
            report[0] |= 0x01;
        }
        if event.buttons.right {
            report[0] |= 0x02;
        }
        if event.buttons.middle {
            report[0] |= 0x04;
        }

        // Handle button actions
        if let Some(ref btn) = event.button {
            let mask = match btn {
                super::event::MouseButton::Left => 0x01,
                super::event::MouseButton::Right => 0x02,
                super::event::MouseButton::Middle => 0x04,
                super::event::MouseButton::Side => 0x08,
                super::event::MouseButton::Extra => 0x10,
            };
            match event.action {
                super::event::MouseAction::ButtonPress | super::event::MouseAction::Click => {
                    report[0] |= mask;
                }
                super::event::MouseAction::ButtonRelease => {
                    report[0] &= !mask;
                }
                _ => {}
            }
        }

        // Relative movement (signed 8-bit)
        report[1] = event.x.clamp(-127, 127) as u8;
        report[2] = event.y.clamp(-127, 127) as u8;

        // Scroll wheel
        report[3] = event.z.clamp(-127, 127) as u8;

        report
    }
}

impl Default for UsbHidBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl InputBackend for UsbHidBackend {
    fn name(&self) -> &'static str {
        self.capabilities.name
    }

    fn capabilities(&self) -> InputCapabilities {
        self.capabilities.clone()
    }

    fn is_ready(&self) -> bool {
        self.ready
    }

    fn inject_keyboard(&mut self, event: &KeyboardEvent) -> Result<()> {
        if !self.ready {
            return Err(InputError::DeviceNotReady);
        }

        // Generate HID report
        let _report = self.keyboard_to_hid_report(event);

        // TODO: Send report to xHCI controller / USB device
        // This will be implemented when xHCI controller is added
        Ok(())
    }

    fn inject_mouse(&mut self, event: &MouseEvent) -> Result<()> {
        if !self.ready {
            return Err(InputError::DeviceNotReady);
        }

        // Generate HID report
        let _report = self.mouse_to_hid_report(event);

        // TODO: Send report to xHCI controller / USB device
        // This will be implemented when xHCI controller is added
        Ok(())
    }
}
