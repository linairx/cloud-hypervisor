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
        // This is a simplified implementation
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
}
