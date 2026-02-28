// Copyright 2024 Cloud Hypervisor Authors. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! USB Device Emulation
//!
//! This module provides USB device emulation for Cloud Hypervisor,
//! including xHCI controller and USB HID devices.

pub mod hid;
pub mod xhci;

pub use hid::{HidType, HidState, UsbHidDevice, SharedUsbHidDevice};
pub use hid::{
    USB_CLASS_HID,
    HID_SUBCLASS_BOOT,
    HID_PROTOCOL_KEYBOARD,
    HID_PROTOCOL_MOUSE,
    keyboard_report_descriptor,
    mouse_report_descriptor,
};

// Re-export commonly used xHCI types
pub use xhci::{XhciController, XhciState, XhciError, XHCI_VERSION, XHCI_MAX_SLOTS, XHCI_MAX_PORTS};
pub use xhci::rings::{Trb, TrbType, CompletionCode, CommandRing, EventRing, TransferRing};
pub use xhci::device::{UsbDevice, UsbSpeed, DeviceContext, SlotContext, SlotState, XhciDeviceSlot};
