// Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE-BSD-3-Clause file.
//
// SPDX-License-Identifier: Apache-2.0 AND BSD-3-Clause

//! i8042 PS/2 Controller Device
//!
//! This module implements an extended i8042 PS/2 controller that supports:
//! - System reset (original functionality)
//! - Keyboard input injection
//! - Mouse input injection (PS/2 Intellimouse protocol)
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    I8042Device                              │
//! │  ┌─────────────────────────────────────────────────────────┐│
//! │  │                 Registers                               ││
//! │  │  0x60: Data Port (read/write)                           ││
//! │  │  0x64: Command/Status Port                              ││
//! │  └─────────────────────────────────────────────────────────┘│
//! │                                                             │
//! │  ┌─────────────────────────────────────────────────────────┐│
//! │  │                 Buffers                                 ││
//! │  │  Keyboard Buffer → IRQ1                                 ││
//! │  │  Mouse Buffer → IRQ12                                   ││
//! │  └─────────────────────────────────────────────────────────┘│
//! └─────────────────────────────────────────────────────────────┘
//! ```

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

use log::{debug, error, info, warn};
use vm_device::interrupt::InterruptSourceGroup;
use vm_device::BusDevice;
use vmm_sys_util::eventfd::EventFd;

// ============================================================================
// Constants
// ============================================================================

/// I8042 Data Port offset (port 0x60)
const I8042_DATA_REG: u64 = 0;

/// I8042 Command/Status Port offset (port 0x64)
const I8042_COMMAND_REG: u64 = 4;

/// Maximum buffer size for keyboard and mouse
const MAX_BUFFER_SIZE: usize = 16;

/// PS/2 keyboard scancode set 1: release key prefix
const SCANCODE_RELEASE_PREFIX: u8 = 0xF0;

/// PS/2 mouse packet size (Intellimouse: 4 bytes)
const MOUSE_PACKET_SIZE: usize = 4;

// ============================================================================
// PS/2 Commands
// ============================================================================

/// PS/2 controller commands
mod cmd {
    pub const READ_CMD_BYTE: u8 = 0x20;
    pub const WRITE_CMD_BYTE: u8 = 0x60;
    pub const DISABLE_MOUSE: u8 = 0xA7;
    pub const ENABLE_MOUSE: u8 = 0xA8;
    pub const TEST_MOUSE: u8 = 0xA9;
    pub const SELF_TEST: u8 = 0xAA;
    pub const TEST_KBD: u8 = 0xAB;
    pub const DISABLE_KBD: u8 = 0xAD;
    pub const ENABLE_KBD: u8 = 0xAE;
    pub const WRITE_TO_MOUSE: u8 = 0xD4;
}

/// Controller Command Byte bits
mod ccb {
    pub const KBD_INT: u8 = 0x01; // Enable keyboard interrupt
    pub const MOUSE_INT: u8 = 0x02; // Enable mouse interrupt
    pub const SYS_FLAG: u8 = 0x04; // System flag
    pub const KBD_DISABLE: u8 = 0x10; // Disable keyboard
    pub const MOUSE_DISABLE: u8 = 0x20; // Disable mouse
    pub const KBD_TRANSLATE: u8 = 0x40; // Translate scancodes
}

/// Status Register bits
mod status {
    pub const OUT_FULL: u8 = 0x01; // Output buffer full
    pub const IN_FULL: u8 = 0x02; // Input buffer full
    pub const SYS_FLAG: u8 = 0x04; // System flag
    pub const CMD_DATA: u8 = 0x08; // Command/Data
    pub const KBD_LOCK: u8 = 0x10; // Keyboard locked
    pub const MOUSE_OUT: u8 = 0x20; // Mouse output buffer
    pub const TIMEOUT: u8 = 0x40; // Timeout error
    pub const PARITY: u8 = 0x80; // Parity error
}

// ============================================================================
// Mouse Button Flags
// ============================================================================

/// PS/2 mouse button bits for byte 1 of the packet
mod mouse_btn {
    pub const LEFT: u8 = 0x01;
    pub const RIGHT: u8 = 0x02;
    pub const MIDDLE: u8 = 0x04;
    pub const ALWAYS_1: u8 = 0x08;
    pub const X_SIGN: u8 = 0x10;
    pub const Y_SIGN: u8 = 0x20;
    pub const X_OVERFLOW: u8 = 0x40;
    pub const Y_OVERFLOW: u8 = 0x40;
}

// ============================================================================
// Input Event Types
// ============================================================================

/// Keyboard input event
#[derive(Clone, Debug)]
pub struct KeyboardEvent {
    /// Scancode (PS/2 Set 2 format)
    pub scancode: u8,
    /// Whether this is a key release event
    pub release: bool,
}

/// Mouse input event
#[derive(Clone, Debug, Default)]
pub struct MouseEvent {
    /// X movement delta (-255 to 255)
    pub dx: i16,
    /// Y movement delta (-255 to 255)
    pub dy: i16,
    /// Scroll wheel delta (-8 to 7)
    pub dz: i8,
    /// Button states
    pub buttons: MouseButtons,
}

/// Mouse button states
#[derive(Clone, Debug, Default)]
pub struct MouseButtons {
    pub left: bool,
    pub right: bool,
    pub middle: bool,
}

// ============================================================================
// I8042 Device
// ============================================================================

/// Extended i8042 PS/2 Controller Device
///
/// Supports keyboard and mouse input injection in addition to the original
/// reset functionality.
pub struct I8042Device {
    // Original reset functionality
    reset_evt: EventFd,
    vcpus_kill_signalled: Arc<AtomicBool>,

    // Controller state
    status: u8,
    command_byte: u8,
    pending_command: Option<u8>,

    // Keyboard state
    kbd_buffer: VecDeque<u8>,
    kbd_interrupt: Option<Arc<dyn InterruptSourceGroup>>,

    // Mouse state
    mouse_buffer: VecDeque<u8>,
    mouse_interrupt: Option<Arc<dyn InterruptSourceGroup>>,
    mouse_buttons: MouseButtons,

    // Device identification
    mouse_id: u8, // 0x00: standard, 0x03: Intellimouse, 0x04: Intellimouse Explorer
}

impl I8042Device {
    /// Constructs an i8042 device with reset event support
    pub fn new(reset_evt: EventFd, vcpus_kill_signalled: Arc<AtomicBool>) -> Self {
        Self {
            reset_evt,
            vcpus_kill_signalled,
            status: status::SYS_FLAG | status::KBD_LOCK,
            command_byte: ccb::SYS_FLAG | ccb::KBD_INT | ccb::MOUSE_INT,
            pending_command: None,
            kbd_buffer: VecDeque::with_capacity(MAX_BUFFER_SIZE),
            kbd_interrupt: None,
            mouse_buffer: VecDeque::with_capacity(MAX_BUFFER_SIZE),
            mouse_interrupt: None,
            mouse_buttons: MouseButtons::default(),
            mouse_id: 0x03, // Intellimouse (supports scroll wheel)
        }
    }

    /// Set the keyboard interrupt source
    pub fn set_keyboard_interrupt(&mut self, interrupt: Arc<dyn InterruptSourceGroup>) {
        self.kbd_interrupt = Some(interrupt);
    }

    /// Set the mouse interrupt source
    pub fn set_mouse_interrupt(&mut self, interrupt: Arc<dyn InterruptSourceGroup>) {
        self.mouse_interrupt = Some(interrupt);
    }

    // ========================================================================
    // Keyboard Input Injection
    // ========================================================================

    /// Inject a keyboard event
    ///
    /// # Arguments
    /// * `event` - The keyboard event to inject
    ///
    /// # Example
    /// ```ignore
    /// // Press 'A' key
    /// device.inject_keyboard(KeyboardEvent { scancode: 0x1C, release: false });
    ///
    /// // Release 'A' key
    /// device.inject_keyboard(KeyboardEvent { scancode: 0x1C, release: true });
    /// ```
    pub fn inject_keyboard(&mut self, event: KeyboardEvent) {
        // Check if keyboard is disabled
        if self.command_byte & ccb::KBD_DISABLE != 0 {
            debug!("Keyboard disabled, ignoring input");
            return;
        }

        // Check buffer space
        if self.kbd_buffer.len() >= MAX_BUFFER_SIZE {
            warn!("Keyboard buffer overflow, dropping event");
            return;
        }

        // PS/2 Set 2 scancode format
        // Release: 0xF0 followed by scancode
        if event.release {
            self.kbd_buffer.push_back(SCANCODE_RELEASE_PREFIX);
        }
        self.kbd_buffer.push_back(event.scancode);

        debug!(
            "Injected keyboard event: scancode=0x{:02X}, release={}",
            event.scancode, event.release
        );

        // Trigger keyboard interrupt
        self.trigger_keyboard_interrupt();
    }

    /// Inject raw scancode bytes directly
    pub fn inject_keyboard_bytes(&mut self, bytes: &[u8]) {
        if self.command_byte & ccb::KBD_DISABLE != 0 {
            return;
        }

        for &byte in bytes {
            if self.kbd_buffer.len() < MAX_BUFFER_SIZE {
                self.kbd_buffer.push_back(byte);
            }
        }

        self.trigger_keyboard_interrupt();
    }

    /// Trigger keyboard interrupt (IRQ1)
    fn trigger_keyboard_interrupt(&mut self) {
        // Set output buffer full flag
        self.status |= status::OUT_FULL;
        // Clear mouse output flag (keyboard data)
        self.status &= !status::MOUSE_OUT;

        // Trigger IRQ1 if enabled
        if self.command_byte & ccb::KBD_INT != 0 {
            if let Some(ref intr) = self.kbd_interrupt {
                if let Err(e) = intr.trigger(0) {
                    error!("Failed to trigger keyboard interrupt: {}", e);
                }
            }
        }
    }

    // ========================================================================
    // Mouse Input Injection
    // ========================================================================

    /// Inject a mouse event
    ///
    /// # Arguments
    /// * `event` - The mouse event to inject
    ///
    /// # Example
    /// ```ignore
    /// // Move mouse right and down
    /// device.inject_mouse(MouseEvent {
    ///     dx: 10, dy: 10, dz: 0,
    ///     buttons: MouseButtons::default(),
    /// });
    ///
    /// // Left click
    /// device.inject_mouse(MouseEvent {
    ///     dx: 0, dy: 0, dz: 0,
    ///     buttons: MouseButtons { left: true, ..Default::default() },
    /// });
    /// ```
    pub fn inject_mouse(&mut self, event: MouseEvent) {
        // Check if mouse is disabled
        if self.command_byte & ccb::MOUSE_DISABLE != 0 {
            debug!("Mouse disabled, ignoring input");
            return;
        }

        // Update button state
        self.mouse_buttons = event.buttons.clone();

        // Build PS/2 Intellimouse packet (4 bytes)
        let packet = self.build_mouse_packet(&event);

        // Check buffer space
        if self.mouse_buffer.len() + MOUSE_PACKET_SIZE > MAX_BUFFER_SIZE {
            warn!("Mouse buffer overflow, dropping event");
            return;
        }

        // Add packet to buffer
        for byte in packet {
            self.mouse_buffer.push_back(byte);
        }

        debug!(
            "Injected mouse event: dx={}, dy={}, dz={}, buttons=({},{},{})",
            event.dx, event.dy, event.dz,
            event.buttons.left, event.buttons.right, event.buttons.middle
        );

        // Trigger mouse interrupt
        self.trigger_mouse_interrupt();
    }

    /// Build a PS/2 Intellimouse packet
    fn build_mouse_packet(&self, event: &MouseEvent) -> [u8; 4] {
        let mut byte1 = mouse_btn::ALWAYS_1;

        // Button states
        if event.buttons.left {
            byte1 |= mouse_btn::LEFT;
        }
        if event.buttons.right {
            byte1 |= mouse_btn::RIGHT;
        }
        if event.buttons.middle {
            byte1 |= mouse_btn::MIDDLE;
        }

        // Movement signs and overflow
        let dx = event.dx.clamp(-255, 255) as i8;
        let dy = event.dy.clamp(-255, 255) as i8;

        if dx < 0 {
            byte1 |= mouse_btn::X_SIGN;
        }
        if dy < 0 {
            byte1 |= mouse_btn::Y_SIGN;
        }
        // Note: overflow bits are typically not used

        // Y axis is inverted in PS/2 protocol
        let dy_inverted = -dy as u8;

        // Scroll wheel (4-bit signed)
        let dz = (event.dz.clamp(-8, 7) as i8) as u8;

        [byte1, dx as u8, dy_inverted, dz]
    }

    /// Trigger mouse interrupt (IRQ12)
    fn trigger_mouse_interrupt(&mut self) {
        // Set output buffer full and mouse output flags
        self.status |= status::OUT_FULL | status::MOUSE_OUT;

        // Trigger IRQ12 if enabled
        if self.command_byte & ccb::MOUSE_INT != 0 {
            if let Some(ref intr) = self.mouse_interrupt {
                if let Err(e) = intr.trigger(0) {
                    error!("Failed to trigger mouse interrupt: {}", e);
                }
            }
        }
    }

    // ========================================================================
    // Internal Helpers
    // ========================================================================

    /// Handle controller command
    fn handle_command(&mut self, cmd: u8) {
        debug!("i8042 command: 0x{:02X}", cmd);

        match cmd {
            cmd::READ_CMD_BYTE => {
                // Put command byte in output buffer
                self.kbd_buffer.clear();
                self.kbd_buffer.push_back(self.command_byte);
                self.status |= status::OUT_FULL;
            }
            cmd::WRITE_CMD_BYTE => {
                // Next byte is command byte
                self.pending_command = Some(cmd);
            }
            cmd::DISABLE_MOUSE => {
                self.command_byte |= ccb::MOUSE_DISABLE;
            }
            cmd::ENABLE_MOUSE => {
                self.command_byte &= !ccb::MOUSE_DISABLE;
            }
            cmd::SELF_TEST => {
                // Test passed
                self.kbd_buffer.clear();
                self.kbd_buffer.push_back(0x55);
                self.status |= status::OUT_FULL;
            }
            cmd::TEST_KBD => {
                // Test passed
                self.kbd_buffer.clear();
                self.kbd_buffer.push_back(0x00);
                self.status |= status::OUT_FULL;
            }
            cmd::TEST_MOUSE => {
                // Test passed
                self.kbd_buffer.clear();
                self.kbd_buffer.push_back(0x00);
                self.status |= status::OUT_FULL;
            }
            cmd::DISABLE_KBD => {
                self.command_byte |= ccb::KBD_DISABLE;
            }
            cmd::ENABLE_KBD => {
                self.command_byte &= !ccb::KBD_DISABLE;
            }
            cmd::WRITE_TO_MOUSE => {
                // Next byte goes to mouse
                self.pending_command = Some(cmd);
            }
            0xFE => {
                // CPU reset (original functionality)
                info!("i8042 reset signalled");
                if let Err(e) = self.reset_evt.write(1) {
                    error!("Error triggering i8042 reset event: {e}");
                }
                // Wait for VMM to handle reset
                while !self.vcpus_kill_signalled.load(Ordering::SeqCst) {
                    thread::sleep(std::time::Duration::from_millis(1));
                }
            }
            _ => {
                warn!("Unknown i8042 command: 0x{:02X}", cmd);
            }
        }
    }

    /// Handle data write (to keyboard or mouse)
    fn handle_data_write(&mut self, data: u8) {
        if let Some(cmd) = self.pending_command.take() {
            match cmd {
                cmd::WRITE_CMD_BYTE => {
                    self.command_byte = data;
                    debug!("Command byte set to: 0x{:02X}", data);
                }
                cmd::WRITE_TO_MOUSE => {
                    // Mouse command/response
                    debug!("Mouse data: 0x{:02X}", data);
                    // Acknowledge
                    self.mouse_buffer.clear();
                    self.mouse_buffer.push_back(0xFA); // ACK
                    self.trigger_mouse_interrupt();
                }
                _ => {}
            }
        } else {
            // Keyboard data
            debug!("Keyboard data: 0x{:02X}", data);
        }
    }
}

// ============================================================================
// BusDevice Implementation
// ============================================================================

impl BusDevice for I8042Device {
    fn read(&mut self, _base: u64, offset: u64, data: &mut [u8]) {
        if data.len() != 1 {
            return;
        }

        match offset {
            // Data Port (0x60)
            I8042_DATA_REG => {
                // Check if mouse data is available
                if self.status & status::MOUSE_OUT != 0 && !self.mouse_buffer.is_empty() {
                    data[0] = self.mouse_buffer.pop_front().unwrap_or(0);
                    if self.mouse_buffer.is_empty() {
                        self.status &= !(status::OUT_FULL | status::MOUSE_OUT);
                    }
                } else if !self.kbd_buffer.is_empty() {
                    data[0] = self.kbd_buffer.pop_front().unwrap_or(0);
                    if self.kbd_buffer.is_empty() {
                        self.status &= !status::OUT_FULL;
                    }
                } else {
                    data[0] = 0;
                }
                debug!("i8042 data read: 0x{:02X}", data[0]);
            }

            // Command/Status Port (0x64)
            I8042_COMMAND_REG => {
                data[0] = self.status;
                // Clear input buffer full flag on read
                self.status &= !status::IN_FULL;
            }

            _ => {
                data[0] = 0;
            }
        }
    }

    fn write(&mut self, _base: u64, offset: u64, data: &[u8]) -> Option<Arc<Barrier>> {
        if data.len() != 1 {
            return None;
        }

        match offset {
            // Data Port (0x60)
            I8042_DATA_REG => {
                self.handle_data_write(data[0]);
            }

            // Command Port (0x64)
            I8042_COMMAND_REG => {
                self.handle_command(data[0]);
            }

            _ => {}
        }

        None
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use vm_device::interrupt::{InterruptIndex, InterruptSourceConfig};
    use vmm_sys_util::eventfd::EventFd;

    struct TestInterrupt {
        event_fd: EventFd,
    }

    impl InterruptSourceGroup for TestInterrupt {
        fn trigger(&self, _index: InterruptIndex) -> std::result::Result<(), std::io::Error> {
            self.event_fd.write(1)
        }
        fn update(
            &self,
            _index: InterruptIndex,
            _config: InterruptSourceConfig,
            _masked: bool,
            _set_gsi: bool,
        ) -> std::result::Result<(), std::io::Error> {
            Ok(())
        }
        fn set_gsi(&self) -> std::result::Result<(), std::io::Error> {
            Ok(())
        }
        fn notifier(&self, _index: InterruptIndex) -> Option<EventFd> {
            Some(self.event_fd.try_clone().unwrap())
        }
    }

    fn create_test_device() -> I8042Device {
        let reset_evt = EventFd::new(0).unwrap();
        let vcpus_kill = Arc::new(AtomicBool::new(false));
        I8042Device::new(reset_evt, vcpus_kill)
    }

    #[test]
    fn test_keyboard_injection() {
        let mut dev = create_test_device();
        let intr = Arc::new(TestInterrupt {
            event_fd: EventFd::new(0).unwrap(),
        });
        dev.set_keyboard_interrupt(intr.clone());

        // Press 'A' key (scancode 0x1C in Set 2)
        dev.inject_keyboard(KeyboardEvent {
            scancode: 0x1C,
            release: false,
        });

        // Read back the scancode
        let mut data = [0u8; 1];
        dev.read(0, I8042_DATA_REG, &mut data);
        assert_eq!(data[0], 0x1C);
    }

    #[test]
    fn test_keyboard_release() {
        let mut dev = create_test_device();
        let intr = Arc::new(TestInterrupt {
            event_fd: EventFd::new(0).unwrap(),
        });
        dev.set_keyboard_interrupt(intr);

        // Release 'A' key
        dev.inject_keyboard(KeyboardEvent {
            scancode: 0x1C,
            release: true,
        });

        // Read back the release sequence (0xF0 0x1C)
        let mut data = [0u8; 1];
        dev.read(0, I8042_DATA_REG, &mut data);
        assert_eq!(data[0], 0xF0);
        dev.read(0, I8042_DATA_REG, &mut data);
        assert_eq!(data[0], 0x1C);
    }

    #[test]
    fn test_mouse_injection() {
        let mut dev = create_test_device();
        let intr = Arc::new(TestInterrupt {
            event_fd: EventFd::new(0).unwrap(),
        });
        dev.set_mouse_interrupt(intr);

        // Move mouse
        dev.inject_mouse(MouseEvent {
            dx: 10,
            dy: -5,
            dz: 0,
            buttons: MouseButtons {
                left: true,
                ..Default::default()
            },
        });

        // Read back the packet
        let mut data = [0u8; 1];

        // Byte 1: buttons and signs
        dev.read(0, I8042_DATA_REG, &mut data);
        assert_eq!(data[0], 0x29); // 0x08 (always 1) | 0x01 (left btn) | 0x20 (Y sign for negative dy)

        // Byte 2: X movement
        dev.read(0, I8042_DATA_REG, &mut data);
        assert_eq!(data[0], 10);

        // Byte 3: Y movement (inverted)
        dev.read(0, I8042_DATA_REG, &mut data);
        assert_eq!(data[0] as i8, 5); // -(-5) = 5

        // Byte 4: scroll wheel
        dev.read(0, I8042_DATA_REG, &mut data);
        assert_eq!(data[0], 0);
    }

    #[test]
    fn test_status_register() {
        let mut dev = create_test_device();
        let intr = Arc::new(TestInterrupt {
            event_fd: EventFd::new(0).unwrap(),
        });
        dev.set_keyboard_interrupt(intr);

        // Initially, output buffer should be empty
        let mut data = [0u8; 1];
        dev.read(0, I8042_COMMAND_REG, &mut data);
        assert_eq!(data[0] & status::OUT_FULL, 0);

        // Inject keyboard event
        dev.inject_keyboard(KeyboardEvent {
            scancode: 0x1C,
            release: false,
        });

        // Now output buffer should be full
        dev.read(0, I8042_COMMAND_REG, &mut data);
        assert_ne!(data[0] & status::OUT_FULL, 0);
    }

    #[test]
    fn test_buffer_overflow() {
        let mut dev = create_test_device();
        let intr = Arc::new(TestInterrupt {
            event_fd: EventFd::new(0).unwrap(),
        });
        dev.set_keyboard_interrupt(intr);

        // Fill buffer beyond capacity
        for i in 0..MAX_BUFFER_SIZE + 5 {
            dev.inject_keyboard(KeyboardEvent {
                scancode: i as u8,
                release: false,
            });
        }

        // Buffer should be limited to MAX_BUFFER_SIZE
        assert!(dev.kbd_buffer.len() <= MAX_BUFFER_SIZE);
    }
}
