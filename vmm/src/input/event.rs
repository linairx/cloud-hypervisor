// Copyright 2024 Cloud Hypervisor Authors. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Input Event Types
//!
//! This module defines the input event types used across all backends.
//! Events are designed to be backend-agnostic and support:
//!
//! - Keyboard events (press/release/type)
//! - Mouse events (move/buttons/scroll)
//! - Touch events (planned)
//! - Gamepad events (planned)

use serde::{Deserialize, Serialize};

/// Input device type
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InputDevice {
    Keyboard,
    Mouse,
    Touch,
    Gamepad,
}

/// Input action type
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InputAction {
    /// Key/button press
    Press,
    /// Key/button release
    Release,
    /// Key press and release (convenience)
    Type,
    /// Relative movement
    Move,
    /// Absolute position
    MoveAbsolute,
    /// Scroll/wheel
    Scroll,
}

// ============================================================================
// Keyboard Events
// ============================================================================

/// Keyboard event
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KeyboardEvent {
    /// Action to perform
    pub action: KeyboardAction,
    /// Key code (scancode)
    pub code: u16,
    /// Modifier keys (optional, for complex shortcuts)
    #[serde(default)]
    pub modifiers: KeyboardModifiers,
}

/// Keyboard action
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyboardAction {
    Press,
    Release,
    Type,
}

/// Keyboard modifier states
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct KeyboardModifiers {
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub alt: bool,
    #[serde(default)]
    pub shift: bool,
    #[serde(default)]
    pub meta: bool, // Windows/Super key
}

/// Standard key codes (PC scancodes Set 1)
pub mod keys {
    // Function keys
    pub const ESCAPE: u16 = 0x01;
    pub const F1: u16 = 0x3B;
    pub const F2: u16 = 0x3C;
    pub const F3: u16 = 0x3D;
    pub const F4: u16 = 0x3E;
    pub const F5: u16 = 0x3F;
    pub const F6: u16 = 0x40;
    pub const F7: u16 = 0x41;
    pub const F8: u16 = 0x42;
    pub const F9: u16 = 0x43;
    pub const F10: u16 = 0x44;
    pub const F11: u16 = 0x57;
    pub const F12: u16 = 0x58;

    // Number row
    pub const KEY_1: u16 = 0x02;
    pub const KEY_2: u16 = 0x03;
    pub const KEY_3: u16 = 0x04;
    pub const KEY_4: u16 = 0x05;
    pub const KEY_5: u16 = 0x06;
    pub const KEY_6: u16 = 0x07;
    pub const KEY_7: u16 = 0x08;
    pub const KEY_8: u16 = 0x09;
    pub const KEY_9: u16 = 0x0A;
    pub const KEY_0: u16 = 0x0B;
    pub const MINUS: u16 = 0x0C;
    pub const EQUALS: u16 = 0x0D;
    pub const BACKSPACE: u16 = 0x0E;

    // Top letter row
    pub const TAB: u16 = 0x0F;
    pub const Q: u16 = 0x10;
    pub const W: u16 = 0x11;
    pub const E: u16 = 0x12;
    pub const R: u16 = 0x13;
    pub const T: u16 = 0x14;
    pub const Y: u16 = 0x15;
    pub const U: u16 = 0x16;
    pub const I: u16 = 0x17;
    pub const O: u16 = 0x18;
    pub const P: u16 = 0x19;
    pub const LEFT_BRACKET: u16 = 0x1A;
    pub const RIGHT_BRACKET: u16 = 0x1B;
    pub const ENTER: u16 = 0x1C;

    // Middle letter row
    pub const CAPS_LOCK: u16 = 0x3A;
    pub const A: u16 = 0x1E;
    pub const S: u16 = 0x1F;
    pub const D: u16 = 0x20;
    pub const F: u16 = 0x21;
    pub const G: u16 = 0x22;
    pub const H: u16 = 0x23;
    pub const J: u16 = 0x24;
    pub const K: u16 = 0x25;
    pub const L: u16 = 0x26;
    pub const SEMICOLON: u16 = 0x27;
    pub const APOSTROPHE: u16 = 0x28;
    pub const GRAVE: u16 = 0x29;

    // Bottom letter row
    pub const LEFT_SHIFT: u16 = 0x2A;
    pub const BACKSLASH: u16 = 0x2B;
    pub const Z: u16 = 0x2C;
    pub const X: u16 = 0x2D;
    pub const C: u16 = 0x2E;
    pub const V: u16 = 0x2F;
    pub const B: u16 = 0x30;
    pub const N: u16 = 0x31;
    pub const M: u16 = 0x32;
    pub const COMMA: u16 = 0x33;
    pub const PERIOD: u16 = 0x34;
    pub const SLASH: u16 = 0x35;
    pub const RIGHT_SHIFT: u16 = 0x36;

    // Bottom row
    pub const LEFT_CTRL: u16 = 0x1D;
    pub const LEFT_ALT: u16 = 0x38;
    pub const SPACE: u16 = 0x39;
    pub const RIGHT_ALT: u16 = 0xE038; // Extended
    pub const RIGHT_CTRL: u16 = 0xE01D; // Extended

    // Navigation
    pub const INSERT: u16 = 0xE052;
    pub const DELETE: u16 = 0xE053;
    pub const HOME: u16 = 0xE047;
    pub const END: u16 = 0xE04F;
    pub const PAGE_UP: u16 = 0xE049;
    pub const PAGE_DOWN: u16 = 0xE051;
    pub const UP: u16 = 0xE048;
    pub const DOWN: u16 = 0xE050;
    pub const LEFT: u16 = 0xE04B;
    pub const RIGHT: u16 = 0xE04D;

    // Numpad
    pub const NUM_LOCK: u16 = 0x45;
    pub const KP_DIVIDE: u16 = 0xE035;
    pub const KP_MULTIPLY: u16 = 0x37;
    pub const KP_SUBTRACT: u16 = 0x4A;
    pub const KP_ADD: u16 = 0x4E;
    pub const KP_ENTER: u16 = 0xE01C;
    pub const KP_DECIMAL: u16 = 0x53;
    pub const KP_0: u16 = 0x52;
    pub const KP_1: u16 = 0x4F;
    pub const KP_2: u16 = 0x50;
    pub const KP_3: u16 = 0x51;
    pub const KP_4: u16 = 0x4B;
    pub const KP_5: u16 = 0x4C;
    pub const KP_6: u16 = 0x4D;
    pub const KP_7: u16 = 0x47;
    pub const KP_8: u16 = 0x48;
    pub const KP_9: u16 = 0x49;
}

// ============================================================================
// Mouse Events
// ============================================================================

/// Mouse event
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MouseEvent {
    /// Action to perform
    pub action: MouseAction,
    /// X coordinate or delta
    #[serde(default)]
    pub x: i32,
    /// Y coordinate or delta
    #[serde(default)]
    pub y: i32,
    /// Z coordinate (scroll wheel) or delta
    #[serde(default)]
    pub z: i32,
    /// Button (for button actions)
    #[serde(default)]
    pub button: Option<MouseButton>,
    /// All button states (for move with buttons)
    #[serde(default)]
    pub buttons: MouseButtons,
}

/// Mouse action
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseAction {
    /// Relative movement
    Move,
    /// Absolute position (0-65535 range)
    MoveAbsolute,
    /// Button press
    ButtonPress,
    /// Button release
    ButtonRelease,
    /// Button click (press + release)
    Click,
    /// Scroll wheel
    Scroll,
}

/// Mouse button identifier
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Side,   // Side button (typically back)
    Extra,  // Extra button (typically forward)
}

/// Mouse button states
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct MouseButtons {
    #[serde(default)]
    pub left: bool,
    #[serde(default)]
    pub right: bool,
    #[serde(default)]
    pub middle: bool,
    #[serde(default)]
    pub side: bool,
    #[serde(default)]
    pub extra: bool,
}

impl MouseButtons {
    /// Check if any button is pressed
    pub fn any(&self) -> bool {
        self.left || self.right || self.middle || self.side || self.extra
    }

    /// Get button count
    pub fn count(&self) -> u8 {
        let mut count = 0;
        if self.left {
            count += 1;
        }
        if self.right {
            count += 1;
        }
        if self.middle {
            count += 1;
        }
        if self.side {
            count += 1;
        }
        if self.extra {
            count += 1;
        }
        count
    }
}

// ============================================================================
// Generic Input Event
// ============================================================================

/// Generic input event (union of all event types)
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "device", rename_all = "lowercase")]
pub enum InputEvent {
    Keyboard(KeyboardEvent),
    Mouse(MouseEvent),
}

impl InputEvent {
    /// Create a keyboard event
    pub fn keyboard(action: KeyboardAction, code: u16) -> Self {
        InputEvent::Keyboard(KeyboardEvent {
            action,
            code,
            modifiers: KeyboardModifiers::default(),
        })
    }

    /// Create a mouse move event
    pub fn mouse_move(dx: i32, dy: i32) -> Self {
        InputEvent::Mouse(MouseEvent {
            action: MouseAction::Move,
            x: dx,
            y: dy,
            z: 0,
            button: None,
            buttons: MouseButtons::default(),
        })
    }

    /// Create a mouse button event
    pub fn mouse_button(button: MouseButton, pressed: bool) -> Self {
        InputEvent::Mouse(MouseEvent {
            action: if pressed {
                MouseAction::ButtonPress
            } else {
                MouseAction::ButtonRelease
            },
            x: 0,
            y: 0,
            z: 0,
            button: Some(button),
            buttons: MouseButtons::default(),
        })
    }

    /// Create a mouse scroll event
    pub fn mouse_scroll(delta: i32) -> Self {
        InputEvent::Mouse(MouseEvent {
            action: MouseAction::Scroll,
            x: 0,
            y: 0,
            z: delta,
            button: None,
            buttons: MouseButtons::default(),
        })
    }
}

// ============================================================================
// Batch Input Request
// ============================================================================

/// Batch input injection request
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InputRequest {
    /// Optional backend override
    #[serde(default)]
    pub backend: Option<String>,
    /// Keyboard events to inject
    #[serde(default)]
    pub keyboard: Vec<KeyboardEvent>,
    /// Mouse events to inject
    #[serde(default)]
    pub mouse: Vec<MouseEvent>,
}

impl InputRequest {
    /// Check if request is empty
    pub fn is_empty(&self) -> bool {
        self.keyboard.is_empty() && self.mouse.is_empty()
    }

    /// Count total events
    pub fn event_count(&self) -> usize {
        self.keyboard.len() + self.mouse.len()
    }
}
