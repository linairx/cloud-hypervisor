// Copyright 2024 Tencent Corporation. All rights reserved.
//
// SPDX-License-Identifier: Apache-2.0
//
// VirtIO Input Device Implementation - Minimal stub for input injection API

use std::collections::VecDeque;
use std::io;
use std::result;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::anyhow;
use log::error;
use seccompiler::SeccompAction;
use serde::{Deserialize, Serialize};
use virtio_queue::{Queue, QueueT};
use vm_memory::{ByteValued, GuestMemoryAtomic};
use vm_migration::{
    Migratable, MigratableError, Pausable, Snapshot, Snapshottable, Transportable,
};
use vm_virtio::AccessPlatform;
use vmm_sys_util::eventfd::EventFd;

use super::{
    ActivateError, ActivateResult, Error as DeviceError, VirtioDevice, VirtioDeviceType,
    VirtioInterrupt,
};
use crate::GuestMemoryMmap;

/// VirtIO Input device ID type
pub const VIRTIO_ID_INPUT: u32 = 18;

/// Queue sizes
const QUEUE_SIZES: &[u16] = &[64, 64];

/// VirtIO Input event structure
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct VirtioInputEvent {
    /// Event type
    pub ev_type: u16,
    /// Event code
    pub code: u16,
    /// Event value
    pub value: u32,
}

unsafe impl ByteValued for VirtioInputEvent {}

impl VirtioInputEvent {
    /// Create a keyboard event
    pub fn keyboard(code: u16, pressed: bool) -> Self {
        Self {
            ev_type: 0x01, // EV_KEY
            code,
            value: if pressed { 1 } else { 0 },
        }
    }

    /// Create a relative mouse event
    pub fn rel(code: u16, value: i32) -> Self {
        Self {
            ev_type: 0x02, // EV_REL
            code,
            value: value as u32,
        }
    }

    /// Create a sync event
    pub fn syn() -> Self {
        Self {
            ev_type: 0x00, // EV_SYN
            code: 0,
            value: 0,
        }
    }
}

/// Maximum event queue size
const EVENT_QUEUE_SIZE: usize = 256;

/// VirtIO Input device
pub struct Input {
    /// Device ID
    id: String,
    /// Available features
    avail_features: u64,
    /// Acked features
    acked_features: u64,
    /// Event queue for injection
    events: Arc<Mutex<VecDeque<VirtioInputEvent>>>,
    /// EventFd to signal new events
    event_fd: EventFd,
    /// Activated flag
    activated: Arc<AtomicBool>,
    /// Interrupt callback
    interrupt_cb: Option<Arc<dyn VirtioInterrupt>>,
    /// Access platform
    access_platform: Option<Arc<dyn AccessPlatform>>,
    /// Seccomp action
    seccomp_action: SeccompAction,
    /// Exit event
    exit_evt: EventFd,
}

impl Input {
    /// Create a new VirtIO Input device
    pub fn new(
        id: String,
        _seccomp_action: SeccompAction,
        exit_evt: EventFd,
    ) -> io::Result<Self> {
        Ok(Input {
            id,
            avail_features: 0u64,
            acked_features: 0u64,
            events: Arc::new(Mutex::new(VecDeque::with_capacity(EVENT_QUEUE_SIZE))),
            event_fd: EventFd::new(libc::EFD_NONBLOCK)?,
            activated: Arc::new(AtomicBool::new(false)),
            interrupt_cb: None,
            access_platform: None,
            seccomp_action: SeccompAction::Trap,
            exit_evt,
        })
    }

    /// Inject an input event
    pub fn inject_event(&self, event: VirtioInputEvent) -> io::Result<()> {
        if let Ok(mut events) = self.events.lock() {
            if events.len() < EVENT_QUEUE_SIZE {
                events.push_back(event);
                // Signal that new event is available
                self.event_fd.write(1)?;
                return Ok(());
            }
        }
        Err(io::Error::new(io::ErrorKind::Other, "Event queue full"))
    }

    /// Inject a keyboard event
    pub fn inject_keyboard(&self, code: u16, pressed: bool) -> io::Result<()> {
        self.inject_event(VirtioInputEvent::keyboard(code, pressed))
    }

    /// Inject a mouse relative movement
    pub fn inject_mouse_rel(&self, dx: i32, dy: i32) -> io::Result<()> {
        self.inject_event(VirtioInputEvent::rel(0x00, dx))?; // REL_X
        self.inject_event(VirtioInputEvent::rel(0x01, dy))?; // REL_Y
        self.inject_event(VirtioInputEvent::syn())
    }

    /// Inject a mouse button event
    pub fn inject_mouse_button(&self, button: u16, pressed: bool) -> io::Result<()> {
        self.inject_event(VirtioInputEvent::keyboard(button, pressed))
    }

    /// Inject a mouse wheel event
    pub fn inject_mouse_wheel(&self, delta: i32) -> io::Result<()> {
        self.inject_event(VirtioInputEvent::rel(0x08, delta)) // REL_WHEEL
    }
}

impl VirtioDevice for Input {
    fn device_type(&self) -> u32 {
        VIRTIO_ID_INPUT
    }

    fn queue_max_sizes(&self) -> &[u16] {
        QUEUE_SIZES
    }

    fn features(&self) -> u64 {
        self.avail_features
    }

    fn ack_features(&mut self, value: u64) {
        let _ = value;
    }

    fn read_config(&self, _offset: u64, _data: &mut [u8]) {
        // No config space for now
    }

    fn write_config(&mut self, _offset: u64, _data: &[u8]) {
        // No config space for now
    }

    fn activate(
        &mut self,
        _mem: GuestMemoryAtomic<GuestMemoryMmap>,
        interrupt_cb: Arc<dyn VirtioInterrupt>,
        _queues: Vec<(usize, Queue, EventFd)>,
    ) -> ActivateResult {
        self.interrupt_cb = Some(interrupt_cb);
        self.activated.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn reset(&mut self) -> Option<Arc<dyn VirtioInterrupt>> {
        self.activated.store(false, Ordering::SeqCst);
        self.interrupt_cb.take()
    }

    fn set_access_platform(&mut self, access_platform: Arc<dyn AccessPlatform>) {
        self.access_platform = Some(access_platform);
    }
}

impl Pausable for Input {}

impl Snapshottable for Input {
    fn id(&self) -> String {
        self.id.clone()
    }

    fn snapshot(&mut self) -> result::Result<Snapshot, MigratableError> {
        Snapshot::new_from_state(&InputState {
            avail_features: self.avail_features,
            acked_features: self.acked_features,
        })
    }
}

impl Transportable for Input {}

impl Migratable for Input {}

/// Input device state for migration
#[derive(Deserialize, Serialize)]
pub struct InputState {
    pub avail_features: u64,
    pub acked_features: u64,
}
