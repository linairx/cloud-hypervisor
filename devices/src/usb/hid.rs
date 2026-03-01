// Copyright 2024 Cloud Hypervisor Authors. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! USB HID Device Emulation
//!
//! This module provides USB Human Interface Device (HID) emulation for
//! keyboard and mouse input. It implements the USB HID specification
//! for compatibility with standard guest drivers.

use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Mutex};

use vm_memory::ByteValued;

// ============================================================================
// USB HID Constants
// ============================================================================

/// USB HID device class
pub const USB_CLASS_HID: u8 = 0x03;

/// HID subclass for boot interface
pub const HID_SUBCLASS_BOOT: u8 = 0x01;

/// HID protocol for keyboard
pub const HID_PROTOCOL_KEYBOARD: u8 = 0x01;

/// HID protocol for mouse
pub const HID_PROTOCOL_MOUSE: u8 = 0x02;

/// USB endpoint direction: IN (device to host)
pub const USB_DIR_IN: u8 = 0x80;

/// USB endpoint type: interrupt
pub const USB_ENDPOINT_XFER_INT: u8 = 0x03;

// ============================================================================
// USB Descriptors
// ============================================================================

/// USB device descriptor
#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct UsbDeviceDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,
    pub bcd_usb: u16,
    pub b_device_class: u8,
    pub b_device_sub_class: u8,
    pub b_device_protocol: u8,
    pub b_max_packet_size0: u8,
    pub id_vendor: u16,
    pub id_product: u16,
    pub bcd_device: u16,
    pub i_manufacturer: u8,
    pub i_product: u8,
    pub i_serial_number: u8,
    pub b_num_configurations: u8,
}

unsafe impl ByteValued for UsbDeviceDescriptor {}

impl UsbDeviceDescriptor {
    /// Create a HID keyboard device descriptor
    pub fn keyboard() -> Self {
        Self {
            b_length: std::mem::size_of::<Self>() as u8,
            b_descriptor_type: 0x01, // DEVICE
            bcd_usb: 0x0200,         // USB 2.0
            b_device_class: 0x00,    // Defined at interface level
            b_device_sub_class: 0x00,
            b_device_protocol: 0x00,
            b_max_packet_size0: 8,
            id_vendor: 0x1D6B,       // Linux Foundation
            id_product: 0x0104,      // Virtual HID Keyboard
            bcd_device: 0x0100,
            i_manufacturer: 1,
            i_product: 2,
            i_serial_number: 0,
            b_num_configurations: 1,
        }
    }

    /// Create a HID mouse device descriptor
    pub fn mouse() -> Self {
        Self {
            b_length: std::mem::size_of::<Self>() as u8,
            b_descriptor_type: 0x01,
            bcd_usb: 0x0200,
            b_device_class: 0x00,
            b_device_sub_class: 0x00,
            b_device_protocol: 0x00,
            b_max_packet_size0: 8,
            id_vendor: 0x1D6B,
            id_product: 0x0105,      // Virtual HID Mouse
            bcd_device: 0x0100,
            i_manufacturer: 1,
            i_product: 3,
            i_serial_number: 0,
            b_num_configurations: 1,
        }
    }
}

/// USB configuration descriptor
#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct UsbConfigDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,
    pub w_total_length: u16,
    pub b_num_interfaces: u8,
    pub b_configuration_value: u8,
    pub i_configuration: u8,
    pub bm_attributes: u8,
    pub b_max_power: u8,
}

unsafe impl ByteValued for UsbConfigDescriptor {}

/// USB interface descriptor
#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct UsbInterfaceDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,
    pub b_interface_number: u8,
    pub b_alternate_setting: u8,
    pub b_num_endpoints: u8,
    pub b_interface_class: u8,
    pub b_interface_sub_class: u8,
    pub b_interface_protocol: u8,
    pub i_interface: u8,
}

unsafe impl ByteValued for UsbInterfaceDescriptor {}

/// USB endpoint descriptor
#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct UsbEndpointDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,
    pub b_endpoint_address: u8,
    pub bm_attributes: u8,
    pub w_max_packet_size: u16,
    pub b_interval: u8,
}

unsafe impl ByteValued for UsbEndpointDescriptor {}

/// HID class descriptor
#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct HidClassDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,
    pub b_descriptor_type_sub: u8,
    pub w_descriptor_length: u16,
}

unsafe impl ByteValued for HidClassDescriptor {}

// ============================================================================
// HID Report Descriptors
// ============================================================================

/// Standard USB HID keyboard report descriptor
pub fn keyboard_report_descriptor() -> Vec<u8> {
    vec![
        0x05, 0x01,        // Usage Page (Generic Desktop)
        0x09, 0x06,        // Usage (Keyboard)
        0xA1, 0x01,        // Collection (Application)
        0x05, 0x07,        //   Usage Page (Key Codes)
        0x19, 0xE0,        //   Usage Minimum (224)
        0x29, 0xE7,        //   Usage Maximum (231)
        0x15, 0x00,        //   Logical Minimum (0)
        0x25, 0x01,        //   Logical Maximum (1)
        0x75, 0x01,        //   Report Size (1)
        0x95, 0x08,        //   Report Count (8)
        0x81, 0x02,        //   Input (Data, Var, Abs)
        0x95, 0x01,        //   Report Count (1)
        0x75, 0x08,        //   Report Size (8)
        0x81, 0x01,        //   Input (Const) - Reserved
        0x95, 0x05,        //   Report Count (5)
        0x75, 0x01,        //   Report Size (1)
        0x05, 0x08,        //   Usage Page (LEDs)
        0x19, 0x01,        //   Usage Minimum (1)
        0x29, 0x05,        //   Usage Maximum (5)
        0x91, 0x02,        //   Output (Data, Var, Abs)
        0x95, 0x01,        //   Report Count (1)
        0x75, 0x03,        //   Report Size (3)
        0x91, 0x01,        //   Output (Const) - Reserved
        0x95, 0x06,        //   Report Count (6)
        0x75, 0x08,        //   Report Size (8)
        0x15, 0x00,        //   Logical Minimum (0)
        0x25, 0x65,        //   Logical Maximum (101)
        0x05, 0x07,        //   Usage Page (Key Codes)
        0x19, 0x00,        //   Usage Minimum (0)
        0x29, 0x65,        //   Usage Maximum (101)
        0x81, 0x00,        //   Input (Data, Array)
        0xC0,              // End Collection
    ]
}

/// Standard USB HID mouse report descriptor
pub fn mouse_report_descriptor() -> Vec<u8> {
    vec![
        0x05, 0x01,        // Usage Page (Generic Desktop)
        0x09, 0x02,        // Usage (Mouse)
        0xA1, 0x01,        // Collection (Application)
        0x09, 0x01,        //   Usage (Pointer)
        0xA1, 0x00,        //   Collection (Physical)
        0x05, 0x09,        //     Usage Page (Button)
        0x19, 0x01,        //     Usage Minimum (1)
        0x29, 0x03,        //     Usage Maximum (3)
        0x15, 0x00,        //     Logical Minimum (0)
        0x25, 0x01,        //     Logical Maximum (1)
        0x95, 0x03,        //     Report Count (3)
        0x75, 0x01,        //     Report Size (1)
        0x81, 0x02,        //     Input (Data, Var, Abs)
        0x95, 0x01,        //     Report Count (1)
        0x75, 0x05,        //     Report Size (5)
        0x81, 0x01,        //     Input (Const) - Reserved
        0x05, 0x01,        //     Usage Page (Generic Desktop)
        0x09, 0x30,        //     Usage (X)
        0x09, 0x31,        //     Usage (Y)
        0x09, 0x38,        //     Usage (Wheel)
        0x15, 0x81,        //     Logical Minimum (-127)
        0x25, 0x7F,        //     Logical Maximum (127)
        0x75, 0x08,        //     Report Size (8)
        0x95, 0x03,        //     Report Count (3)
        0x81, 0x06,        //     Input (Data, Var, Rel)
        0xC0,              //   End Collection
        0xC0,              // End Collection
    ]
}

// ============================================================================
// HID Device Types
// ============================================================================

/// HID device type
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HidType {
    Keyboard,
    Mouse,
}

/// HID device state
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HidState {
    Default,
    Addressed,
    Configured,
    Suspended,
}

// ============================================================================
// USB HID Device
// ============================================================================

/// USB HID device emulator
pub struct UsbHidDevice {
    /// Device type (keyboard or mouse)
    hid_type: HidType,
    /// Current device state
    state: HidState,
    /// USB device address
    address: u8,
    /// Current configuration
    configuration: u8,
    /// Device descriptor
    device_descriptor: UsbDeviceDescriptor,
    /// HID report queue (for interrupt IN endpoint)
    report_queue: VecDeque<Vec<u8>>,
    /// Maximum queue depth
    max_queue_depth: usize,
}

impl UsbHidDevice {
    /// Create a new USB HID keyboard device
    pub fn new_keyboard() -> Self {
        Self {
            hid_type: HidType::Keyboard,
            state: HidState::Default,
            address: 0,
            configuration: 0,
            device_descriptor: UsbDeviceDescriptor::keyboard(),
            report_queue: VecDeque::new(),
            max_queue_depth: 16,
        }
    }

    /// Create a new USB HID mouse device
    pub fn new_mouse() -> Self {
        Self {
            hid_type: HidType::Mouse,
            state: HidState::Default,
            address: 0,
            configuration: 0,
            device_descriptor: UsbDeviceDescriptor::mouse(),
            report_queue: VecDeque::new(),
            max_queue_depth: 16,
        }
    }

    /// Get HID type
    pub fn hid_type(&self) -> HidType {
        self.hid_type
    }

    /// Get current state
    pub fn state(&self) -> HidState {
        self.state
    }

    /// Get device address
    pub fn address(&self) -> u8 {
        self.address
    }

    /// Get device descriptor
    pub fn device_descriptor(&self) -> &UsbDeviceDescriptor {
        &self.device_descriptor
    }

    /// Get report descriptor
    pub fn report_descriptor(&self) -> Vec<u8> {
        match self.hid_type {
            HidType::Keyboard => keyboard_report_descriptor(),
            HidType::Mouse => mouse_report_descriptor(),
        }
    }

    /// Handle USB control request
    pub fn handle_control(&mut self, request: &[u8]) -> io::Result<Vec<u8>> {
        if request.len() < 8 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Request too short",
            ));
        }

        let request_type = request[0];
        let request_code = request[1];
        let value = u16::from(request[2]) | (u16::from(request[3]) << 8);
        let _index = u16::from(request[4]) | (u16::from(request[5]) << 8);
        let _length = u16::from(request[6]) | (u16::from(request[7]) << 8);

        // Standard device requests
        match request_code {
            0x05 => {
                // SET_ADDRESS
                self.address = value as u8;
                self.state = HidState::Addressed;
                Ok(vec![])
            }
            0x09 => {
                // SET_CONFIGURATION
                self.configuration = value as u8;
                if self.configuration != 0 {
                    self.state = HidState::Configured;
                }
                Ok(vec![])
            }
            0x06 => {
                // GET_DESCRIPTOR
                let descriptor_type = (value >> 8) as u8;
                let _descriptor_index = (value & 0xFF) as u8;

                match descriptor_type {
                    0x01 => {
                        // Device descriptor
                        let desc = self.device_descriptor;
                        Ok(desc.as_slice().to_vec())
                    }
                    0x02 => {
                        // Configuration descriptor
                        Ok(self.get_configuration_descriptor())
                    }
                    0x22 => {
                        // HID report descriptor
                        Ok(self.report_descriptor())
                    }
                    0x03 => {
                        // String descriptor
                        Ok(self.get_string_descriptor(value as u8))
                    }
                    _ => Ok(vec![]),
                }
            }
            0x0A => {
                // GET_INTERFACE
                Ok(vec![0])
            }
            0x21 => {
                // HID SET_REPORT
                Ok(vec![])
            }
            0x01 => {
                // HID SET_IDLE
                Ok(vec![])
            }
            0x0B => {
                // SET_INTERFACE
                Ok(vec![])
            }
            _ => Ok(vec![]),
        }
    }

    /// Get configuration descriptor (including interface and endpoint)
    fn get_configuration_descriptor(&self) -> Vec<u8> {
        let mut buf = Vec::new();

        // Configuration descriptor
        let config = UsbConfigDescriptor {
            b_length: std::mem::size_of::<UsbConfigDescriptor>() as u8,
            b_descriptor_type: 0x02,
            w_total_length: 0, // Will be filled later
            b_num_interfaces: 1,
            b_configuration_value: 1,
            i_configuration: 0,
            bm_attributes: 0x80, // Bus powered
            b_max_power: 50,     // 100mA
        };

        // Interface descriptor
        let interface = UsbInterfaceDescriptor {
            b_length: std::mem::size_of::<UsbInterfaceDescriptor>() as u8,
            b_descriptor_type: 0x04,
            b_interface_number: 0,
            b_alternate_setting: 0,
            b_num_endpoints: 1,
            b_interface_class: USB_CLASS_HID,
            b_interface_sub_class: HID_SUBCLASS_BOOT,
            b_interface_protocol: match self.hid_type {
                HidType::Keyboard => HID_PROTOCOL_KEYBOARD,
                HidType::Mouse => HID_PROTOCOL_MOUSE,
            },
            i_interface: 0,
        };

        // HID class descriptor
        let hid_desc = HidClassDescriptor {
            b_length: std::mem::size_of::<HidClassDescriptor>() as u8,
            b_descriptor_type: 0x21,
            b_descriptor_type_sub: 0x22, // Report descriptor
            w_descriptor_length: self.report_descriptor().len() as u16,
        };

        // Endpoint descriptor (interrupt IN)
        let endpoint = UsbEndpointDescriptor {
            b_length: std::mem::size_of::<UsbEndpointDescriptor>() as u8,
            b_descriptor_type: 0x05,
            b_endpoint_address: 0x81, // EP1 IN
            bm_attributes: USB_ENDPOINT_XFER_INT,
            w_max_packet_size: match self.hid_type {
                HidType::Keyboard => 8,
                HidType::Mouse => 4,
            },
            b_interval: 10, // 10ms polling interval
        };

        // Calculate total length
        let total_len = std::mem::size_of::<UsbConfigDescriptor>()
            + std::mem::size_of::<UsbInterfaceDescriptor>()
            + std::mem::size_of::<HidClassDescriptor>()
            + std::mem::size_of::<UsbEndpointDescriptor>();

        // Append all descriptors
        buf.extend_from_slice(config.as_slice());
        let total_len_offset = buf.len() - std::mem::size_of::<UsbConfigDescriptor>() + 2;
        buf[total_len_offset] = total_len as u8;
        buf[total_len_offset + 1] = (total_len >> 8) as u8;

        buf.extend_from_slice(interface.as_slice());
        buf.extend_from_slice(hid_desc.as_slice());
        buf.extend_from_slice(endpoint.as_slice());

        buf
    }

    /// Get string descriptor
    fn get_string_descriptor(&self, index: u8) -> Vec<u8> {
        let strings = ["Cloud Hypervisor", "HID Device", "HID Mouse"];

        if index == 0 {
            // Language ID descriptor
            return vec![4, 0x03, 0x09, 0x04]; // US English
        }

        let idx = (index - 1) as usize;
        if idx >= strings.len() {
            return vec![];
        }

        let s = strings[idx];
        let mut buf = vec![0u8; 2 + s.len() * 2];
        buf[0] = buf.len() as u8;
        buf[1] = 0x03; // String descriptor type

        for (i, c) in s.encode_utf16().enumerate() {
            buf[2 + i * 2] = c as u8;
            buf[2 + i * 2 + 1] = (c >> 8) as u8;
        }

        buf
    }

    /// Queue a HID report for transmission
    pub fn queue_report(&mut self, report: Vec<u8>) -> bool {
        if self.report_queue.len() >= self.max_queue_depth {
            self.report_queue.pop_front();
        }
        self.report_queue.push_back(report);
        true
    }

    /// Get next queued report
    pub fn get_report(&mut self) -> Option<Vec<u8>> {
        self.report_queue.pop_front()
    }

    /// Check if reports are pending
    pub fn has_pending_reports(&self) -> bool {
        !self.report_queue.is_empty()
    }
}

// ============================================================================
// Thread-safe wrapper
// ============================================================================

/// Thread-safe USB HID device wrapper
pub type SharedUsbHidDevice = Arc<Mutex<UsbHidDevice>>;

// ============================================================================
// UsbDevice Trait Implementation
// ============================================================================

use super::xhci::device::{UsbDevice, UsbSpeed};
use std::io::Cursor;
use std::io::Write;

impl UsbDevice for UsbHidDevice {
    fn device_descriptor(&self) -> &[u8] {
        // SAFETY: We cast to bytes through a fixed-size struct
        unsafe {
            std::slice::from_raw_parts(
                &self.device_descriptor as *const UsbDeviceDescriptor as *const u8,
                std::mem::size_of::<UsbDeviceDescriptor>(),
            )
        }
    }

    fn configuration_descriptor(&self) -> &[u8] {
        // Return a static slice based on device type
        // This is safe because descriptors are constant
        static KEYBOARD_CONFIG: &[u8] = &[
            // Configuration descriptor (9 bytes)
            0x09, 0x02, 0x22, 0x00, 0x01, 0x01, 0x00, 0x80, 0x32,
            // Interface descriptor (9 bytes)
            0x09, 0x04, 0x00, 0x00, 0x01, 0x03, 0x01, 0x01, 0x00,
            // HID descriptor (9 bytes)
            0x09, 0x21, 0x11, 0x01, 0x00, 0x01, 0x22, 0x3F, 0x00,
            // Endpoint descriptor (7 bytes)
            0x07, 0x05, 0x81, 0x03, 0x08, 0x00, 0x0A,
        ];

        static MOUSE_CONFIG: &[u8] = &[
            // Configuration descriptor (9 bytes)
            0x09, 0x02, 0x22, 0x00, 0x01, 0x01, 0x00, 0x80, 0x32,
            // Interface descriptor (9 bytes)
            0x09, 0x04, 0x00, 0x00, 0x01, 0x03, 0x01, 0x02, 0x00,
            // HID descriptor (9 bytes)
            0x09, 0x21, 0x11, 0x01, 0x00, 0x01, 0x22, 0x32, 0x00,
            // Endpoint descriptor (7 bytes)
            0x07, 0x05, 0x81, 0x03, 0x04, 0x00, 0x0A,
        ];

        match self.hid_type {
            HidType::Keyboard => KEYBOARD_CONFIG,
            HidType::Mouse => MOUSE_CONFIG,
        }
    }

    fn handle_control(&mut self, request: &[u8]) -> io::Result<Vec<u8>> {
        self.handle_control(request)
    }

    fn handle_transfer(&mut self, ep: u8, _data: &[u8]) -> io::Result<Vec<u8>> {
        // Handle endpoint transfers
        match ep {
            0x81 => {
                // EP1 IN - Return queued HID report
                self.get_report().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::WouldBlock, "No report available")
                })
            }
            0x01 => {
                // EP1 OUT - Receive HID report (for SET_REPORT)
                // Usually not needed for simple input devices
                Ok(vec![])
            }
            _ => {
                // Unknown endpoint
                Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("Unknown endpoint: {}", ep),
                ))
            }
        }
    }

    fn speed(&self) -> UsbSpeed {
        // USB HID typically uses Full Speed (12 Mbps)
        UsbSpeed::Full
    }

    fn reset(&mut self) {
        self.state = HidState::Default;
        self.address = 0;
        self.configuration = 0;
        self.report_queue.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keyboard_device_creation() {
        let device = UsbHidDevice::new_keyboard();
        assert_eq!(device.hid_type(), HidType::Keyboard);
        assert_eq!(device.state(), HidState::Default);
        assert_eq!(device.address(), 0);
    }

    #[test]
    fn test_mouse_device_creation() {
        let device = UsbHidDevice::new_mouse();
        assert_eq!(device.hid_type(), HidType::Mouse);
        assert_eq!(device.state(), HidState::Default);
    }

    #[test]
    fn test_report_descriptors() {
        let keyboard = UsbHidDevice::new_keyboard();
        let mouse = UsbHidDevice::new_mouse();

        assert!(!keyboard.report_descriptor().is_empty());
        assert!(!mouse.report_descriptor().is_empty());
    }

    #[test]
    fn test_queue_report() {
        let mut device = UsbHidDevice::new_keyboard();
        let report = vec![0, 0, 0x04, 0, 0, 0, 0, 0]; // 'A' key

        assert!(device.queue_report(report.clone()));
        assert!(device.has_pending_reports());
        assert_eq!(device.get_report(), Some(report));
        assert!(!device.has_pending_reports());
    }
}
