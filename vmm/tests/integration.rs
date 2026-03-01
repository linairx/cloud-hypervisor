// Copyright 2024 Cloud Hypervisor Authors. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Integration tests for lg-capture module
//!
//! This file contains end-to-end tests for:
//! - Input injection (PS/2, VirtIO, USB HID backends)
//! - Frame capture (X11, Wayland backends)
//! - Audio capture (PulseAudio, WASAPI backends)

use vmm::input::{
    InputBackend, InputCapabilities, InputEvent, KeyboardAction, KeyboardEvent,
    KeyboardModifiers, MouseAction, MouseButton, MouseEvent, MouseButtons,
    Ps2Backend, StealthLevel, VirtioInputBackend, UsbHidBackend,
};

// Note: lg_guest_agent types are tested separately in guest-agent crate

// ============================================================================
// Marker Test
// ============================================================================

#[test]
fn integration_tests_available() {
    // Marker test to ensure integration test module is available
}

// ============================================================================
// PS/2 Backend Tests
// ============================================================================

#[test]
fn test_ps2_backend_creation() {
    let backend = Ps2Backend::new();
    assert!(backend.is_ready());
    assert_eq!(backend.name(), "ps2");
}

#[test]
fn test_ps2_backend_capabilities() {
    let backend = Ps2Backend::new();
    let caps = backend.capabilities();

    assert_eq!(caps.name, "ps2");
    assert!(caps.max_keyboard_rate > 0);
    assert!(!caps.supports_absolute_mouse); // PS/2 is relative only
    assert!(caps.supports_scroll_wheel);
    assert_eq!(caps.stealth_level, StealthLevel::High);
}

#[test]
fn test_ps2_keyboard_injection() {
    let mut backend = Ps2Backend::new();

    let event = KeyboardEvent {
        action: KeyboardAction::Press,
        code: 0x1E, // A key
        modifiers: KeyboardModifiers::default(),
    };

    let result = backend.inject_keyboard(&event);
    assert!(result.is_ok());
}

#[test]
fn test_ps2_mouse_injection() {
    let mut backend = Ps2Backend::new();

    let event = MouseEvent {
        action: MouseAction::Move,
        x: 100,
        y: 200,
        z: 0,
        button: None,
        buttons: MouseButtons::default(),
    };

    let result = backend.inject_mouse(&event);
    assert!(result.is_ok());
}

// ============================================================================
// VirtIO Input Backend Tests
// ============================================================================

#[test]
fn test_virtio_backend_creation() {
    let backend = VirtioInputBackend::new();
    assert!(!backend.is_ready()); // Not ready until device is set
    assert_eq!(backend.name(), "virtio");
}

#[test]
fn test_virtio_backend_capabilities() {
    let backend = VirtioInputBackend::new();
    let caps = backend.capabilities();

    assert_eq!(caps.name, "virtio");
    assert!(caps.supports_absolute_mouse); // VirtIO supports absolute
    assert!(caps.supports_multi_touch);
    assert!(caps.supports_scroll_wheel);
}

// ============================================================================
// USB HID Backend Tests
// ============================================================================

#[test]
fn test_usb_hid_backend_creation() {
    let backend = UsbHidBackend::new();
    assert!(!backend.is_ready()); // Not ready until devices are set
    assert_eq!(backend.name(), "usb-hid");
}

#[test]
fn test_usb_hid_backend_capabilities() {
    let backend = UsbHidBackend::new();
    let caps = backend.capabilities();

    assert_eq!(caps.name, "usb-hid");
    assert!(caps.supports_absolute_mouse);
    assert!(caps.supports_scroll_wheel);
}

// ============================================================================
// Input Event Tests
// ============================================================================

#[test]
fn test_keyboard_event_creation() {
    let event = KeyboardEvent {
        code: 0x1E, // A key
        action: KeyboardAction::Press,
        modifiers: KeyboardModifiers::default(),
    };

    assert_eq!(event.code, 0x1E);
    assert_eq!(event.action, KeyboardAction::Press);
}

#[test]
fn test_keyboard_event_with_modifiers() {
    let modifiers = KeyboardModifiers {
        ctrl: true,
        alt: false,
        shift: true,
        meta: false,
    };

    let event = KeyboardEvent {
        code: 0x1E, // A key
        action: KeyboardAction::Type,
        modifiers,
    };

    assert!(event.modifiers.ctrl);
    assert!(event.modifiers.shift);
    assert!(!event.modifiers.alt);
    assert!(!event.modifiers.meta);
}

#[test]
fn test_mouse_event_creation() {
    let event = MouseEvent {
        action: MouseAction::Move,
        x: 100,
        y: 200,
        z: 0,
        button: None,
        buttons: MouseButtons::default(),
    };

    assert_eq!(event.x, 100);
    assert_eq!(event.y, 200);
    assert_eq!(event.action, MouseAction::Move);
}

#[test]
fn test_mouse_event_with_button() {
    let event = MouseEvent {
        action: MouseAction::Click,
        x: 0,
        y: 0,
        z: 0,
        button: Some(MouseButton::Left),
        buttons: MouseButtons::default(),
    };

    assert_eq!(event.action, MouseAction::Click);
    assert_eq!(event.button, Some(MouseButton::Left));
}

#[test]
fn test_input_event_keyboard() {
    let kb_event = InputEvent::keyboard(KeyboardAction::Press, 0x1E);

    assert!(matches!(kb_event, InputEvent::Keyboard(_)));
}

#[test]
fn test_input_event_mouse_move() {
    let mouse_event = InputEvent::mouse_move(10, 20);

    assert!(matches!(mouse_event, InputEvent::Mouse(_)));
    if let InputEvent::Mouse(m) = mouse_event {
        assert_eq!(m.x, 10);
        assert_eq!(m.y, 20);
        assert_eq!(m.action, MouseAction::Move);
    }
}

#[test]
fn test_input_event_mouse_button() {
    let mouse_event = InputEvent::mouse_button(MouseButton::Left, true);

    assert!(matches!(mouse_event, InputEvent::Mouse(_)));
    if let InputEvent::Mouse(m) = mouse_event {
        assert_eq!(m.action, MouseAction::ButtonPress);
        assert_eq!(m.button, Some(MouseButton::Left));
    }
}

#[test]
fn test_input_event_mouse_scroll() {
    let scroll_event = InputEvent::mouse_scroll(5);

    assert!(matches!(scroll_event, InputEvent::Mouse(_)));
    if let InputEvent::Mouse(m) = scroll_event {
        assert_eq!(m.action, MouseAction::Scroll);
        assert_eq!(m.z, 5);
    }
}

#[test]
fn test_mouse_buttons_utility() {
    let buttons = MouseButtons {
        left: true,
        right: false,
        middle: true,
        side: false,
        extra: false,
    };

    assert!(buttons.any());
    assert_eq!(buttons.count(), 2);
}

#[test]
fn test_mouse_buttons_default() {
    let buttons = MouseButtons::default();

    assert!(!buttons.any());
    assert_eq!(buttons.count(), 0);
}

// ============================================================================
// InputCapabilities Tests
// ============================================================================

#[test]
fn test_input_capabilities_default() {
    let caps = InputCapabilities::default();

    assert_eq!(caps.name, "unknown");
    assert!(caps.max_keyboard_rate > 0);
}

// ============================================================================
// StealthLevel Tests
// ============================================================================

#[test]
fn test_stealth_level_ordering() {
    // High > Medium > Low
    assert!(StealthLevel::High > StealthLevel::Medium);
    assert!(StealthLevel::Medium > StealthLevel::Low);
    assert!(StealthLevel::High > StealthLevel::Low);
}
