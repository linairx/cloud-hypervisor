// Copyright 2024 Cloud Hypervisor Authors. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! xHCI Device Slot and USB Device Emulation
//!
//! This module provides device slot management and USB device traits.

use std::io;
use std::sync::{Arc, Mutex};

use super::rings::{Trb, TransferRing, CompletionCode, TrbType};

// ============================================================================
// USB Device Trait
// ============================================================================

/// USB Device trait for emulated devices
pub trait UsbDevice: Send {
    /// Get device descriptor
    fn device_descriptor(&self) -> &[u8];

    /// Get configuration descriptor
    fn configuration_descriptor(&self) -> &[u8];

    /// Handle control transfer
    fn handle_control(&mut self, request: &[u8]) -> io::Result<Vec<u8>>;

    /// Handle data transfer (IN/OUT)
    fn handle_transfer(&mut self, ep: u8, data: &[u8]) -> io::Result<Vec<u8>>;

    /// Get device speed
    fn speed(&self) -> UsbSpeed;

    /// Reset device
    fn reset(&mut self);
}

/// USB device speeds
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbSpeed {
    /// Full Speed (12 Mbps)
    Full,
    /// High Speed (480 Mbps)
    High,
    /// Super Speed (5 Gbps)
    Super,
}

// ============================================================================
// Device Context
// ============================================================================

/// Input/Output Device Context
/// Size depends on context size setting (32 or 64 bytes per context)
#[derive(Debug, Clone)]
pub struct DeviceContext {
    /// Slot context
    slot: SlotContext,
    /// Endpoint contexts (EP0 + up to 30 endpoints)
    endpoints: Vec<EndpointContext>,
}

impl DeviceContext {
    /// Create a new device context
    pub fn new(num_endpoints: usize) -> Self {
        Self {
            slot: SlotContext::default(),
            endpoints: vec![EndpointContext::default(); num_endpoints + 1],
        }
    }

    /// Get slot context
    pub fn slot(&self) -> &SlotContext {
        &self.slot
    }

    /// Get mutable slot context
    pub fn slot_mut(&mut self) -> &mut SlotContext {
        &mut self.slot
    }

    /// Get endpoint context
    pub fn endpoint(&self, ep_id: usize) -> Option<&EndpointContext> {
        self.endpoints.get(ep_id)
    }

    /// Get mutable endpoint context
    pub fn endpoint_mut(&mut self, ep_id: usize) -> Option<&mut EndpointContext> {
        self.endpoints.get_mut(ep_id)
    }

    /// Calculate context size
    pub fn size(&self, context_64: bool) -> usize {
        let ctx_size = if context_64 { 64 } else { 32 };
        ctx_size * (1 + self.endpoints.len())
    }
}

// ============================================================================
// Slot Context
// ============================================================================

/// Slot Context (32 bytes)
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SlotContext {
    /// DWORD 0
    pub route_string: u32,
    /// DWORD 1
    pub usb_dev_addr: u8,
    pub slot_state: u8,
    pub interrupter_target: u16,
    /// DWORD 2
    pub usb_dev_speed: u8,
    pub num_ports: u8,
    pub hub_tt_time: u8,
    pub tt_think_time: u8,
    /// DWORD 3
    pub max_exit_latency: u16,
    pub target_exit_latency: u8,
    pub slot_id: u8,
    /// DWORD 4-7 reserved
    _reserved: [u32; 4],
}

/// Slot states
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotState {
    Disabled = 0,
    Default = 1,
    Addressed = 2,
    Configured = 3,
}

impl SlotContext {
    /// Create new slot context
    pub fn new() -> Self {
        Self::default()
    }

    /// Get slot state
    pub fn state(&self) -> SlotState {
        match self.slot_state & 0x0F {
            0 => SlotState::Disabled,
            1 => SlotState::Default,
            2 => SlotState::Addressed,
            3 => SlotState::Configured,
            _ => SlotState::Disabled,
        }
    }

    /// Set slot state
    pub fn set_state(&mut self, state: SlotState) {
        self.slot_state = (self.slot_state & 0xF0) | (state as u8);
    }

    /// Get USB device address
    pub fn device_address(&self) -> u8 {
        self.usb_dev_addr
    }

    /// Set USB device address
    pub fn set_device_address(&mut self, addr: u8) {
        self.usb_dev_addr = addr;
    }

    /// Get USB device speed
    pub fn speed(&self) -> UsbSpeed {
        match self.usb_dev_speed & 0x0F {
            1 => UsbSpeed::Full,
            2 => UsbSpeed::High,
            3 => UsbSpeed::Super,
            _ => UsbSpeed::Full,
        }
    }

    /// Set USB device speed
    pub fn set_speed(&mut self, speed: UsbSpeed) {
        self.usb_dev_speed = (self.usb_dev_speed & 0xF0)
            | match speed {
                UsbSpeed::Full => 1,
                UsbSpeed::High => 2,
                UsbSpeed::Super => 3,
            };
    }
}

// ============================================================================
// Endpoint Context
// ============================================================================

/// Endpoint Context (32 bytes)
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct EndpointContext {
    /// DWORD 0
    pub ep_state: u8,
    pub mult: u8,
    pub max_streams: u8,
    pub lsa: u8,
    /// DWORD 1
    pub interval: u8,
    pub max_esit_payload_hi: u8,
    pub hid: u8,
    pub max_burst_size: u8,
    /// DWORD 2
    pub max_packet_size: u16,
    pub max_esit_payload_lo: u8,
    pub ep_type: u8,
    /// DWORD 3
    pub average_trb_length: u16,
    pub max_esit_payload: u16,
    /// DWORD 4-5: Dequeue pointer
    pub tr_dequeue: u64,
    /// DWORD 6
    pub dcs: u8,
    pub max_primary_streams: u8,
    pub linear_stream_array: u8,
    pub pending_type: u8,
    /// DWORD 7
    pub mod_loop: u16,
    pub pending_type2: u8,
    pub ce: u8,
}

/// Endpoint states
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointState {
    Disabled = 0,
    Running = 1,
    Halted = 2,
    Stopped = 3,
    Error = 4,
}

/// Endpoint types
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointType {
    Invalid = 0,
    IsochOut = 1,
    BulkOut = 2,
    InterruptOut = 3,
    Control = 4,
    IsochIn = 5,
    BulkIn = 6,
    InterruptIn = 7,
}

impl EndpointContext {
    /// Create new endpoint context
    pub fn new() -> Self {
        Self::default()
    }

    /// Get endpoint state
    pub fn state(&self) -> EndpointState {
        match self.ep_state & 0x07 {
            0 => EndpointState::Disabled,
            1 => EndpointState::Running,
            2 => EndpointState::Halted,
            3 => EndpointState::Stopped,
            _ => EndpointState::Error,
        }
    }

    /// Set endpoint state
    pub fn set_state(&mut self, state: EndpointState) {
        self.ep_state = (self.ep_state & 0xF8) | (state as u8);
    }

    /// Get endpoint type
    pub fn ep_type(&self) -> EndpointType {
        match self.ep_type & 0x07 {
            0 => EndpointType::Invalid,
            1 => EndpointType::IsochOut,
            2 => EndpointType::BulkOut,
            3 => EndpointType::InterruptOut,
            4 => EndpointType::Control,
            5 => EndpointType::IsochIn,
            6 => EndpointType::BulkIn,
            7 => EndpointType::InterruptIn,
            _ => EndpointType::Invalid,
        }
    }

    /// Set endpoint type
    pub fn set_ep_type(&mut self, ep_type: EndpointType) {
        self.ep_type = (self.ep_type & 0xF8) | (ep_type as u8);
    }

    /// Get TR dequeue pointer
    pub fn tr_dequeue_ptr(&self) -> u64 {
        self.tr_dequeue
    }

    /// Set TR dequeue pointer
    pub fn set_tr_dequeue_ptr(&mut self, ptr: u64) {
        self.tr_dequeue = ptr;
    }

    /// Get dequeue cycle state
    pub fn dcs(&self) -> bool {
        self.dcs & 1 != 0
    }

    /// Set dequeue cycle state
    pub fn set_dcs(&mut self, dcs: bool) {
        self.dcs = if dcs { 1 } else { 0 };
    }
}

// ============================================================================
// Device Slot
// ============================================================================

/// xHCI Device Slot
pub struct XhciDeviceSlot {
    /// Slot ID
    slot_id: u8,
    /// Device context
    context: DeviceContext,
    /// USB device
    device: Option<Arc<Mutex<dyn UsbDevice>>>,
    /// Transfer rings for each endpoint
    transfer_rings: Vec<Option<TransferRing>>,
}

impl XhciDeviceSlot {
    /// Create a new device slot
    pub fn new(slot_id: u8, device: Arc<Mutex<dyn UsbDevice>>) -> Self {
        // Create context with 31 endpoints (EP0 + 30)
        let context = DeviceContext::new(31);

        Self {
            slot_id,
            context,
            device: Some(device),
            transfer_rings: (0..32).map(|_| None).collect(), // EP0-31
        }
    }

    /// Get slot ID
    pub fn slot_id(&self) -> u8 {
        self.slot_id
    }

    /// Get device context
    pub fn context(&self) -> &DeviceContext {
        &self.context
    }

    /// Get mutable device context
    pub fn context_mut(&mut self) -> &mut DeviceContext {
        &mut self.context
    }

    /// Ring endpoint (process transfers)
    pub fn ring_ep(&mut self, ep_id: u8) {
        if ep_id as usize >= self.transfer_rings.len() {
            return;
        }

        if let Some(ref mut ring) = self.transfer_rings[ep_id as usize] {
            // Process pending transfers
            while let Some(_trb) = ring.next() {
                // Handle transfer
            }
        }
    }

    /// Initialize endpoint ring
    pub fn init_ep_ring(&mut self, ep_id: u8, base: u64, size: usize) {
        if ep_id as usize >= self.transfer_rings.len() {
            return;
        }

        let mut ring = TransferRing::new(ep_id);
        ring.init(base, size);
        self.transfer_rings[ep_id as usize] = Some(ring);
    }

    /// Set slot state
    pub fn set_state(&mut self, state: SlotState) {
        self.context.slot_mut().set_state(state);
    }

    /// Get slot state
    pub fn state(&self) -> SlotState {
        self.context.slot().state()
    }

    /// Set device address
    pub fn set_address(&mut self, addr: u8) {
        self.context.slot_mut().set_device_address(addr);
    }

    /// Get device address
    pub fn address(&self) -> u8 {
        self.context.slot().device_address()
    }

    /// Handle command TRB
    pub fn handle_command(&mut self, trb: &Trb) -> (CompletionCode, u32) {
        match trb.trb_type() {
            Some(TrbType::AddressDevice) => {
                // Address device command
                self.set_state(SlotState::Addressed);
                self.set_address(1); // Simplified: assign address 1
                (CompletionCode::Success, 0)
            }
            Some(TrbType::ConfigureEndpoint) => {
                // Configure endpoint command
                (CompletionCode::Success, 0)
            }
            Some(TrbType::EvaluateContext) => {
                // Evaluate context command
                (CompletionCode::Success, 0)
            }
            Some(TrbType::ResetEndpoint) => {
                // Reset endpoint command
                (CompletionCode::Success, 0)
            }
            Some(TrbType::StopEndpoint) => {
                // Stop endpoint command
                (CompletionCode::Success, 0)
            }
            Some(TrbType::SetTrDequeue) => {
                // Set TR dequeue pointer command
                let ep_id = (trb.control >> 16) & 0x1F;
                if let Some(ref mut ring) = self.transfer_rings[ep_id as usize] {
                    ring.set_dequeue_ptr(trb.parameter & !0xF, trb.parameter & 1 != 0);
                }
                (CompletionCode::Success, 0)
            }
            Some(TrbType::ResetDevice) => {
                // Reset device command
                if let Some(ref device) = self.device {
                    if let Ok(mut dev) = device.lock() {
                        dev.reset();
                    }
                }
                self.set_state(SlotState::Default);
                (CompletionCode::Success, 0)
            }
            _ => (CompletionCode::TrbError, 0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_context() {
        let ctx = DeviceContext::new(31);
        assert_eq!(ctx.endpoints.len(), 32);
    }

    #[test]
    fn test_slot_context() {
        let mut slot = SlotContext::new();
        slot.set_state(SlotState::Addressed);
        slot.set_device_address(5);
        slot.set_speed(UsbSpeed::High);

        assert_eq!(slot.state(), SlotState::Addressed);
        assert_eq!(slot.device_address(), 5);
        assert_eq!(slot.speed(), UsbSpeed::High);
    }

    #[test]
    fn test_endpoint_context() {
        let mut ep = EndpointContext::new();
        ep.set_state(EndpointState::Running);
        ep.set_ep_type(EndpointType::BulkIn);
        ep.set_tr_dequeue_ptr(0x10000);
        ep.set_dcs(true);

        assert_eq!(ep.state(), EndpointState::Running);
        assert_eq!(ep.ep_type(), EndpointType::BulkIn);
        assert_eq!(ep.tr_dequeue_ptr(), 0x10000);
        assert!(ep.dcs());
    }
}
