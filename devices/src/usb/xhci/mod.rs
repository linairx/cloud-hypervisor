// Copyright 2024 Cloud Hypervisor Authors. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! xHCI (USB 3.0) Host Controller Interface
//!
//! This module implements an xHCI host controller for USB device emulation.
//! The xHCI specification provides a unified interface for USB 1.1, 2.0, and 3.0 devices.

pub mod regs;
pub mod rings;
pub mod device;

pub use regs::*;
pub use rings::*;
pub use device::*;

use std::sync::{Arc, Mutex};

use vm_memory::GuestMemoryMmap;

/// xHCI controller version
pub const XHCI_VERSION: u16 = 0x0100; // xHCI 1.0

/// Maximum number of device slots
pub const XHCI_MAX_SLOTS: u8 = 32;

/// Maximum number of interrupters
pub const XHCI_MAX_INTRS: u8 = 8;

/// Maximum number of ports
pub const XHCI_MAX_PORTS: u8 = 8;

/// xHCI operational states
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XhciState {
    /// Controller is halted
    Halted,
    /// Controller is running
    Running,
    /// Controller is in error state
    Error,
    /// Controller is being reset
    Reset,
}

/// xHCI Host Controller
pub struct XhciController {
    /// Controller state
    state: XhciState,
    /// Capability registers
    cap_regs: regs::CapabilityRegisters,
    /// Operational registers
    op_regs: regs::OperationalRegisters,
    /// Runtime registers
    rt_regs: regs::RuntimeRegisters,
    /// Doorbell registers
    doorbells: Vec<regs::DoorbellRegister>,
    /// Device slots
    slots: Vec<Option<Arc<Mutex<device::XhciDeviceSlot>>>>,
    /// Command ring
    cmd_ring: rings::CommandRing,
    /// Event rings
    event_rings: Vec<rings::EventRing>,
    /// Guest memory for DMA
    mem: Option<GuestMemoryMmap>,
    /// USB device address allocator (bit 0 unused, 1-127 valid)
    address_bitmap: u128,
}

impl XhciController {
    /// Create a new xHCI controller
    pub fn new() -> Self {
        let cap_regs = regs::CapabilityRegisters::new(XHCI_MAX_SLOTS, XHCI_MAX_INTRS, XHCI_MAX_PORTS);
        let op_regs = regs::OperationalRegisters::default();
        let rt_regs = regs::RuntimeRegisters::default();

        let doorbells = vec![regs::DoorbellRegister::default(); XHCI_MAX_SLOTS as usize + 1];
        let slots = vec![None; XHCI_MAX_SLOTS as usize + 1];
        let event_rings = (0..XHCI_MAX_INTRS)
            .map(|_| rings::EventRing::new())
            .collect();

        Self {
            state: XhciState::Halted,
            cap_regs,
            op_regs,
            rt_regs,
            doorbells,
            slots,
            cmd_ring: rings::CommandRing::new(),
            event_rings,
            mem: None,
            address_bitmap: 0,
        }
    }

    /// Set guest memory for DMA operations
    pub fn set_memory(&mut self, mem: GuestMemoryMmap) {
        self.mem = Some(mem);
    }

    /// Get capability registers
    pub fn capability_registers(&self) -> &regs::CapabilityRegisters {
        &self.cap_regs
    }

    /// Read operational register
    pub fn read_operational(&self, offset: u64) -> u32 {
        self.op_regs.read(offset)
    }

    /// Write operational register
    pub fn write_operational(&mut self, offset: u64, value: u32) {
        self.op_regs.write(offset, value, &mut self.state);
    }

    /// Read runtime register
    pub fn read_runtime(&self, offset: u64) -> u32 {
        self.rt_regs.read(offset)
    }

    /// Write runtime register
    pub fn write_runtime(&mut self, offset: u64, value: u32) {
        self.rt_regs.write(offset, value);
    }

    /// Ring doorbell
    pub fn ring_doorbell(&mut self, slot_id: u8, target: u8) {
        if slot_id as usize >= self.doorbells.len() {
            return;
        }

        // Store doorbell value
        self.doorbells[slot_id as usize].target = target;

        if slot_id == 0 {
            // Command ring doorbell
            self.process_command_ring();
        } else if let Some(ref slot) = self.slots[slot_id as usize] {
            // Transfer ring doorbell for a device slot
            if let Ok(mut slot) = slot.lock() {
                slot.ring_ep(target);
            }
        }
    }

    /// Process command ring
    fn process_command_ring(&mut self) {
        if self.state != XhciState::Running {
            return;
        }

        // Process command TRBs from the command ring
        while let Some(trb) = self.cmd_ring.next() {
            let (code, slot_id) = self.handle_command(&trb);

            // Create completion event
            let event = self.cmd_ring.create_completion_event(&trb, slot_id, code);

            // Queue event to primary interrupter
            if let Some(event_ring) = self.event_rings.first_mut() {
                event_ring.queue(event);
            }
        }
    }

    /// Handle a command TRB
    fn handle_command(&mut self, trb: &rings::Trb) -> (rings::CompletionCode, u8) {
        use rings::TrbType;

        match trb.trb_type() {
            Some(TrbType::EnableSlot) => {
                // Find free slot
                match self.find_free_slot() {
                    Ok(slot_id) => (rings::CompletionCode::Success, slot_id),
                    Err(_) => (rings::CompletionCode::NoSlotsAvailable, 0),
                }
            }
            Some(TrbType::DisableSlot) => {
                let slot_id = (trb.control >> 24) as u8;
                if (slot_id as usize) < self.slots.len() {
                    self.slots[slot_id as usize] = None;
                    (rings::CompletionCode::Success, slot_id)
                } else {
                    (rings::CompletionCode::SlotNotEnabled, 0)
                }
            }
            Some(TrbType::AddressDevice) => {
                let slot_id = (trb.control >> 24) as u8;
                if let Some(ref slot) = self.slots.get(slot_id as usize).and_then(|s| s.clone()) {
                    if let Ok(mut s) = slot.lock() {
                        let addr = self.allocate_address();
                        let (code, _) = s.handle_command(trb, addr);
                        (code, slot_id)
                    } else {
                        (rings::CompletionCode::SlotNotEnabled, slot_id)
                    }
                } else {
                    (rings::CompletionCode::SlotNotEnabled, 0)
                }
            }
            Some(TrbType::ConfigureEndpoint) => {
                let slot_id = (trb.control >> 24) as u8;
                if let Some(ref slot) = self.slots.get(slot_id as usize).and_then(|s| s.clone()) {
                    if let Ok(mut s) = slot.lock() {
                        let (code, _) = s.handle_command(trb, None);
                        (code, slot_id)
                    } else {
                        (rings::CompletionCode::SlotNotEnabled, slot_id)
                    }
                } else {
                    (rings::CompletionCode::SlotNotEnabled, 0)
                }
            }
            Some(TrbType::Noop) => {
                (rings::CompletionCode::Success, 0)
            }
            _ => {
                (rings::CompletionCode::TrbError, 0)
            }
        }
    }

    /// Get controller state
    pub fn state(&self) -> XhciState {
        self.state
    }

    /// Get number of ports
    pub fn num_ports(&self) -> u8 {
        XHCI_MAX_PORTS
    }

    /// Attach a device to a port
    pub fn attach_device(&mut self, port: u8, device: Arc<Mutex<dyn device::UsbDevice>>) -> Result<u8, XhciError> {
        if port >= XHCI_MAX_PORTS {
            return Err(XhciError::InvalidPort);
        }

        // Find free slot
        let slot_id = self.find_free_slot()?;
        if slot_id == 0 {
            return Err(XhciError::NoFreeSlots);
        }

        // Create device slot
        let slot = device::XhciDeviceSlot::new(slot_id, device);
        self.slots[slot_id as usize] = Some(Arc::new(Mutex::new(slot)));

        // Enable slot in port status
        self.op_regs.set_port_connected(port, true);

        Ok(slot_id)
    }

    /// Detach device from a port
    pub fn detach_device(&mut self, slot_id: u8) -> Result<(), XhciError> {
        if slot_id as usize >= self.slots.len() || slot_id == 0 {
            return Err(XhciError::InvalidSlot);
        }

        // Free the device address (get address first to avoid borrow issues)
        let addr = if let Some(ref slot) = self.slots[slot_id as usize] {
            if let Ok(s) = slot.lock() {
                Some(s.address())
            } else {
                None
            }
        } else {
            None
        };

        if let Some(a) = addr {
            self.free_address(a);
        }

        self.slots[slot_id as usize] = None;
        Ok(())
    }

    /// Find a free device slot
    fn find_free_slot(&self) -> Result<u8, XhciError> {
        for (i, slot) in self.slots.iter().enumerate() {
            if i > 0 && slot.is_none() {
                return Ok(i as u8);
            }
        }
        Err(XhciError::NoFreeSlots)
    }

    /// Process transfer ring for a slot endpoint
    pub fn process_transfer(&mut self, slot_id: u8, ep_id: u8) {
        if slot_id as usize >= self.slots.len() || ep_id > 31 {
            return;
        }

        if let Some(ref slot) = self.slots[slot_id as usize] {
            if let Ok(mut slot_guard) = slot.lock() {
                // Process transfers from the endpoint's transfer ring
                slot_guard.process_transfer(ep_id, &self.mem);
            }
        }
    }

    /// Allocate a USB device address (1-127)
    /// Returns None if all addresses are in use
    pub fn allocate_address(&mut self) -> Option<u8> {
        // Address 0 is reserved for default address
        for addr in 1..=127 {
            if self.address_bitmap & (1u128 << addr) == 0 {
                self.address_bitmap |= 1u128 << addr;
                return Some(addr);
            }
        }
        None
    }

    /// Free a USB device address
    pub fn free_address(&mut self, addr: u8) {
        if addr > 0 && addr <= 127 {
            self.address_bitmap &= !(1u128 << addr);
        }
    }

    /// Assign address to a slot
    /// Returns the assigned address, or None if allocation failed
    pub fn assign_address_to_slot(&mut self, slot_id: u8) -> Option<u8> {
        let addr = self.allocate_address()?;
        if let Some(slot) = self.slots.get(slot_id as usize)? {
            if let Ok(mut s) = slot.lock() {
                s.set_address(addr);
            }
        }
        Some(addr)
    }
}

impl Default for XhciController {
    fn default() -> Self {
        Self::new()
    }
}

/// xHCI errors
#[derive(Debug, Clone, thiserror::Error)]
pub enum XhciError {
    #[error("Invalid port number")]
    InvalidPort,
    #[error("Invalid slot ID")]
    InvalidSlot,
    #[error("No free device slots")]
    NoFreeSlots,
    #[error("Device not found")]
    DeviceNotFound,
    #[error("Transfer failed")]
    TransferFailed,
    #[error("Command failed")]
    CommandFailed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_controller_creation() {
        let ctrl = XhciController::new();
        assert_eq!(ctrl.state(), XhciState::Halted);
        assert_eq!(ctrl.num_ports(), XHCI_MAX_PORTS);
    }

    #[test]
    fn test_capability_registers() {
        let ctrl = XhciController::new();
        let caps = ctrl.capability_registers();

        assert!(caps.hciversion() == XHCI_VERSION);
    }

    #[test]
    fn test_address_allocation() {
        let mut ctrl = XhciController::new();

        // First allocation should return address 1
        let addr1 = ctrl.allocate_address();
        assert_eq!(addr1, Some(1));

        // Second allocation should return address 2
        let addr2 = ctrl.allocate_address();
        assert_eq!(addr2, Some(2));

        // Free address 1
        ctrl.free_address(1);

        // Next allocation should reuse address 1
        let addr3 = ctrl.allocate_address();
        assert_eq!(addr3, Some(1));

        // Next allocation should return address 3 (since 2 is still in use)
        let addr4 = ctrl.allocate_address();
        assert_eq!(addr4, Some(3));
    }

    #[test]
    fn test_address_allocation_all() {
        let mut ctrl = XhciController::new();

        // Allocate all 127 addresses
        for expected in 1..=127 {
            let addr = ctrl.allocate_address();
            assert_eq!(addr, Some(expected), "Failed at address {}", expected);
        }

        // Next allocation should fail
        let addr = ctrl.allocate_address();
        assert_eq!(addr, None);

        // Free address 50
        ctrl.free_address(50);

        // Now we should get address 50
        let addr = ctrl.allocate_address();
        assert_eq!(addr, Some(50));
    }

    #[test]
    fn test_free_address_invalid() {
        let mut ctrl = XhciController::new();

        // Free address 0 (should be ignored, no panic)
        ctrl.free_address(0);

        // Free address 128 (should be ignored, no panic)
        ctrl.free_address(128);

        // First allocation should still be 1
        let addr = ctrl.allocate_address();
        assert_eq!(addr, Some(1));
    }
}
