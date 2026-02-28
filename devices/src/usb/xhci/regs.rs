// Copyright 2024 Cloud Hypervisor Authors. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! xHCI Register Definitions
//!
//! This module defines the xHCI host controller registers according to
//! the xHCI Specification 1.0. Registers are organized into four groups:
//! - Capability Registers (read-only)
//! - Operational Registers (read/write)
//! - Runtime Registers
//! - Doorbell Registers

use super::XhciState;
use std::sync::atomic::{AtomicU32, Ordering};

// ============================================================================
// Capability Registers (Offset 0x00 - 0x1F)
// ============================================================================

/// xHCI Capability Registers
#[repr(C)]
#[derive(Debug, Clone)]
pub struct CapabilityRegisters {
    /// HCIVERSION: Interface Version Number (0x00)
    hciversion: u16,
    /// HCSPARAMS1: Structural Parameters 1 (0x04)
    hcsparams1: HcsParams1,
    /// HCSPARAMS2: Structural Parameters 2 (0x08)
    hcsparams2: HcsParams2,
    /// HCSPARAMS3: Structural Parameters 3 (0x0C)
    hcsparams3: HcsParams3,
    /// HCCPARAMS1: Capability Parameters 1 (0x10)
    hccparams1: HccParams1,
    /// DBOFF: Doorbell Offset (0x14)
    dboff: u32,
    /// RTSOFF: Runtime Registers Offset (0x18)
    rtsoff: u32,
    /// HCCPARAMS2: Capability Parameters 2 (0x1C) - xHCI 1.1+
    hccparams2: u32,
}

impl CapabilityRegisters {
    /// Create new capability registers
    pub fn new(max_slots: u8, max_intrs: u8, max_ports: u8) -> Self {
        Self {
            hciversion: super::XHCI_VERSION,
            hcsparams1: HcsParams1::new(max_slots, max_intrs, max_ports),
            hcsparams2: HcsParams2::default(),
            hcsparams3: HcsParams3::default(),
            hccparams1: HccParams1::default(),
            dboff: 0x1000, // Doorbell registers start at 4KB offset
            rtsoff: 0x2000, // Runtime registers start at 8KB offset
            hccparams2: 0,
        }
    }

    /// Get interface version
    pub fn hciversion(&self) -> u16 {
        self.hciversion
    }

    /// Get structural parameters 1
    pub fn hcsparams1(&self) -> u32 {
        self.hcsparams1.into()
    }

    /// Get structural parameters 2
    pub fn hcsparams2(&self) -> u32 {
        self.hcsparams2.into()
    }

    /// Get structural parameters 3
    pub fn hcsparams3(&self) -> u32 {
        self.hcsparams3.into()
    }

    /// Get capability parameters 1
    pub fn hccparams1(&self) -> u32 {
        self.hccparams1.into()
    }

    /// Get doorbell offset
    pub fn dboff(&self) -> u32 {
        self.dboff
    }

    /// Get runtime registers offset
    pub fn rtsoff(&self) -> u32 {
        self.rtsoff
    }

    /// Read register at offset
    pub fn read(&self, offset: u64) -> u32 {
        match offset {
            0x00 => self.hciversion as u32,
            0x02 => (self.hciversion >> 8) as u32 | ((self.hciversion as u32) << 16),
            0x04 => self.hcsparams1(),
            0x08 => self.hcsparams2(),
            0x0C => self.hcsparams3(),
            0x10 => self.hccparams1(),
            0x14 => self.dboff,
            0x18 => self.rtsoff,
            0x1C => self.hccparams2,
            _ => 0,
        }
    }
}

/// HCSPARAMS1: Structural Parameters 1
#[repr(transparent)]
#[derive(Debug, Clone, Copy, Default)]
struct HcsParams1(u32);

impl HcsParams1 {
    fn new(max_slots: u8, max_intrs: u8, max_ports: u8) -> Self {
        Self(
            (max_slots as u32)            // Bits 0-7: MaxSlots
            | ((max_intrs as u32) << 8)   // Bits 8-18: MaxIntrs
            | ((max_ports as u32) << 24), // Bits 24-31: MaxPorts
        )
    }

    fn max_slots(&self) -> u8 {
        (self.0 & 0xFF) as u8
    }

    fn max_intrs(&self) -> u16 {
        ((self.0 >> 8) & 0x7FF) as u16
    }

    fn max_ports(&self) -> u8 {
        ((self.0 >> 24) & 0xFF) as u8
    }
}

impl From<HcsParams1> for u32 {
    fn from(val: HcsParams1) -> Self {
        val.0
    }
}

/// HCSPARAMS2: Structural Parameters 2
#[repr(transparent)]
#[derive(Debug, Clone, Copy, Default)]
struct HcsParams2(u32);

impl HcsParams2 {
    /// IST: Isochronous Scheduling Threshold (bits 0-3)
    fn ist(&self) -> u8 {
        (self.0 & 0xF) as u8
    }

    /// ERST Max: Event Ring Segment Table Max (bits 4-8)
    fn erst_max(&self) -> u8 {
        ((self.0 >> 4) & 0x1F) as u8
    }

    /// SPB: Scratchpad Buffer High/Support (bit 13)
    fn spb(&self) -> bool {
        (self.0 >> 13) & 1 != 0
    }

    /// Max Scratchpad Buffers (bits 21-25)
    fn max_scratchpad_bufs(&self) -> u8 {
        ((self.0 >> 21) & 0x1F) as u8
    }
}

impl From<HcsParams2> for u32 {
    fn from(val: HcsParams2) -> Self {
        val.0
    }
}

/// HCSPARAMS3: Structural Parameters 3
#[repr(transparent)]
#[derive(Debug, Clone, Copy, Default)]
struct HcsParams3(u32);

impl From<HcsParams3> for u32 {
    fn from(val: HcsParams3) -> Self {
        val.0
    }
}

/// HCCPARAMS1: Capability Parameters 1
#[repr(transparent)]
#[derive(Debug, Clone, Copy, Default)]
struct HccParams1(u32);

impl HccParams1 {
    /// AC64: 64-bit Addressing Capability (bit 0)
    fn ac64(&self) -> bool {
        self.0 & 1 != 0
    }

    /// BNC: BW Negotiation Capability (bit 1)
    fn bnc(&self) -> bool {
        (self.0 >> 1) & 1 != 0
    }

    /// CSZ: Context Size (bit 2)
    fn csz(&self) -> bool {
        (self.0 >> 2) & 1 != 0
    }

    /// PPC: Port Power Control (bit 3)
    fn ppc(&self) -> bool {
        (self.0 >> 3) & 1 != 0
    }

    /// PIND: Port Indicators (bit 4)
    fn pind(&self) -> bool {
        (self.0 >> 4) & 1 != 0
    }

    /// LHRC: Light HC Reset Capability (bit 5)
    fn lhrc(&self) -> bool {
        (self.0 >> 5) & 1 != 0
    }

    /// SEC: Stopped - EDTLA Capability (bit 6)
    fn sec(&self) -> bool {
        (self.0 >> 6) & 1 != 0
    }

    /// CFC: Contiguous Frame ID Capability (bit 7)
    fn cfc(&self) -> bool {
        (self.0 >> 7) & 1 != 0
    }
}

impl From<HccParams1> for u32 {
    fn from(val: HccParams1) -> Self {
        val.0
    }
}

// ============================================================================
// Operational Registers (Offset 0x00 - 0x3FF from operational base)
// ============================================================================

/// xHCI Operational Registers
#[derive(Debug)]
pub struct OperationalRegisters {
    /// USBCMD: USB Command Register (0x00)
    usbcmd: AtomicU32,
    /// USBSTS: USB Status Register (0x04)
    usbsts: AtomicU32,
    /// PGSZ: Page Size Register (0x08)
    pgsz: AtomicU32,
    /// DNCTRL: Device Notification Control (0x0C)
    dnctrl: AtomicU32,
    /// CRCR: Command Ring Control Register (0x10)
    crcr: AtomicU32,
    /// DCBAAP: Device Context Base Address Array Pointer (0x30)
    dcbaap: AtomicU32,
    /// CONFIG: Configure Register (0x38)
    config: AtomicU32,
    /// Port Status and Control Registers (0x400+)
    ports: Vec<PortRegister>,
}

impl Default for OperationalRegisters {
    fn default() -> Self {
        Self {
            usbcmd: AtomicU32::new(0),
            usbsts: AtomicU32::new(USBSTS_HCH), // Controller halted
            pgsz: AtomicU32::new(0x1000),       // 4KB page size
            dnctrl: AtomicU32::new(0),
            crcr: AtomicU32::new(0),
            dcbaap: AtomicU32::new(0),
            config: AtomicU32::new(0),
            ports: (0..super::XHCI_MAX_PORTS).map(|_| PortRegister::default()).collect(),
        }
    }
}

/// USBCMD register bits
pub const USBCMD_RS: u32 = 1 << 0;       // Run/Stop
pub const USBCMD_HCRST: u32 = 1 << 1;    // Host Controller Reset
pub const USBCMD_INTE: u32 = 1 << 2;     // Interrupter Enable
pub const USBCMD_HSEE: u32 = 1 << 3;     // Host System Error Enable
pub const USBCMD_LHCRST: u32 = 1 << 7;   // Light Host Controller Reset
pub const USBCMD_CSS: u32 = 1 << 8;      // Controller Save State
pub const USBCMD_CRS: u32 = 1 << 9;      // Controller Restore State
pub const USBCMD_EWE: u32 = 1 << 10;     // Enable Wrap Event

/// USBSTS register bits
pub const USBSTS_HCH: u32 = 1 << 0;      // HCHalted
pub const USBSTS_HSE: u32 = 1 << 2;      // Host System Error
pub const USBSTS_EINT: u32 = 1 << 3;     // Event Interrupt
pub const USBSTS_PCD: u32 = 1 << 4;      // Port Change Detected
pub const USBSTS_SSS: u32 = 1 << 8;      // Save State Status
pub const USBSTS_RSS: u32 = 1 << 9;      // Restore State Status
pub const USBSTS_SRE: u32 = 1 << 10;     // Save/Restore Error
pub const USBSTS_CNR: u32 = 1 << 11;     // Controller Not Ready
pub const USBSTS_HCE: u32 = 1 << 12;     // Host Controller Error

impl OperationalRegisters {
    /// Read register at offset
    pub fn read(&self, offset: u64) -> u32 {
        match offset {
            0x00 => self.usbcmd.load(Ordering::Acquire),
            0x04 => self.usbsts.load(Ordering::Acquire),
            0x08 => self.pgsz.load(Ordering::Acquire),
            0x0C => self.dnctrl.load(Ordering::Acquire),
            0x10..=0x17 => self.crcr.load(Ordering::Acquire),
            0x30..=0x37 => self.dcbaap.load(Ordering::Acquire),
            0x38 => self.config.load(Ordering::Acquire),
            0x400.. => {
                let port_idx = ((offset - 0x400) / 0x10) as usize;
                let port_offset = (offset - 0x400) % 0x10;
                if port_idx < self.ports.len() {
                    self.ports[port_idx].read(port_offset)
                } else {
                    0
                }
            }
            _ => 0,
        }
    }

    /// Write register at offset
    pub fn write(&mut self, offset: u64, value: u32, state: &mut XhciState) {
        match offset {
            0x00 => {
                // USBCMD
                self.handle_usbcmd(value, state);
            }
            0x04 => {
                // USBSTS - write 1 to clear
                let current = self.usbsts.load(Ordering::Acquire);
                let clear_bits = value & (USBSTS_EINT | USBSTS_PCD | USBSTS_SRE | USBSTS_HCE);
                self.usbsts.store(current & !clear_bits, Ordering::Release);
            }
            0x0C => self.dnctrl.store(value, Ordering::Release),
            0x10..=0x17 => {
                let current = self.crcr.load(Ordering::Acquire);
                // Only update low bits, preserve high address bits
                let new_val = (current & !0xFF) | (value & 0xFF);
                self.crcr.store(new_val, Ordering::Release);
            }
            0x30..=0x37 => self.dcbaap.store(value, Ordering::Release),
            0x38 => self.config.store(value, Ordering::Release),
            0x400.. => {
                let port_idx = ((offset - 0x400) / 0x10) as usize;
                let port_offset = (offset - 0x400) % 0x10;
                if port_idx < self.ports.len() {
                    self.ports[port_idx].write(port_offset, value);
                }
            }
            _ => {}
        }
    }

    /// Handle USBCMD register write
    fn handle_usbcmd(&mut self, value: u32, state: &mut XhciState) {
        let current = self.usbcmd.load(Ordering::Acquire);

        // Host Controller Reset
        if value & USBCMD_HCRST != 0 {
            *state = XhciState::Reset;
            // Reset internal state
            self.usbsts.store(USBSTS_HCH | USBSTS_CNR, Ordering::Release);
            self.crcr.store(0, Ordering::Release);
            self.dcbaap.store(0, Ordering::Release);
            *state = XhciState::Halted;
            return;
        }

        // Run/Stop
        if (value ^ current) & USBCMD_RS != 0 {
            if value & USBCMD_RS != 0 {
                // Start controller
                *state = XhciState::Running;
                self.usbsts.store(USBSTS_CNR, Ordering::Release);
            } else {
                // Stop controller
                *state = XhciState::Halted;
                self.usbsts.store(USBSTS_HCH, Ordering::Release);
            }
        }

        self.usbcmd.store(value, Ordering::Release);
    }

    /// Set port connected state
    pub fn set_port_connected(&mut self, port: u8, connected: bool) {
        if port as usize >= self.ports.len() {
            return;
        }
        self.ports[port as usize].set_connected(connected);
    }

    /// Check if controller is running
    pub fn is_running(&self) -> bool {
        self.usbcmd.load(Ordering::Acquire) & USBCMD_RS != 0
    }
}

/// Port Status and Control Register
#[repr(C)]
#[derive(Debug, Default)]
struct PortRegister {
    /// PORTSC: Port Status and Control (0x00)
    portsc: AtomicU32,
    /// PORTPMSC: Port Power Management Status and Control (0x04)
    portpmsc: AtomicU32,
    /// PORTLI: Port Link Info (0x08)
    portli: AtomicU32,
    /// Reserved (0x0C)
    _reserved: AtomicU32,
}

/// PORTSC register bits
const PORTSC_CCS: u32 = 1 << 0;        // Current Connect Status
const PORTSC_PED: u32 = 1 << 1;        // Port Enable/Disable
const PORTSC_OCA: u32 = 1 << 3;        // Over-current Active
const PORTSC_PR: u32 = 1 << 4;         // Port Reset
const PORTSC_PLS_MASK: u32 = 0xF << 5; // Port Link State
const PORTSC_PP: u32 = 1 << 9;         // Port Power
const PORTSC_SPEED_MASK: u32 = 0xF << 10; // Port Speed
const PORTSC_LWS: u32 = 1 << 16;       // Port Link State Write Strobe
const PORTSC_CSC: u32 = 1 << 17;       // Connect Status Change
const PORTSC_PEC: u32 = 1 << 18;       // Port Enable/Disable Change
const PORTSC_WRC: u32 = 1 << 19;       // Warm Port Reset Change
const PORTSC_OCC: u32 = 1 << 20;       // Over-current Change
const PORTSC_PRC: u32 = 1 << 21;       // Port Reset Change
const PORTSC_PLC: u32 = 1 << 22;       // Port Link State Change
const PORTSC_CEC: u32 = 1 << 23;       // Port Config Error Change
const PORTSC_WPR: u32 = 1 << 31;       // Warm Port Reset

impl PortRegister {
    fn read(&self, offset: u64) -> u32 {
        match offset {
            0x00 => self.portsc.load(Ordering::Acquire),
            0x04 => self.portpmsc.load(Ordering::Acquire),
            0x08 => self.portli.load(Ordering::Acquire),
            _ => 0,
        }
    }

    fn write(&mut self, offset: u64, value: u32) {
        match offset {
            0x00 => {
                // PORTSC - some bits are write-clear, others are RW
                let current = self.portsc.load(Ordering::Acquire);

                // Clear status change bits on write of 1
                let clear_mask = value & (PORTSC_CSC | PORTSC_PEC | PORTSC_WRC
                    | PORTSC_OCC | PORTSC_PRC | PORTSC_PLC | PORTSC_CEC);
                let new_val = (current & !clear_mask)
                    | (value & (PORTSC_PED | PORTSC_PR | PORTSC_PLS_MASK | PORTSC_LWS | PORTSC_PP | PORTSC_WPR));

                self.portsc.store(new_val, Ordering::Release);
            }
            0x04 => self.portpmsc.store(value, Ordering::Release),
            0x08 => self.portli.store(value, Ordering::Release),
            _ => {}
        }
    }

    fn set_connected(&mut self, connected: bool) {
        let current = self.portsc.load(Ordering::Acquire);
        let new_val = if connected {
            current | PORTSC_CCS | PORTSC_CSC | PORTSC_PP | (4 << 10) // Connected, power on, full speed
        } else {
            current & !PORTSC_CCS
        };
        self.portsc.store(new_val | PORTSC_CSC, Ordering::Release); // Set change bit
    }
}

// ============================================================================
// Runtime Registers
// ============================================================================

/// xHCI Runtime Registers
#[derive(Debug, Default)]
pub struct RuntimeRegisters {
    /// MFINDEX: Microframe Index (0x00)
    mfindex: AtomicU32,
    /// Interrupter Registers (0x20+)
    interrupters: Vec<InterrupterRegister>,
}

impl RuntimeRegisters {
    /// Create new runtime registers
    pub fn new() -> Self {
        Self {
            mfindex: AtomicU32::new(0),
            interrupters: (0..super::XHCI_MAX_INTRS)
                .map(|_| InterrupterRegister::default())
                .collect(),
        }
    }

    /// Read register at offset
    pub fn read(&self, offset: u64) -> u32 {
        match offset {
            0x00 => self.mfindex.load(Ordering::Acquire),
            0x20.. => {
                let intr_idx = ((offset - 0x20) / 0x20) as usize;
                let intr_offset = (offset - 0x20) % 0x20;
                if intr_idx < self.interrupters.len() {
                    self.interrupters[intr_idx].read(intr_offset)
                } else {
                    0
                }
            }
            _ => 0,
        }
    }

    /// Write register at offset
    pub fn write(&mut self, offset: u64, value: u32) {
        match offset {
            0x00 => {} // MFINDEX is read-only
            0x20.. => {
                let intr_idx = ((offset - 0x20) / 0x20) as usize;
                let intr_offset = (offset - 0x20) % 0x20;
                if intr_idx < self.interrupters.len() {
                    self.interrupters[intr_idx].write(intr_offset, value);
                }
            }
            _ => {}
        }
    }
}

/// Interrupter Register Set
#[repr(C)]
#[derive(Debug, Default)]
struct InterrupterRegister {
    /// IMAN: Interrupter Management (0x00)
    iman: AtomicU32,
    /// IMOD: Interrupter Moderation (0x04)
    imod: AtomicU32,
    /// ERSTSZ: Event Ring Segment Table Size (0x08)
    erstsz: AtomicU32,
    /// Reserved (0x0C)
    _reserved1: AtomicU32,
    /// ERSTBA: Event Ring Segment Table Base Address (0x10)
    erstba: AtomicU32,
    /// Reserved (0x14)
    _reserved2: AtomicU32,
    /// ERDP: Event Ring Dequeue Pointer (0x18)
    erdp: AtomicU32,
}

/// IMAN register bits
const IMAN_IE: u32 = 1 << 1;  // Interrupt Enable
const IMAN_IP: u32 = 1 << 0;  // Interrupt Pending

impl InterrupterRegister {
    fn read(&self, offset: u64) -> u32 {
        match offset {
            0x00 => self.iman.load(Ordering::Acquire),
            0x04 => self.imod.load(Ordering::Acquire),
            0x08 => self.erstsz.load(Ordering::Acquire),
            0x10 => self.erstba.load(Ordering::Acquire),
            0x18 => self.erdp.load(Ordering::Acquire),
            _ => 0,
        }
    }

    fn write(&mut self, offset: u64, value: u32) {
        match offset {
            0x00 => {
                // IMAN - IP bit is write-clear
                let current = self.iman.load(Ordering::Acquire);
                let new_val = (current & !IMAN_IP) | (value & IMAN_IE);
                if value & IMAN_IP != 0 {
                    // Clear IP
                }
                self.iman.store(new_val, Ordering::Release);
            }
            0x04 => self.imod.store(value, Ordering::Release),
            0x08 => self.erstsz.store(value, Ordering::Release),
            0x10 => self.erstba.store(value, Ordering::Release),
            0x18 => self.erdp.store(value, Ordering::Release),
            _ => {}
        }
    }
}

// ============================================================================
// Doorbell Register
// ============================================================================

/// Doorbell Register (32-bit)
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct DoorbellRegister {
    /// Doorbell target (endpoint ID or command)
    pub target: u8,
    /// Reserved
    _reserved: [u8; 3],
}

impl DoorbellRegister {
    /// Create new doorbell register
    pub fn new(target: u8) -> Self {
        Self {
            target,
            _reserved: [0; 3],
        }
    }

    /// Read register value
    pub fn read(&self) -> u32 {
        self.target as u32
    }

    /// Write register value
    pub fn write(&mut self, value: u32) {
        self.target = (value & 0xFF) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capability_registers() {
        let caps = CapabilityRegisters::new(32, 8, 8);
        assert_eq!(caps.hciversion(), 0x0100);
    }

    #[test]
    fn test_operational_registers() {
        let mut op = OperationalRegisters::default();
        let mut state = XhciState::Halted;

        // Test reset
        op.write(0x00, USBCMD_HCRST, &mut state);
        assert_eq!(state, XhciState::Halted);

        // Test start
        op.write(0x00, USBCMD_RS, &mut state);
        assert_eq!(state, XhciState::Running);
    }

    #[test]
    fn test_port_register() {
        let mut port = PortRegister::default();
        port.set_connected(true);

        let val = port.read(0);
        assert!(val & PORTSC_CCS != 0);
        assert!(val & PORTSC_CSC != 0);
    }
}
