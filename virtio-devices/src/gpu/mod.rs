// Copyright 2024 Cloud Hypervisor Authors. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! VirtIO GPU Device
//!
//! This module provides a VirtIO GPU device with basic 2D rendering support.

use std::collections::HashMap;
use std::io;
use std::os::unix::io::AsRawFd;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Barrier, Mutex};

use anyhow::anyhow;
use log::error;
use seccompiler::SeccompAction;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use virtio_queue::{Queue, QueueT};
use vm_memory::{
    Address, ByteValued, Bytes, GuestAddress, GuestAddressSpace, GuestMemory, GuestMemoryAtomic,
    GuestMemoryError,
};
use vm_migration::{Migratable, Pausable, Snapshottable, Transportable};
use vmm_sys_util::eventfd::EventFd;

use super::{ActivateResult, VirtioCommon, VirtioDevice, VirtioDeviceType, VirtioInterrupt};
use crate::seccomp_filters::Thread;
use crate::thread_helper::spawn_virtio_thread;
use crate::{
    EPOLL_HELPER_EVENT_LAST, EpollHelper, EpollHelperError, EpollHelperHandler, GuestMemoryMmap,
};

/// Queue sizes
const QUEUE_SIZE: u16 = 256;
const NUM_QUEUES: usize = 2;
const QUEUE_SIZES: &[u16] = &[QUEUE_SIZE; NUM_QUEUES];

/// Control queue index
const CONTROL_QUEUE: usize = 0;
const CURSOR_QUEUE: usize = 1;

/// EDID feature bit
pub const VIRTIO_GPU_F_EDID: u64 = 1;

// VirtIO GPU commands
const VIRTIO_GPU_CMD_GET_DISPLAY_INFO: u32 = 0x0100;
const VIRTIO_GPU_CMD_RESOURCE_CREATE_2D: u32 = 0x0101;
const VIRTIO_GPU_CMD_RESOURCE_UNREF: u32 = 0x0102;
const VIRTIO_GPU_CMD_SET_SCANOUT: u32 = 0x0103;
const VIRTIO_GPU_CMD_RESOURCE_FLUSH: u32 = 0x0104;
const VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D: u32 = 0x0105;
const VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING: u32 = 0x0106;
const VIRTIO_GPU_CMD_RESOURCE_DETACH_BACKING: u32 = 0x0107;

// VirtIO GPU responses
const VIRTIO_GPU_RESP_OK_NODATA: u32 = 0x1100;
const VIRTIO_GPU_RESP_OK_DISPLAY_INFO: u32 = 0x1101;
const VIRTIO_GPU_RESP_ERR_UNSPEC: u32 = 0x1200;
const VIRTIO_GPU_RESP_ERR_OUT_OF_MEMORY: u32 = 0x1201;
const VIRTIO_GPU_RESP_ERR_INVALID_RESOURCE_ID: u32 = 0x1202;
const VIRTIO_GPU_RESP_ERR_INVALID_SCANOUT_ID: u32 = 0x1203;

// Response flags
const VIRTIO_GPU_FLAG_FENCE: u32 = 1 << 0;

// Control queue event
const CONTROL_QUEUE_EVENT: u16 = EPOLL_HELPER_EVENT_LAST + 1;
// Cursor queue event
const CURSOR_QUEUE_EVENT: u16 = EPOLL_HELPER_EVENT_LAST + 2;

// Pixel formats
const VIRTIO_GPU_FORMAT_B8G8R8A8_UNORM: u32 = 1;
const VIRTIO_GPU_FORMAT_B8G8R8X8_UNORM: u32 = 2;
const VIRTIO_GPU_FORMAT_A8R8G8B8_UNORM: u32 = 3;
const VIRTIO_GPU_FORMAT_X8R8G8B8_UNORM: u32 = 4;
const VIRTIO_GPU_FORMAT_R8G8B8A8_UNORM: u32 = 67;
const VIRTIO_GPU_FORMAT_X8B8G8R8_UNORM: u32 = 68;
const VIRTIO_GPU_FORMAT_A8B8G8R8_UNORM: u32 = 69;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Guest gave us bad memory addresses.")]
    GuestMemory(#[source] GuestMemoryError),
    #[error("Guest gave us a write only descriptor that protocol says to read from")]
    UnexpectedWriteOnlyDescriptor,
    #[error("Guest sent us invalid request")]
    InvalidRequest,
    #[error("Descriptor chain is too short")]
    DescriptorChainTooShort,
    #[error("Failed adding used index")]
    QueueAddUsed(#[source] virtio_queue::Error),
    #[error("Failed creating an iterator over the queue")]
    QueueIterator(#[source] virtio_queue::Error),
    #[error("Failed to signal")]
    FailedSignal(#[source] io::Error),
    #[error("Invalid resource ID")]
    InvalidResourceId,
    #[error("Invalid scanout ID")]
    InvalidScanoutId,
    #[error("Resource not found")]
    ResourceNotFound,
    #[error("Invalid format")]
    InvalidFormat,
}

/// Command header
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct CtrlHeader {
    hdr_type: u32,
    flags: u32,
    fence_id: u64,
    ctx_id: u32,
    padding: u32,
}

// SAFETY: CtrlHeader is POD and has no implicit padding
unsafe impl ByteValued for CtrlHeader {}

/// Rectangle
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct Rect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

// SAFETY: Rect is POD and has no implicit padding
unsafe impl ByteValued for Rect {}

/// Display info for one scanout
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct DisplayOne {
    r: Rect,
    enabled: u32,
    flags: u32,
}

// SAFETY: DisplayOne is POD and has no implicit padding
unsafe impl ByteValued for DisplayOne {}

/// Display info response
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct DisplayInfo {
    header: CtrlHeader,
    pmodes: [DisplayOne; 16],
}

// SAFETY: DisplayInfo is POD
unsafe impl ByteValued for DisplayInfo {}

/// Resource create 2D command
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct ResourceCreate2D {
    header: CtrlHeader,
    resource_id: u32,
    format: u32,
    width: u32,
    height: u32,
}

// SAFETY: ResourceCreate2D is POD
unsafe impl ByteValued for ResourceCreate2D {}

/// Resource unref command
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct ResourceUnref {
    header: CtrlHeader,
    resource_id: u32,
    padding: u32,
}

// SAFETY: ResourceUnref is POD
unsafe impl ByteValued for ResourceUnref {}

/// Set scanout command
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct SetScanout {
    header: CtrlHeader,
    r: Rect,
    scanout_id: u32,
    resource_id: u32,
}

// SAFETY: SetScanout is POD
unsafe impl ByteValued for SetScanout {}

/// Transfer to host 2D command
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct TransferToHost2D {
    header: CtrlHeader,
    r: Rect,
    offset: u64,
    resource_id: u32,
    padding: u32,
}

// SAFETY: TransferToHost2D is POD
unsafe impl ByteValued for TransferToHost2D {}

/// Resource flush command
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct ResourceFlush {
    header: CtrlHeader,
    r: Rect,
    resource_id: u32,
    padding: u32,
}

// SAFETY: ResourceFlush is POD
unsafe impl ByteValued for ResourceFlush {}

/// Resource attach backing command
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct ResourceAttachBacking {
    header: CtrlHeader,
    resource_id: u32,
    nr_entries: u32,
}

// SAFETY: ResourceAttachBacking is POD
unsafe impl ByteValued for ResourceAttachBacking {}

/// Memory entry for backing storage
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct MemEntry {
    addr: u64,
    length: u32,
    padding: u32,
}

// SAFETY: MemEntry is POD
unsafe impl ByteValued for MemEntry {}

/// 2D resource
#[derive(Debug, Clone)]
struct Resource2D {
    width: u32,
    height: u32,
    format: u32,
    /// Backing storage entries
    backing: Vec<MemEntry>,
    /// Simple pixel buffer (for basic software rendering)
    data: Vec<u8>,
}

impl Resource2D {
    fn new(width: u32, height: u32, format: u32) -> Result<Self, Error> {
        let bytes_per_pixel = match format {
            VIRTIO_GPU_FORMAT_B8G8R8A8_UNORM
            | VIRTIO_GPU_FORMAT_B8G8R8X8_UNORM
            | VIRTIO_GPU_FORMAT_A8R8G8B8_UNORM
            | VIRTIO_GPU_FORMAT_X8R8G8B8_UNORM
            | VIRTIO_GPU_FORMAT_R8G8B8A8_UNORM
            | VIRTIO_GPU_FORMAT_X8B8G8R8_UNORM
            | VIRTIO_GPU_FORMAT_A8B8G8R8_UNORM => 4,
            _ => return Err(Error::InvalidFormat),
        };

        let size = (width as usize) * (height as usize) * (bytes_per_pixel as usize);
        Ok(Self {
            width,
            height,
            format,
            backing: Vec::new(),
            data: vec![0u8; size],
        })
    }
}

/// GPU configuration
#[repr(C)]
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct GpuConfig {
    pub num_scanouts: u32,
    pub reserved: u32,
}

impl Default for GpuConfig {
    fn default() -> Self {
        Self {
            num_scanouts: 1, // Default to 1 scanout
            reserved: 0,
        }
    }
}

// SAFETY: GpuConfig is POD and has no implicit padding
unsafe impl ByteValued for GpuConfig {}

impl GpuConfig {
    fn as_slice(&self) -> &[u8] {
        unsafe {
            std::slice::from_raw_parts(
                self as *const GpuConfig as *const u8,
                std::mem::size_of::<GpuConfig>(),
            )
        }
    }

    fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe {
            std::slice::from_raw_parts_mut(
                self as *mut GpuConfig as *mut u8,
                std::mem::size_of::<GpuConfig>(),
            )
        }
    }
}

/// VirtIO GPU device
pub struct Gpu {
    /// Common virtio device data
    common: VirtioCommon,
    /// GPU configuration
    config: Arc<Mutex<GpuConfig>>,
    /// Display width
    display_width: u32,
    /// Display height
    display_height: u32,
    /// 2D resources
    resources: Arc<Mutex<HashMap<u32, Resource2D>>>,
    /// Seccomp action
    seccomp_action: SeccompAction,
    /// Exit event
    exit_evt: EventFd,
    /// Interrupt callback
    interrupt_cb: Option<Arc<dyn VirtioInterrupt>>,
}

impl Gpu {
    /// Create a new VirtIO GPU device
    pub fn new(
        display_width: u32,
        display_height: u32,
        seccomp_action: SeccompAction,
        exit_evt: EventFd,
    ) -> io::Result<Self> {
        Ok(Self {
            common: VirtioCommon {
                device_type: VirtioDeviceType::Gpu as u32,
                queue_sizes: QUEUE_SIZES.to_vec(),
                avail_features: 1u64 << VIRTIO_GPU_F_EDID,
                paused_sync: Some(Arc::new(Barrier::new(2))),
                min_queues: NUM_QUEUES as u16,
                ..Default::default()
            },
            config: Arc::new(Mutex::new(GpuConfig::default())),
            display_width,
            display_height,
            resources: Arc::new(Mutex::new(HashMap::new())),
            seccomp_action,
            exit_evt,
            interrupt_cb: None,
        })
    }

    /// Get display dimensions
    pub fn display_dimensions(&self) -> (u32, u32) {
        (self.display_width, self.display_height)
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        if let Some(kill_evt) = self.common.kill_evt.take() {
            // Ignore the result because there is nothing we can do about it.
            let _ = kill_evt.write(1);
        }
        self.common.wait_for_epoll_threads();
    }
}

/// GPU epoll handler
struct GpuEpollHandler {
    mem: GuestMemoryAtomic<GuestMemoryMmap>,
    queues: Vec<Queue>,
    interrupt_cb: Arc<dyn VirtioInterrupt>,
    control_queue_evt: EventFd,
    cursor_queue_evt: EventFd,
    kill_evt: EventFd,
    pause_evt: EventFd,
    resources: Arc<Mutex<HashMap<u32, Resource2D>>>,
    display_width: u32,
    display_height: u32,
}

impl GpuEpollHandler {
    fn signal(&self, int_type: super::VirtioInterruptType) -> Result<(), Error> {
        self.interrupt_cb.trigger(int_type).map_err(|e| {
            error!("Failed to signal used queue: {e:?}");
            Error::FailedSignal(e)
        })
    }

    /// Create a response header
    fn create_response_header(hdr_type: u32) -> CtrlHeader {
        CtrlHeader {
            hdr_type,
            flags: 0,
            fence_id: 0,
            ctx_id: 0,
            padding: 0,
        }
    }

    /// Handle GET_DISPLAY_INFO command
    fn handle_get_display_info(&self) -> DisplayInfo {
        let mut pmodes = [DisplayOne::default(); 16];
        pmodes[0] = DisplayOne {
            r: Rect {
                x: 0,
                y: 0,
                width: self.display_width,
                height: self.display_height,
            },
            enabled: 1,
            flags: 0,
        };

        DisplayInfo {
            header: Self::create_response_header(VIRTIO_GPU_RESP_OK_DISPLAY_INFO),
            pmodes,
        }
    }

    /// Handle RESOURCE_CREATE_2D command
    fn handle_resource_create_2d(&mut self, cmd: &ResourceCreate2D) -> CtrlHeader {
        if cmd.resource_id == 0 {
            return Self::create_response_header(VIRTIO_GPU_RESP_ERR_INVALID_RESOURCE_ID);
        }

        match Resource2D::new(cmd.width, cmd.height, cmd.format) {
            Ok(resource) => {
                let mut resources = self.resources.lock().expect("Failed to lock resources mutex: another thread panicked while holding the lock");
                resources.insert(cmd.resource_id, resource);
                Self::create_response_header(VIRTIO_GPU_RESP_OK_NODATA)
            }
            Err(Error::InvalidFormat) => {
                Self::create_response_header(VIRTIO_GPU_RESP_ERR_UNSPEC)
            }
            _ => Self::create_response_header(VIRTIO_GPU_RESP_ERR_OUT_OF_MEMORY),
        }
    }

    /// Handle RESOURCE_UNREF command
    fn handle_resource_unref(&mut self, cmd: &ResourceUnref) -> CtrlHeader {
        if cmd.resource_id == 0 {
            return Self::create_response_header(VIRTIO_GPU_RESP_ERR_INVALID_RESOURCE_ID);
        }

        let mut resources = self.resources.lock().expect("Failed to lock resources mutex: another thread panicked while holding the lock");
        resources.remove(&cmd.resource_id);
        Self::create_response_header(VIRTIO_GPU_RESP_OK_NODATA)
    }

    /// Handle SET_SCANOUT command
    fn handle_set_scanout(&mut self, cmd: &SetScanout) -> CtrlHeader {
        if cmd.scanout_id >= 16 {
            return Self::create_response_header(VIRTIO_GPU_RESP_ERR_INVALID_SCANOUT_ID);
        }

        // For now, we just accept the scanout configuration
        // In a full implementation, this would configure the display output
        Self::create_response_header(VIRTIO_GPU_RESP_OK_NODATA)
    }

    /// Handle RESOURCE_ATTACH_BACKING command
    fn handle_resource_attach_backing(
        &mut self,
        cmd: &ResourceAttachBacking,
        entries: Vec<MemEntry>,
    ) -> CtrlHeader {
        if cmd.resource_id == 0 {
            return Self::create_response_header(VIRTIO_GPU_RESP_ERR_INVALID_RESOURCE_ID);
        }

        let mut resources = self.resources.lock().expect("Failed to lock resources mutex: another thread panicked while holding the lock");
        if let Some(resource) = resources.get_mut(&cmd.resource_id) {
            resource.backing = entries;
            Self::create_response_header(VIRTIO_GPU_RESP_OK_NODATA)
        } else {
            Self::create_response_header(VIRTIO_GPU_RESP_ERR_INVALID_RESOURCE_ID)
        }
    }

    /// Handle TRANSFER_TO_HOST_2D command
    fn handle_transfer_to_host_2d<M: GuestMemory>(
        &mut self,
        mem: &M,
        cmd: &TransferToHost2D,
    ) -> CtrlHeader {
        if cmd.resource_id == 0 {
            return Self::create_response_header(VIRTIO_GPU_RESP_ERR_INVALID_RESOURCE_ID);
        }

        let mut resources = self.resources.lock().expect("Failed to lock resources mutex: another thread panicked while holding the lock");
        if let Some(resource) = resources.get_mut(&cmd.resource_id) {
            // Copy data from guest memory to resource buffer
            if !resource.backing.is_empty() {
                let backing = &resource.backing[0];
                let src_addr = GuestAddress(backing.addr + cmd.offset);
                let dst_start = (cmd.r.y as usize * resource.width as usize + cmd.r.x as usize) * 4;
                let bytes_to_copy = std::cmp::min(
                    (cmd.r.width * cmd.r.height * 4) as usize,
                    resource.data.len().saturating_sub(dst_start),
                );

                // Read from guest memory
                if bytes_to_copy > 0 && dst_start + bytes_to_copy <= resource.data.len() {
                    if mem
                        .read_slice(&mut resource.data[dst_start..dst_start + bytes_to_copy], src_addr)
                        .is_err()
                    {
                        // Log read error but continue processing
                        error!("Failed to read from guest memory during TRANSFER_TO_HOST_2D");
                    }
                }
            }
            Self::create_response_header(VIRTIO_GPU_RESP_OK_NODATA)
        } else {
            Self::create_response_header(VIRTIO_GPU_RESP_ERR_INVALID_RESOURCE_ID)
        }
    }

    /// Handle RESOURCE_FLUSH command
    fn handle_resource_flush(&mut self, cmd: &ResourceFlush) -> CtrlHeader {
        if cmd.resource_id == 0 {
            return Self::create_response_header(VIRTIO_GPU_RESP_ERR_INVALID_RESOURCE_ID);
        }

        // In a full implementation, this would trigger a redraw of the specified region
        // For now, we just acknowledge the flush
        Self::create_response_header(VIRTIO_GPU_RESP_OK_NODATA)
    }

    /// Process the control queue
    fn process_control_queue(&mut self) -> Result<(), Error> {
        let mut used_descs = false;

        while let Some(mut desc_chain) = self.queues[CONTROL_QUEUE]
            .pop_descriptor_chain(self.mem.memory())
        {
            let head_desc = desc_chain.next().ok_or(Error::DescriptorChainTooShort)?;

            if head_desc.is_write_only() {
                error!("The head descriptor is write-only");
                return Err(Error::UnexpectedWriteOnlyDescriptor);
            }

            // Read command header
            let header: CtrlHeader = desc_chain
                .memory()
                .read_obj(head_desc.addr())
                .map_err(Error::GuestMemory)?;

            let response = match header.hdr_type {
                VIRTIO_GPU_CMD_GET_DISPLAY_INFO => {
                    let display_info = self.handle_get_display_info();
                    // Write response to the next descriptor
                    if let Some(resp_desc) = desc_chain.next() {
                        if resp_desc.is_write_only() {
                            let _ = desc_chain.memory().write_obj(display_info, resp_desc.addr());
                        }
                    }
                    self.queues[CONTROL_QUEUE]
                        .add_used(
                            desc_chain.memory(),
                            desc_chain.head_index(),
                            std::mem::size_of::<DisplayInfo>() as u32,
                        )
                        .map_err(Error::QueueAddUsed)?;
                    used_descs = true;
                    continue;
                }
                VIRTIO_GPU_CMD_RESOURCE_CREATE_2D => {
                    let cmd: ResourceCreate2D = desc_chain
                        .memory()
                        .read_obj(head_desc.addr())
                        .map_err(Error::GuestMemory)?;
                    self.handle_resource_create_2d(&cmd)
                }
                VIRTIO_GPU_CMD_RESOURCE_UNREF => {
                    let cmd: ResourceUnref = desc_chain
                        .memory()
                        .read_obj(head_desc.addr())
                        .map_err(Error::GuestMemory)?;
                    self.handle_resource_unref(&cmd)
                }
                VIRTIO_GPU_CMD_SET_SCANOUT => {
                    let cmd: SetScanout = desc_chain
                        .memory()
                        .read_obj(head_desc.addr())
                        .map_err(Error::GuestMemory)?;
                    self.handle_set_scanout(&cmd)
                }
                VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D => {
                    let cmd: TransferToHost2D = desc_chain
                        .memory()
                        .read_obj(head_desc.addr())
                        .map_err(Error::GuestMemory)?;
                    self.handle_transfer_to_host_2d(desc_chain.memory(), &cmd)
                }
                VIRTIO_GPU_CMD_RESOURCE_FLUSH => {
                    let cmd: ResourceFlush = desc_chain
                        .memory()
                        .read_obj(head_desc.addr())
                        .map_err(Error::GuestMemory)?;
                    self.handle_resource_flush(&cmd)
                }
                VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING => {
                    let cmd: ResourceAttachBacking = desc_chain
                        .memory()
                        .read_obj(head_desc.addr())
                        .map_err(Error::GuestMemory)?;

                    // Read memory entries from the next descriptor
                    let mut entries = Vec::new();
                    if let Some(entry_desc) = desc_chain.next() {
                        for i in 0..cmd.nr_entries as usize {
                            let offset = i * std::mem::size_of::<MemEntry>();
                            if let Some(addr) = entry_desc.addr().checked_add(offset as u64) {
                                if let Ok(entry) = desc_chain.memory().read_obj::<MemEntry>(addr) {
                                    entries.push(entry);
                                }
                            }
                        }
                    }
                    self.handle_resource_attach_backing(&cmd, entries)
                }
                VIRTIO_GPU_CMD_RESOURCE_DETACH_BACKING => {
                    // Simplified handling - just acknowledge
                    Self::create_response_header(VIRTIO_GPU_RESP_OK_NODATA)
                }
                _ => {
                    error!("Unknown GPU command: 0x{:x}", header.hdr_type);
                    Self::create_response_header(VIRTIO_GPU_RESP_ERR_UNSPEC)
                }
            };

            // Write response to the next descriptor (if writeable)
            if let Some(resp_desc) = desc_chain.next() {
                if resp_desc.is_write_only() {
                    let _ = desc_chain.memory().write_obj(response, resp_desc.addr());
                }
            }

            self.queues[CONTROL_QUEUE]
                .add_used(
                    desc_chain.memory(),
                    desc_chain.head_index(),
                    std::mem::size_of::<CtrlHeader>() as u32,
                )
                .map_err(Error::QueueAddUsed)?;
            used_descs = true;
        }

        if used_descs {
            self.signal(super::VirtioInterruptType::Queue(CONTROL_QUEUE as u16))
        } else {
            Ok(())
        }
    }

    /// Process the cursor queue (minimal implementation)
    fn process_cursor_queue(&mut self) -> Result<(), Error> {
        let mut used_descs = false;

        while let Some(mut desc_chain) = self.queues[CURSOR_QUEUE]
            .pop_descriptor_chain(self.mem.memory())
        {
            // For now, just acknowledge and discard cursor commands
            let head_desc = desc_chain.next().ok_or(Error::DescriptorChainTooShort)?;

            self.queues[CURSOR_QUEUE]
                .add_used(
                    desc_chain.memory(),
                    desc_chain.head_index(),
                    head_desc.len(),
                )
                .map_err(Error::QueueAddUsed)?;
            used_descs = true;
        }

        if used_descs {
            self.signal(super::VirtioInterruptType::Queue(CURSOR_QUEUE as u16))
        } else {
            Ok(())
        }
    }

    fn run(
        &mut self,
        paused: &AtomicBool,
        paused_sync: &Barrier,
    ) -> std::result::Result<(), EpollHelperError> {
        let mut helper = EpollHelper::new(&self.kill_evt, &self.pause_evt)?;
        helper.add_event(self.control_queue_evt.as_raw_fd(), CONTROL_QUEUE_EVENT)?;
        helper.add_event(self.cursor_queue_evt.as_raw_fd(), CURSOR_QUEUE_EVENT)?;
        helper.run(paused, paused_sync, self)?;

        Ok(())
    }
}

impl EpollHelperHandler for GpuEpollHandler {
    fn handle_event(
        &mut self,
        _helper: &mut EpollHelper,
        event: &epoll::Event,
    ) -> std::result::Result<(), EpollHelperError> {
        let ev_type = event.data as u16;
        match ev_type {
            CONTROL_QUEUE_EVENT => {
                self.control_queue_evt.read().map_err(|e| {
                    EpollHelperError::HandleEvent(anyhow!(
                        "Failed to get control queue event: {e:?}"
                    ))
                })?;
                self.process_control_queue().map_err(|e| {
                    EpollHelperError::HandleEvent(anyhow!(
                        "Failed to process control queue: {e:?}"
                    ))
                })?;
            }
            CURSOR_QUEUE_EVENT => {
                self.cursor_queue_evt.read().map_err(|e| {
                    EpollHelperError::HandleEvent(anyhow!(
                        "Failed to get cursor queue event: {e:?}"
                    ))
                })?;
                self.process_cursor_queue().map_err(|e| {
                    EpollHelperError::HandleEvent(anyhow!(
                        "Failed to process cursor queue: {e:?}"
                    ))
                })?;
            }
            _ => {
                return Err(EpollHelperError::HandleEvent(anyhow!(
                    "Unknown event for virtio-gpu"
                )));
            }
        }

        Ok(())
    }
}

impl VirtioDevice for Gpu {
    fn device_type(&self) -> u32 {
        self.common.device_type
    }

    fn queue_max_sizes(&self) -> &[u16] {
        &self.common.queue_sizes
    }

    fn features(&self) -> u64 {
        self.common.avail_features
    }

    fn ack_features(&mut self, value: u64) {
        self.common.ack_features(value)
    }

    fn read_config(&self, offset: u64, data: &mut [u8]) {
        let config = self.config.lock().expect("Failed to lock config mutex: another thread panicked while holding the lock");
        self.read_config_from_slice(config.as_slice(), offset, data);
    }

    fn write_config(&mut self, offset: u64, data: &[u8]) {
        // GPU config is mostly read-only for now
        let mut config = self.config.lock().expect("Failed to lock config mutex: another thread panicked while holding the lock");
        if offset + data.len() as u64 <= std::mem::size_of::<GpuConfig>() as u64 {
            let start = offset as usize;
            let end = start + data.len();
            config.as_mut_slice()[start..end].copy_from_slice(data);
        }
    }

    fn activate(
        &mut self,
        mem: GuestMemoryAtomic<GuestMemoryMmap>,
        interrupt_cb: Arc<dyn VirtioInterrupt>,
        mut queues: Vec<(usize, Queue, EventFd)>,
    ) -> ActivateResult {
        self.common.activate(&queues, interrupt_cb.clone())?;
        let (kill_evt, pause_evt) = self.common.dup_eventfds();

        let mut virtqueues = Vec::new();
        let (_, queue, queue_evt) = queues.remove(0);
        virtqueues.push(queue);
        let control_queue_evt = queue_evt;
        let (_, queue, queue_evt) = queues.remove(0);
        virtqueues.push(queue);
        let cursor_queue_evt = queue_evt;

        self.interrupt_cb = Some(interrupt_cb.clone());

        let mut handler = GpuEpollHandler {
            mem,
            queues: virtqueues,
            interrupt_cb,
            control_queue_evt,
            cursor_queue_evt,
            kill_evt,
            pause_evt,
            resources: self.resources.clone(),
            display_width: self.display_width,
            display_height: self.display_height,
        };

        let paused = self.common.paused.clone();
        let paused_sync = self.common.paused_sync.clone();
        let mut epoll_threads = Vec::new();

        spawn_virtio_thread(
            "virtio-gpu",
            &self.seccomp_action,
            Thread::VirtioGpu,
            &mut epoll_threads,
            &self.exit_evt,
            move || handler.run(&paused, paused_sync.as_ref().expect("paused_sync should be initialized during Gpu::new")),
        )?;
        self.common.epoll_threads = Some(epoll_threads);

        Ok(())
    }

    fn reset(&mut self) -> Option<Arc<dyn VirtioInterrupt>> {
        self.common.reset()
    }
}

impl Pausable for Gpu {}
impl Snapshottable for Gpu {
    fn id(&self) -> String {
        "virtio-gpu".to_string()
    }
}
impl Transportable for Gpu {}
impl Migratable for Gpu {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gpu_creation() {
        let exit_evt = EventFd::new(libc::EFD_NONBLOCK).unwrap();
        let gpu = Gpu::new(1024, 768, SeccompAction::Allow, exit_evt).unwrap();
        assert_eq!(gpu.device_type(), VirtioDeviceType::Gpu as u32);
        assert_eq!(gpu.display_dimensions(), (1024, 768));
    }

    #[test]
    fn test_gpu_config() {
        let config = GpuConfig::default();
        assert_eq!(config.num_scanouts, 1);
    }

    #[test]
    fn test_resource_creation() {
        let resource = Resource2D::new(800, 600, VIRTIO_GPU_FORMAT_B8G8R8A8_UNORM).unwrap();
        assert_eq!(resource.width, 800);
        assert_eq!(resource.height, 600);
        assert_eq!(resource.format, VIRTIO_GPU_FORMAT_B8G8R8A8_UNORM);
        assert_eq!(resource.data.len(), 800 * 600 * 4);
    }

    #[test]
    fn test_resource_invalid_format() {
        let result = Resource2D::new(800, 600, 999);
        assert!(result.is_err());
    }

    #[test]
    fn test_ctrl_header() {
        let header = CtrlHeader {
            hdr_type: VIRTIO_GPU_RESP_OK_NODATA,
            flags: 0,
            fence_id: 0,
            ctx_id: 0,
            padding: 0,
        };
        assert_eq!(header.hdr_type, VIRTIO_GPU_RESP_OK_NODATA);
    }

    #[test]
    fn test_create_response_header() {
        let header = GpuEpollHandler::create_response_header(VIRTIO_GPU_RESP_OK_NODATA);
        assert_eq!(header.hdr_type, VIRTIO_GPU_RESP_OK_NODATA);
        assert_eq!(header.flags, 0);
        assert_eq!(header.fence_id, 0);
    }

    #[test]
    fn test_rect() {
        let rect = Rect {
            x: 10,
            y: 20,
            width: 100,
            height: 200,
        };
        assert_eq!(rect.x, 10);
        assert_eq!(rect.y, 20);
        assert_eq!(rect.width, 100);
        assert_eq!(rect.height, 200);
    }

    #[test]
    fn test_display_one() {
        let display = DisplayOne {
            r: Rect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            enabled: 1,
            flags: 0,
        };
        assert!(display.enabled == 1);
    }

    #[test]
    fn test_resource_backing() {
        let mut resource = Resource2D::new(100, 100, VIRTIO_GPU_FORMAT_B8G8R8A8_UNORM).unwrap();

        // Add backing storage
        let entry = MemEntry {
            addr: 0x1000,
            length: 100 * 100 * 4,
            padding: 0,
        };
        resource.backing.push(entry);

        assert_eq!(resource.backing.len(), 1);
        assert_eq!(resource.backing[0].addr, 0x1000);
    }

    #[test]
    fn test_all_pixel_formats() {
        // Test all supported pixel formats
        let formats = [
            VIRTIO_GPU_FORMAT_B8G8R8A8_UNORM,
            VIRTIO_GPU_FORMAT_B8G8R8X8_UNORM,
            VIRTIO_GPU_FORMAT_A8R8G8B8_UNORM,
            VIRTIO_GPU_FORMAT_X8R8G8B8_UNORM,
            VIRTIO_GPU_FORMAT_R8G8B8A8_UNORM,
            VIRTIO_GPU_FORMAT_X8B8G8R8_UNORM,
            VIRTIO_GPU_FORMAT_A8B8G8R8_UNORM,
        ];

        for &format in &formats {
            let resource = Resource2D::new(64, 64, format);
            assert!(resource.is_ok(), "Failed to create resource for format {}", format);
            let resource = resource.unwrap();
            assert_eq!(resource.data.len(), 64 * 64 * 4);
        }
    }

    #[test]
    fn test_gpu_features() {
        let exit_evt = EventFd::new(libc::EFD_NONBLOCK).unwrap();
        let gpu = Gpu::new(1024, 768, SeccompAction::Allow, exit_evt).unwrap();

        // Should have EDID feature
        assert!(gpu.features() & (1u64 << VIRTIO_GPU_F_EDID) != 0);
    }

    #[test]
    fn test_gpu_ack_features() {
        let exit_evt = EventFd::new(libc::EFD_NONBLOCK).unwrap();
        let mut gpu = Gpu::new(1024, 768, SeccompAction::Allow, exit_evt).unwrap();

        // Ack features should not panic
        gpu.ack_features(0);
        gpu.ack_features(1u64 << VIRTIO_GPU_F_EDID);
    }
}
