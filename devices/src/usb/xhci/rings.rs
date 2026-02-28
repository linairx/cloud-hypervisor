// Copyright 2024 Cloud Hypervisor Authors. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! xHCI Transfer Ring Implementation
//!
//! This module implements the xHCI transfer rings:
//! - Command Ring
//! - Event Ring
//! - Transfer Ring

use std::collections::VecDeque;

// ============================================================================
// TRB (Transfer Request Block) Definitions
// ============================================================================

/// TRB size in bytes
pub const TRB_SIZE: usize = 16;

/// TRB Types
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrbType {
    // Command TRBs
    EnableSlot = 9,
    DisableSlot = 10,
    AddressDevice = 11,
    ConfigureEndpoint = 12,
    EvaluateContext = 13,
    ResetEndpoint = 14,
    StopEndpoint = 15,
    SetTrDequeue = 16,
    ResetDevice = 17,
    ForceEvent = 18,
    NegotiateBandwidth = 19,
    SetLatencyTolerance = 20,
    GetPortBandwidth = 21,
    ForceHeader = 22,
    Noop = 23,

    // Transfer TRBs
    Normal = 1,
    SetupStage = 2,
    DataStage = 3,
    StatusStage = 4,
    Isoch = 5,
    Link = 6,
    EventData = 7,
    NoopTransfer = 8,

    // Event TRBs
    TransferEvent = 32,
    CommandCompletion = 33,
    PortStatusChange = 34,
    BandwidthRequest = 35,
    Doorbell = 36,
    HostController = 37,
    DeviceNotification = 38,
    MfindexWrap = 39,
}

impl TryFrom<u32> for TrbType {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(TrbType::Normal),
            2 => Ok(TrbType::SetupStage),
            3 => Ok(TrbType::DataStage),
            4 => Ok(TrbType::StatusStage),
            5 => Ok(TrbType::Isoch),
            6 => Ok(TrbType::Link),
            7 => Ok(TrbType::EventData),
            8 => Ok(TrbType::NoopTransfer),
            9 => Ok(TrbType::EnableSlot),
            10 => Ok(TrbType::DisableSlot),
            11 => Ok(TrbType::AddressDevice),
            12 => Ok(TrbType::ConfigureEndpoint),
            13 => Ok(TrbType::EvaluateContext),
            14 => Ok(TrbType::ResetEndpoint),
            15 => Ok(TrbType::StopEndpoint),
            16 => Ok(TrbType::SetTrDequeue),
            17 => Ok(TrbType::ResetDevice),
            18 => Ok(TrbType::ForceEvent),
            19 => Ok(TrbType::NegotiateBandwidth),
            20 => Ok(TrbType::SetLatencyTolerance),
            21 => Ok(TrbType::GetPortBandwidth),
            22 => Ok(TrbType::ForceHeader),
            23 => Ok(TrbType::Noop),
            32 => Ok(TrbType::TransferEvent),
            33 => Ok(TrbType::CommandCompletion),
            34 => Ok(TrbType::PortStatusChange),
            35 => Ok(TrbType::BandwidthRequest),
            36 => Ok(TrbType::Doorbell),
            37 => Ok(TrbType::HostController),
            38 => Ok(TrbType::DeviceNotification),
            39 => Ok(TrbType::MfindexWrap),
            _ => Err(()),
        }
    }
}

/// TRB Completion Codes
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionCode {
    Success = 1,
    Pending = 2,
    Error = 3,
    Stalled = 6,
    TrbError = 5,
    ShortPacket = 13,
    UndefinedError = 4,
    ResourceError = 7,
    BandwidthError = 8,
    NoSlotsAvailable = 9,
    InvalidStreamType = 10,
    SlotNotEnabled = 11,
    EndpointNotEnabled = 12,
    RingUnderrun = 14,
    RingOverrun = 15,
    VfEventRingFull = 16,
    ParameterError = 17,
    BandwidthOverrun = 18,
    ContextStateError = 19,
    NoPingResponse = 20,
    EventRingFull = 21,
    IncompatibleDevice = 22,
    MissedService = 23,
    CommandRingStopped = 24,
    CommandAborted = 25,
    Stopped = 26,
    StoppedLengthInvalid = 27,
    StoppedShortPacket = 28,
    ExitLatencyTooLarge = 29,
    IsochBufferOverrun = 31,
    EventLost = 32,
    UndefinedError2 = 33,
    InvalidStreamId = 34,
    SecondaryBandwidth = 35,
    SplitTransaction = 36,
}

/// Transfer Request Block (16 bytes)
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Trb {
    /// Parameter (64-bit)
    pub parameter: u64,
    /// Status (32-bit)
    pub status: u32,
    /// Control (32-bit)
    pub control: u32,
}

impl Trb {
    /// Create a new TRB
    pub fn new(parameter: u64, status: u32, control: u32) -> Self {
        Self {
            parameter,
            status,
            control,
        }
    }

    /// Get TRB type
    pub fn trb_type(&self) -> Option<TrbType> {
        TrbType::try_from((self.control >> 10) & 0x3F).ok()
    }

    /// Set TRB type
    pub fn set_trb_type(&mut self, trb_type: TrbType) {
        self.control = (self.control & !(0x3F << 10)) | ((trb_type as u32) << 10);
    }

    /// Check if this is the last TRB in a TD
    pub fn is_last(&self) -> bool {
        (self.control >> 1) & 1 != 0
    }

    /// Set last TRB flag
    pub fn set_last(&mut self, last: bool) {
        if last {
            self.control |= 1 << 1;
        } else {
            self.control &= !(1 << 1);
        }
    }

    /// Check if interrupt on completion is requested
    pub fn is_interrupt_on_completion(&self) -> bool {
        (self.control >> 5) & 1 != 0
    }

    /// Check if immediate data
    pub fn is_immediate_data(&self) -> bool {
        (self.control >> 6) & 1 != 0
    }

    /// Get cycle bit
    pub fn cycle_bit(&self) -> bool {
        self.control & 1 != 0
    }

    /// Set cycle bit
    pub fn set_cycle_bit(&mut self, cycle: bool) {
        if cycle {
            self.control |= 1;
        } else {
            self.control &= !1;
        }
    }

    /// Get transfer length
    pub fn transfer_length(&self) -> u32 {
        self.status & 0x1FFFF
    }

    /// Get completion code
    pub fn completion_code(&self) -> u8 {
        ((self.status >> 24) & 0xFF) as u8
    }

    /// Set completion code
    pub fn set_completion_code(&mut self, code: CompletionCode) {
        self.status = (self.status & !(0xFF << 24)) | ((code as u32) << 24);
    }

    /// Convert to bytes
    pub fn as_bytes(&self) -> [u8; TRB_SIZE] {
        let mut buf = [0u8; TRB_SIZE];
        buf[0..8].copy_from_slice(&self.parameter.to_le_bytes());
        buf[8..12].copy_from_slice(&self.status.to_le_bytes());
        buf[12..16].copy_from_slice(&self.control.to_le_bytes());
        buf
    }

    /// Create from bytes
    pub fn from_bytes(bytes: &[u8; TRB_SIZE]) -> Self {
        Self {
            parameter: u64::from_le_bytes(bytes[0..8].try_into().unwrap()),
            status: u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
            control: u32::from_le_bytes(bytes[12..16].try_into().unwrap()),
        }
    }
}

// ============================================================================
// Command Ring
// ============================================================================

/// Command Ring state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandRingState {
    Stopped,
    Running,
    Aborted,
}

/// Command Ring
#[derive(Debug)]
pub struct CommandRing {
    /// Ring base address
    base: u64,
    /// Ring size in TRBs
    size: usize,
    /// Current enqueue pointer
    enqueue: u64,
    /// Current cycle bit
    cycle: bool,
    /// Ring state
    state: CommandRingState,
    /// Pending command queue
    pending: VecDeque<Trb>,
}

impl CommandRing {
    /// Create a new command ring
    pub fn new() -> Self {
        Self {
            base: 0,
            size: 256,
            enqueue: 0,
            cycle: true,
            state: CommandRingState::Stopped,
            pending: VecDeque::new(),
        }
    }

    /// Initialize the ring
    pub fn init(&mut self, base: u64, size: usize) {
        self.base = base;
        self.size = size;
        self.enqueue = base;
        self.cycle = true;
        self.state = CommandRingState::Running;
    }

    /// Set base address
    pub fn set_base(&mut self, base: u64) {
        self.base = base;
        self.enqueue = base;
    }

    /// Get enqueue pointer
    pub fn enqueue_ptr(&self) -> u64 {
        self.enqueue
    }

    /// Set ring running
    pub fn start(&mut self) {
        self.state = CommandRingState::Running;
    }

    /// Stop the ring
    pub fn stop(&mut self) {
        self.state = CommandRingState::Stopped;
    }

    /// Abort the ring
    pub fn abort(&mut self) {
        self.state = CommandRingState::Aborted;
    }

    /// Check if running
    pub fn is_running(&self) -> bool {
        self.state == CommandRingState::Running
    }

    /// Queue a command TRB
    pub fn queue(&mut self, trb: Trb) {
        self.pending.push_back(trb);
    }

    /// Get next command
    pub fn next(&mut self) -> Option<Trb> {
        self.pending.pop_front()
    }

    /// Create event TRB for command completion
    pub fn create_completion_event(&self, command: &Trb, slot_id: u8, code: CompletionCode) -> Trb {
        let mut event = Trb::new(command.parameter, 0, 0);
        event.set_trb_type(TrbType::CommandCompletion);
        event.set_completion_code(code);
        event.status |= slot_id as u32;
        event.set_cycle_bit(true);
        event
    }
}

impl Default for CommandRing {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Event Ring
// ============================================================================

/// Event Ring Segment Table Entry
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct EventRingSegment {
    /// Ring segment base address
    pub base: u64,
    /// Ring segment size in TRBs
    pub size: u16,
    _reserved: [u8; 6],
}

impl EventRingSegment {
    /// Create new segment
    pub fn new(base: u64, size: u16) -> Self {
        Self {
            base,
            size,
            _reserved: [0; 6],
        }
    }
}

/// Event Ring
#[derive(Debug)]
pub struct EventRing {
    /// Event ring segments
    segments: Vec<EventRingSegment>,
    /// Current segment index
    segment_idx: usize,
    /// Current dequeue pointer within segment
    dequeue_idx: usize,
    /// Producer cycle state
    cycle: bool,
    /// Pending events
    pending: VecDeque<Trb>,
}

impl EventRing {
    /// Create a new event ring
    pub fn new() -> Self {
        Self {
            segments: Vec::new(),
            segment_idx: 0,
            dequeue_idx: 0,
            cycle: true,
            pending: VecDeque::new(),
        }
    }

    /// Set segment table
    pub fn set_segments(&mut self, segments: Vec<EventRingSegment>) {
        self.segments = segments;
        self.segment_idx = 0;
        self.dequeue_idx = 0;
    }

    /// Get dequeue pointer
    pub fn dequeue_ptr(&self) -> u64 {
        if let Some(segment) = self.segments.get(self.segment_idx) {
            segment.base + (self.dequeue_idx as u64 * TRB_SIZE as u64)
        } else {
            0
        }
    }

    /// Set dequeue pointer
    pub fn set_dequeue_ptr(&mut self, ptr: u64) {
        // Find the segment containing this pointer
        for (idx, segment) in self.segments.iter().enumerate() {
            if ptr >= segment.base && ptr < segment.base + (segment.size as u64 * TRB_SIZE as u64) {
                self.segment_idx = idx;
                self.dequeue_idx = ((ptr - segment.base) / TRB_SIZE as u64) as usize;
                break;
            }
        }
    }

    /// Queue an event TRB
    pub fn queue(&mut self, mut event: Trb) -> bool {
        event.set_cycle_bit(self.cycle);
        self.pending.push_back(event);

        // Check if we need to toggle cycle state
        if let Some(segment) = self.segments.get(self.segment_idx) {
            if self.dequeue_idx >= segment.size as usize - 1 {
                self.dequeue_idx = 0;
                self.segment_idx = (self.segment_idx + 1) % self.segments.len();
                self.cycle = !self.cycle;
            }
        }

        true
    }

    /// Get next event
    pub fn next(&mut self) -> Option<Trb> {
        self.pending.pop_front()
    }

    /// Check if events are pending
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
}

impl Default for EventRing {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Transfer Ring
// ============================================================================

/// Transfer Ring (for endpoints)
#[derive(Debug, Clone)]
pub struct TransferRing {
    /// Endpoint ID (1-31)
    ep_id: u8,
    /// Ring base address
    base: u64,
    /// Ring size in TRBs
    size: usize,
    /// Current enqueue pointer
    enqueue: u64,
    /// Current dequeue pointer
    dequeue: u64,
    /// Producer cycle state
    pcs: bool,
    /// Consumer cycle state
    ccs: bool,
    /// Transfer TRBs pending processing
    pending: VecDeque<Trb>,
}

impl TransferRing {
    /// Create a new transfer ring
    pub fn new(ep_id: u8) -> Self {
        Self {
            ep_id,
            base: 0,
            size: 256,
            enqueue: 0,
            dequeue: 0,
            pcs: true,
            ccs: true,
            pending: VecDeque::new(),
        }
    }

    /// Initialize the ring
    pub fn init(&mut self, base: u64, size: usize) {
        self.base = base;
        self.size = size;
        self.enqueue = base;
        self.dequeue = base;
        self.pcs = true;
        self.ccs = true;
    }

    /// Get enqueue pointer
    pub fn enqueue_ptr(&self) -> u64 {
        self.enqueue
    }

    /// Get dequeue pointer
    pub fn dequeue_ptr(&self) -> u64 {
        self.dequeue
    }

    /// Set dequeue pointer
    pub fn set_dequeue_ptr(&mut self, ptr: u64, cycle: bool) {
        self.dequeue = ptr;
        self.ccs = cycle;
    }

    /// Queue a transfer TRB
    pub fn queue(&mut self, trb: Trb) {
        self.pending.push_back(trb);
    }

    /// Get next transfer TRB
    pub fn next(&mut self) -> Option<Trb> {
        self.pending.pop_front()
    }

    /// Check if pending transfers exist
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Create transfer event
    pub fn create_transfer_event(&self, trb: &Trb, code: CompletionCode, length: u32) -> Trb {
        let mut event = Trb::new(trb.parameter, 0, 0);
        event.set_trb_type(TrbType::TransferEvent);
        event.set_completion_code(code);
        event.status |= length;
        event.set_cycle_bit(true);
        event
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trb_creation() {
        let mut trb = Trb::new(0x12345678, 0x100, 0);
        trb.set_trb_type(TrbType::Normal);
        trb.set_last(true);
        trb.set_cycle_bit(true);

        assert_eq!(trb.trb_type(), Some(TrbType::Normal));
        assert!(trb.is_last());
        assert!(trb.cycle_bit());
    }

    #[test]
    fn test_command_ring() {
        let mut ring = CommandRing::new();
        ring.init(0x10000, 256);

        assert!(ring.is_running());
        assert_eq!(ring.enqueue_ptr(), 0x10000);
    }

    #[test]
    fn test_event_ring() {
        let mut ring = EventRing::new();
        let segment = EventRingSegment::new(0x20000, 16);
        ring.set_segments(vec![segment]);

        let event = Trb::new(0, 0, 0);
        assert!(ring.queue(event));
    }
}
