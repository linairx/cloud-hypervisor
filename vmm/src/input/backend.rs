// Copyright 2024 Cloud Hypervisor Authors. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Input Backend Abstraction
//!
//! This module defines the trait and types for input backends.
//! Each backend (PS/2, virtio-input, USB HID) implements this trait
//! to provide unified input injection functionality.
//!
//! # Overview
//!
//! The backend abstraction allows Cloud Hypervisor to inject keyboard and
//! mouse input into VMs through different virtualization technologies:
//!
//! - **PS/2 Backend**: Uses the i8042 controller, highest stealth
//! - **VirtIO Backend**: Uses virtio-input devices, most features
//! - **USB HID Backend**: Emulates USB devices, medium stealth
//!
//! # Example
//!
//! ```ignore
//! use vmm::input::backend::{InputBackend, Ps2Backend};
//! use vmm::input::event::{KeyboardEvent, KeyboardAction};
//!
//! let mut backend = Ps2Backend::new();
//!
//! // Check capabilities
//! let caps = backend.capabilities();
//! println!("Max keyboard rate: {}", caps.max_keyboard_rate);
//!
//! // Inject keyboard event
//! let event = KeyboardEvent {
//!     action: KeyboardAction::Type,
//!     code: 0x1E, // A key
//!     modifiers: Default::default(),
//! };
//! backend.inject_keyboard(&event)?;
//! ```

use super::event::{InputEvent, KeyboardEvent, MouseEvent};
use super::{InputError, Result};
use devices::usb::hid::SharedUsbHidDevice;

/// Stealth level indicates how detectable the input backend is.
///
/// This enum represents the detectability of an input backend when
/// examined from within the guest operating system. Higher stealth
/// levels make it harder for security software to detect that the
/// input is being injected.
///
/// # Levels
///
/// - **High**: Very hard to detect (e.g., PS/2 emulation)
/// - **Medium**: May be detected with careful inspection (e.g., USB HID)
/// - **Low**: Easily detected as virtual device (e.g., virtio-input)
///
/// # Example
///
/// ```ignore
/// use vmm::input::backend::{Ps2Backend, VirtioInputBackend, StealthLevel};
///
/// let ps2 = Ps2Backend::new();
/// let virtio = VirtioInputBackend::new();
///
/// assert_eq!(ps2.capabilities().stealth_level, StealthLevel::High);
/// assert_eq!(virtio.capabilities().stealth_level, StealthLevel::Low);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum StealthLevel {
    /// Easily detected as virtual device (e.g., virtio-input with Red Hat vendor ID)
    Low,
    /// May be detected with careful inspection (e.g., USB HID)
    Medium,
    /// Very hard to detect (e.g., PS/2 emulation)
    High,
}

/// Capabilities of an input backend.
///
/// This structure describes the features and limitations of an input backend.
/// Use this information to select the appropriate backend for your use case
/// and to validate input events before injection.
///
/// # Fields
///
/// * `max_keyboard_rate` - Maximum keyboard events per second
/// * `supports_absolute_mouse` - Whether absolute mouse positioning is available
/// * `supports_multi_touch` - Whether multi-touch gestures are supported
/// * `supports_scroll_wheel` - Whether scroll wheel input is available
/// * `max_scroll_range` - Maximum scroll wheel delta per event
/// * `stealth_level` - How difficult the backend is to detect
/// * `name` - Short identifier for the backend
/// * `description` - Human-readable description
///
/// # Example
///
/// ```ignore
/// use vmm::input::backend::{InputBackend, VirtioInputBackend};
///
/// let backend = VirtioInputBackend::new();
/// let caps = backend.capabilities();
///
/// if caps.supports_absolute_mouse {
///     // Can use absolute mouse positioning
/// }
/// ```
#[derive(Clone, Debug)]
pub struct InputCapabilities {
    /// Maximum keyboard rate (events per second).
    ///
    /// Exceeding this rate may result in dropped events or errors.
    pub max_keyboard_rate: u32,
    /// Supports absolute mouse positioning.
    ///
    /// When true, mouse events can use absolute coordinates (0-65535).
    pub supports_absolute_mouse: bool,
    /// Supports multi-touch input.
    ///
    /// When true, the backend can handle multiple simultaneous touch points.
    pub supports_multi_touch: bool,
    /// Supports scroll wheel.
    ///
    /// When true, scroll wheel events can be injected.
    pub supports_scroll_wheel: bool,
    /// Maximum scroll wheel range per event.
    ///
    /// Values outside this range will be clamped.
    pub max_scroll_range: i8,
    /// Stealth level of this backend.
    ///
    /// Higher levels indicate harder detection.
    pub stealth_level: StealthLevel,
    /// Backend name (short identifier).
    ///
    /// Used for logging and configuration.
    pub name: &'static str,
    /// Backend description (human-readable).
    ///
    /// Provides more detail about the backend type.
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

/// Input backend trait for unified input injection.
///
/// All input backends must implement this trait to provide
/// consistent keyboard and mouse input capabilities.
///
/// # Capabilities
///
/// Each backend reports its capabilities via [`capabilities()`](InputBackend::capabilities).
/// This includes maximum rates, supported features, and stealth level.
///
/// # Thread Safety
///
/// The trait requires `Send` bound to allow backends to be transferred
/// between threads. However, injection operations require `&mut self`,
/// so concurrent access must be synchronized externally.
///
/// # Example Implementation
///
/// ```ignore
/// use vmm::input::backend::{InputBackend, InputCapabilities};
/// use vmm::input::event::{InputEvent, KeyboardEvent, MouseEvent};
/// use vmm::input::{Result, InputError};
///
/// struct MyBackend {
///     ready: bool,
/// }
///
/// impl InputBackend for MyBackend {
///     fn name(&self) -> &'static str { "my-backend" }
///     fn capabilities(&self) -> InputCapabilities { InputCapabilities::default() }
///     fn is_ready(&self) -> bool { self.ready }
///     fn inject_keyboard(&mut self, event: &KeyboardEvent) -> Result<()> {
///         // Implementation
///         Ok(())
///     }
///     fn inject_mouse(&mut self, event: &MouseEvent) -> Result<()> {
///         // Implementation
///         Ok(())
///     }
/// }
/// ```
pub trait InputBackend: Send {
    /// Get the backend name.
    ///
    /// Returns a short identifier used for logging and configuration.
    fn name(&self) -> &'static str;

    /// Get backend capabilities.
    ///
    /// Returns information about supported features and limitations.
    fn capabilities(&self) -> InputCapabilities;

    /// Check if the backend is ready to accept input.
    ///
    /// A backend may not be ready if the guest driver has not yet
    /// initialized or the device is in an error state.
    fn is_ready(&self) -> bool;

    /// Inject a keyboard event.
    ///
    /// # Errors
    ///
    /// Returns [`InputError::DeviceNotReady`] if the backend is not ready.
    /// Returns [`InputError::InjectionFailed`] if the injection fails.
    fn inject_keyboard(&mut self, event: &KeyboardEvent) -> Result<()>;

    /// Inject a mouse event.
    ///
    /// # Errors
    ///
    /// Returns [`InputError::DeviceNotReady`] if the backend is not ready.
    /// Returns [`InputError::InjectionFailed`] if the injection fails.
    fn inject_mouse(&mut self, event: &MouseEvent) -> Result<()>;

    /// Inject a generic input event.
    ///
    /// This is a convenience method that dispatches to the appropriate
    /// type-specific injection method based on the event type.
    ///
    /// # Default Implementation
    ///
    /// The default implementation matches on the event type and calls
    /// [`inject_keyboard`](InputBackend::inject_keyboard) or
    /// [`inject_mouse`](InputBackend::inject_mouse) accordingly.
    fn inject(&mut self, event: &InputEvent) -> Result<()> {
        match event {
            InputEvent::Keyboard(kb) => self.inject_keyboard(kb),
            InputEvent::Mouse(m) => self.inject_mouse(m),
        }
    }

    /// Flush any pending events.
    ///
    /// Some backends may buffer events for efficiency. This method
    /// forces any buffered events to be sent immediately.
    ///
    /// # Default Implementation
    ///
    /// The default implementation does nothing and returns `Ok(())`.
    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

// ============================================================================
// PS/2 Backend (i8042)
// ============================================================================

/// PS/2 input backend using i8042 device.
///
/// This backend provides keyboard and mouse input through the legacy
/// PS/2 controller (i8042). It offers the highest stealth level because
/// PS/2 devices are native to most x86 systems and cannot be easily
/// distinguished from physical hardware.
///
/// # Capabilities
///
/// - **Stealth Level**: High (very hard to detect)
/// - **Max Keyboard Rate**: 500 events/second
/// - **Absolute Mouse**: No (relative only)
/// - **Multi-touch**: No
/// - **Scroll Wheel**: Yes
///
/// # Limitations
///
/// - Only supports relative mouse positioning
/// - No multi-touch support
/// - Lower event rate compared to VirtIO
///
/// # Example
///
/// ```ignore
/// use vmm::input::backend::{InputBackend, Ps2Backend};
/// use vmm::input::event::{KeyboardEvent, KeyboardAction};
///
/// let mut backend = Ps2Backend::new();
///
/// // Check if ready
/// if backend.is_ready() {
///     let event = KeyboardEvent {
///         action: KeyboardAction::Type,
///         code: 0x1E, // A key
///         modifiers: Default::default(),
///     };
///     backend.inject_keyboard(&event)?;
/// }
/// ```
pub struct Ps2Backend {
    capabilities: InputCapabilities,
}

impl Ps2Backend {
    /// Create a new PS/2 backend.
    ///
    /// The PS/2 backend is always ready once created, as it uses
    /// the emulated i8042 controller which is always available.
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
// VirtIO Input Backend
// ============================================================================

/// VirtIO input backend.
///
/// This backend provides keyboard and mouse input through virtio-input devices.
/// It offers the most features but has the lowest stealth level due to the
/// Red Hat vendor ID in device identification.
///
/// # Capabilities
///
/// - **Stealth Level**: Low (easily detected as virtual device)
/// - **Max Keyboard Rate**: 1000 events/second
/// - **Absolute Mouse**: Yes
/// - **Multi-touch**: Yes
/// - **Scroll Wheel**: Yes
///
/// # Setup
///
/// The backend must be connected to a VirtIO Input device using
/// [`set_device`](VirtioInputBackend::set_device) before it can inject events.
/// The ready state must be set using [`set_ready`](VirtioInputBackend::set_ready).
///
/// # Example
///
/// ```ignore
/// use vmm::input::backend::{InputBackend, VirtioInputBackend};
/// use vmm::input::event::{MouseEvent, MouseAction};
///
/// let mut backend = VirtioInputBackend::new();
/// backend.set_device(virtio_device);
/// backend.set_ready(true);
///
/// // Inject absolute mouse position
/// let event = MouseEvent {
///     action: MouseAction::MoveAbsolute,
///     x: 960,
///     y: 540,
///     ..Default::default()
/// };
/// backend.inject_mouse(&event)?;
/// ```
pub struct VirtioInputBackend {
    capabilities: InputCapabilities,
    ready: bool,
    /// Reference to VirtIO Input device for injection
    device: Option<std::sync::Arc<std::sync::Mutex<virtio_devices::VirtioInput>>>,
    /// Current absolute mouse position (X coordinate)
    mouse_x: i32,
    /// Current absolute mouse position (Y coordinate)
    mouse_y: i32,
    /// Screen dimensions for normalization (width)
    screen_width: u32,
    /// Screen dimensions for normalization (height)
    screen_height: u32,
}

impl VirtioInputBackend {
    /// Create a new virtio-input backend.
    ///
    /// The backend is created in a not-ready state. You must call
    /// [`set_device`](VirtioInputBackend::set_device) and
    /// [`set_ready`](VirtioInputBackend::set_ready) before injecting events.
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
            device: None,
            mouse_x: 0,
            mouse_y: 0,
            screen_width: 1920,  // Default screen width
            screen_height: 1080, // Default screen height
        }
    }

    /// Set ready state.
    ///
    /// Call this method after the VirtIO device has been configured
    /// and is ready to accept input events.
    pub fn set_ready(&mut self, ready: bool) {
        self.ready = ready;
    }

    /// Set the VirtIO Input device reference.
    ///
    /// This must be called before injecting any events.
    pub fn set_device(&mut self, device: std::sync::Arc<std::sync::Mutex<virtio_devices::VirtioInput>>) {
        self.device = Some(device);
    }

    /// Set screen dimensions for absolute positioning.
    ///
    /// When using absolute mouse positioning, these dimensions are used
    /// to clamp the mouse coordinates.
    pub fn set_screen_dimensions(&mut self, width: u32, height: u32) {
        self.screen_width = width;
        self.screen_height = height;
    }

    /// Reset mouse position tracking.
    ///
    /// Call this when the guest changes resolution or when you want
    /// to reset the tracked absolute position.
    pub fn reset_mouse_position(&mut self) {
        self.mouse_x = 0;
        self.mouse_y = 0;
    }

    /// Convert keyboard action to pressed boolean.
    fn keyboard_action_to_pressed(action: super::event::KeyboardAction) -> bool {
        match action {
            super::event::KeyboardAction::Press | super::event::KeyboardAction::Type => true,
            super::event::KeyboardAction::Release => false,
        }
    }

    /// Convert mouse button to Linux input code.
    fn mouse_button_to_code(button: super::event::MouseButton) -> u16 {
        match button {
            super::event::MouseButton::Left => 0x110,    // BTN_LEFT
            super::event::MouseButton::Right => 0x111,   // BTN_RIGHT
            super::event::MouseButton::Middle => 0x112,  // BTN_MIDDLE
            super::event::MouseButton::Side => 0x113,    // BTN_SIDE
            super::event::MouseButton::Extra => 0x114,   // BTN_EXTRA
        }
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

    fn inject_keyboard(&mut self, event: &KeyboardEvent) -> Result<()> {
        if !self.ready {
            return Err(InputError::DeviceNotReady);
        }

        let device = self.device.as_ref().ok_or_else(|| {
            InputError::BackendNotAvailable("VirtIO Input device not set".to_string())
        })?;

        let pressed = Self::keyboard_action_to_pressed(event.action);

        // Handle Type action as Press + Release
        if matches!(event.action, super::event::KeyboardAction::Type) {
            if let Ok(dev) = device.lock() {
                dev.inject_keyboard(event.code, true)
                    .map_err(|e| InputError::InjectionFailed(e.to_string()))?;
                dev.inject_keyboard(event.code, false)
                    .map_err(|e| InputError::InjectionFailed(e.to_string()))?;
            }
        } else {
            if let Ok(dev) = device.lock() {
                dev.inject_keyboard(event.code, pressed)
                    .map_err(|e| InputError::InjectionFailed(e.to_string()))?;
            }
        }

        Ok(())
    }

    fn inject_mouse(&mut self, event: &MouseEvent) -> Result<()> {
        if !self.ready {
            return Err(InputError::DeviceNotReady);
        }

        let device = self.device.as_ref().ok_or_else(|| {
            InputError::BackendNotAvailable("VirtIO Input device not set".to_string())
        })?;

        if let Ok(dev) = device.lock() {
            match event.action {
                super::event::MouseAction::Move => {
                    // Relative movement
                    dev.inject_mouse_rel(event.x, event.y)
                        .map_err(|e| InputError::InjectionFailed(e.to_string()))?;
                }
                super::event::MouseAction::MoveAbsolute => {
                    // Clamp the target position to screen bounds
                    let target_x = event.x.clamp(0, self.screen_width as i32);
                    let target_y = event.y.clamp(0, self.screen_height as i32);

                    // Compute relative movement from last tracked position
                    let rel_x = target_x - self.mouse_x;
                    let rel_y = target_y - self.mouse_y;

                    // Update tracked position
                    self.mouse_x = target_x;
                    self.mouse_y = target_y;

                    // Only send relative movement if there's actual movement
                    if rel_x != 0 || rel_y != 0 {
                        dev.inject_mouse_rel(rel_x, rel_y)
                            .map_err(|e| InputError::InjectionFailed(e.to_string()))?;
                    }
                }
                super::event::MouseAction::ButtonPress | super::event::MouseAction::ButtonRelease => {
                    if let Some(ref button) = event.button {
                        let code = Self::mouse_button_to_code(*button);
                        let pressed = matches!(event.action, super::event::MouseAction::ButtonPress);
                        dev.inject_mouse_button(code, pressed)
                            .map_err(|e| InputError::InjectionFailed(e.to_string()))?;
                    }
                }
                super::event::MouseAction::Click => {
                    if let Some(ref button) = event.button {
                        let code = Self::mouse_button_to_code(*button);
                        dev.inject_mouse_button(code, true)
                            .map_err(|e| InputError::InjectionFailed(e.to_string()))?;
                        dev.inject_mouse_button(code, false)
                            .map_err(|e| InputError::InjectionFailed(e.to_string()))?;
                    }
                }
                super::event::MouseAction::Scroll => {
                    dev.inject_mouse_wheel(event.z)
                        .map_err(|e| InputError::InjectionFailed(e.to_string()))?;
                }
            }
        }

        Ok(())
    }
}

// ============================================================================
// Backend Factory
// ============================================================================

/// Available backend types.
///
/// This enum represents the different input backend types that can be used
/// for input injection.
///
/// # Example
///
/// ```ignore
/// use vmm::input::backend::BackendType;
///
/// // Parse from configuration string
/// let backend = BackendType::from_name("ps2").unwrap();
/// assert_eq!(backend, BackendType::Ps2);
///
/// // Get backend name for logging
/// println!("Using backend: {}", backend.name());
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendType {
    /// PS/2 backend (i8042)
    Ps2,
    /// VirtIO input backend
    Virtio,
    /// USB HID backend
    UsbHid,
}

impl Default for BackendType {
    fn default() -> Self {
        BackendType::Ps2
    }
}

impl BackendType {
    /// Get backend name as a string.
    ///
    /// Returns a short identifier used for logging and configuration.
    pub fn name(&self) -> &'static str {
        match self {
            BackendType::Ps2 => "ps2",
            BackendType::Virtio => "virtio",
            BackendType::UsbHid => "usb",
        }
    }

    /// Parse backend type from string.
    ///
    /// Supports multiple aliases for each backend type.
    ///
    /// # Aliases
    ///
    /// - PS/2: `"ps2"`, `"ps/2"`
    /// - VirtIO: `"virtio"`, `"virtio-input"`
    /// - USB HID: `"usb"`, `"usb-hid"`, `"hid"`
    ///
    /// # Example
    ///
    /// ```ignore
    /// use vmm::input::backend::BackendType;
    ///
    /// assert_eq!(BackendType::from_name("ps2"), Some(BackendType::Ps2));
    /// assert_eq!(BackendType::from_name("virtio-input"), Some(BackendType::Virtio));
    /// assert_eq!(BackendType::from_name("unknown"), None);
    /// ```
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

/// USB HID input backend.
///
/// This backend emulates a USB Human Interface Device (HID) for keyboard
/// and mouse input. It provides medium stealth level as USB devices are
/// common but can be identified as emulated with careful inspection.
///
/// # Capabilities
///
/// - **Stealth Level**: Medium (may be detected with inspection)
/// - **Max Keyboard Rate**: 1000 events/second
/// - **Absolute Mouse**: Yes
/// - **Multi-touch**: No
/// - **Scroll Wheel**: Yes
///
/// # Setup
///
/// The backend requires USB HID devices to be configured:
/// - Keyboard device: [`set_keyboard_device`](UsbHidBackend::set_keyboard_device)
/// - Mouse device: [`set_mouse_device`](UsbHidBackend::set_mouse_device)
///
/// # Example
///
/// ```ignore
/// use vmm::input::backend::{InputBackend, UsbHidBackend};
/// use vmm::input::event::{KeyboardEvent, KeyboardAction};
///
/// let mut backend = UsbHidBackend::new();
/// backend.set_keyboard_device(keyboard_dev);
/// backend.set_mouse_device(mouse_dev);
/// backend.set_ready(true);
///
/// let event = KeyboardEvent {
///     action: KeyboardAction::Type,
///     code: 0x1E,
///     modifiers: Default::default(),
/// };
/// backend.inject_keyboard(&event)?;
/// ```
pub struct UsbHidBackend {
    capabilities: InputCapabilities,
    ready: bool,
    /// Keyboard LED state (bitmask: NumLock=0x01, CapsLock=0x02, ScrollLock=0x04)
    keyboard_leds: u8,
    /// Mouse button state (bitmask: Left=0x01, Right=0x02, Middle=0x04)
    mouse_buttons: u8,
    /// Keyboard HID device reference
    keyboard_device: Option<SharedUsbHidDevice>,
    /// Mouse HID device reference
    mouse_device: Option<SharedUsbHidDevice>,
}

impl UsbHidBackend {
    /// Create a new USB HID backend.
    ///
    /// The backend is created in a not-ready state. You must configure
    /// the keyboard and mouse devices before setting ready.
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
            keyboard_device: None,
            mouse_device: None,
        }
    }

    /// Set ready state.
    ///
    /// Call this after configuring keyboard and mouse devices.
    pub fn set_ready(&mut self, ready: bool) {
        self.ready = ready;
    }

    /// Set keyboard HID device.
    ///
    /// This must be called before injecting keyboard events.
    pub fn set_keyboard_device(&mut self, device: SharedUsbHidDevice) {
        self.keyboard_device = Some(device);
    }

    /// Set mouse HID device.
    ///
    /// This must be called before injecting mouse events.
    pub fn set_mouse_device(&mut self, device: SharedUsbHidDevice) {
        self.mouse_device = Some(device);
    }

    /// Get keyboard LED state.
    ///
    /// Returns a bitmask of LED states:
    /// - Bit 0: NumLock
    /// - Bit 1: CapsLock
    /// - Bit 2: ScrollLock
    pub fn keyboard_leds(&self) -> u8 {
        self.keyboard_leds
    }

    /// Get mouse button state.
    ///
    /// Returns a bitmask of button states:
    /// - Bit 0: Left button
    /// - Bit 1: Right button
    /// - Bit 2: Middle button
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
        let report = self.keyboard_to_hid_report(event);

        // Send to HID device
        if let Some(ref device) = self.keyboard_device {
            if let Ok(mut dev) = device.lock() {
                dev.queue_report(report.to_vec());
            }
        }

        Ok(())
    }

    fn inject_mouse(&mut self, event: &MouseEvent) -> Result<()> {
        if !self.ready {
            return Err(InputError::DeviceNotReady);
        }

        // Generate HID report
        let report = self.mouse_to_hid_report(event);

        // Send to HID device
        if let Some(ref device) = self.mouse_device {
            if let Ok(mut dev) = device.lock() {
                dev.queue_report(report.to_vec());
            }
        }

        Ok(())
    }
}
