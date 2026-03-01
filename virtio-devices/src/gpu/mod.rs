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

/// VIRGL 3D feature bit
pub const VIRTIO_GPU_F_VIRGL: u64 = 2;

// VirtIO GPU commands
const VIRTIO_GPU_CMD_GET_DISPLAY_INFO: u32 = 0x0100;
const VIRTIO_GPU_CMD_RESOURCE_CREATE_2D: u32 = 0x0101;
const VIRTIO_GPU_CMD_RESOURCE_UNREF: u32 = 0x0102;
const VIRTIO_GPU_CMD_SET_SCANOUT: u32 = 0x0103;
const VIRTIO_GPU_CMD_RESOURCE_FLUSH: u32 = 0x0104;
const VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D: u32 = 0x0105;
const VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING: u32 = 0x0106;
const VIRTIO_GPU_CMD_RESOURCE_DETACH_BACKING: u32 = 0x0107;

// VIRGL 3D commands
const VIRTIO_GPU_CMD_CTX_CREATE: u32 = 0x0200;
const VIRTIO_GPU_CMD_CTX_DESTROY: u32 = 0x0201;
const VIRTIO_GPU_CMD_CTX_ATTACH_RESOURCE: u32 = 0x0202;
const VIRTIO_GPU_CMD_CTX_DETACH_RESOURCE: u32 = 0x0203;
const VIRTIO_GPU_CMD_RESOURCE_CREATE_3D: u32 = 0x0204;
const VIRTIO_GPU_CMD_TRANSFER_TO_HOST_3D: u32 = 0x0205;
const VIRTIO_GPU_CMD_TRANSFER_FROM_HOST_3D: u32 = 0x0206;
const VIRTIO_GPU_CMD_SUBMIT_3D: u32 = 0x0207;

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

/// Box for 3D transfers
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct Box3D {
    x: u32,
    y: u32,
    z: u32,
    w: u32,
    h: u32,
    d: u32,
}

// SAFETY: Box3D is POD and has no implicit padding
unsafe impl ByteValued for Box3D {}

/// Context create command
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct CtxCreate {
    header: CtrlHeader,
    nctx: u32,  // Context name length
    context_name: [u8; 64],
}

// SAFETY: CtxCreate is POD
unsafe impl ByteValued for CtxCreate {}

/// Context destroy command
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct CtxDestroy {
    header: CtrlHeader,
    padding: u32,
}

// SAFETY: CtxDestroy is POD
unsafe impl ByteValued for CtxDestroy {}

/// Context attach resource command
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct CtxAttachResource {
    header: CtrlHeader,
    resource_id: u32,
    padding: u32,
}

// SAFETY: CtxAttachResource is POD
unsafe impl ByteValued for CtxAttachResource {}

/// Context detach resource command
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct CtxDetachResource {
    header: CtrlHeader,
    resource_id: u32,
    padding: u32,
}

// SAFETY: CtxDetachResource is POD
unsafe impl ByteValued for CtxDetachResource {}

/// Resource create 3D command
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct ResourceCreate3D {
    header: CtrlHeader,
    resource_id: u32,
    format: u32,
    width: u32,
    height: u32,
    depth: u32,
    target: u32,
    bind: u32,
    nr_samples: u32,
    flags: u32,
    padding: u32,
}

// SAFETY: ResourceCreate3D is POD
unsafe impl ByteValued for ResourceCreate3D {}

/// Transfer 3D command
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct Transfer3D {
    header: CtrlHeader,
    resource_id: u32,
    level: u32,
    stride: u32,
    layer_stride: u32,
    box_: Box3D,
    offset: u64,
}

// SAFETY: Transfer3D is POD
unsafe impl ByteValued for Transfer3D {}

/// Submit 3D command header
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct Submit3D {
    header: CtrlHeader,
    size: u32,  // Size of command buffer in bytes
}

// SAFETY: Submit3D is POD
unsafe impl ByteValued for Submit3D {}

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

/// VIRGL 3D context
#[derive(Debug, Clone)]
struct VirglContext {
    /// Context ID
    id: u32,
    /// Context name
    name: String,
    /// Attached resources
    resources: Vec<u32>,
}

impl VirglContext {
    fn new(id: u32, name: String) -> Self {
        Self {
            id,
            name,
            resources: Vec::new(),
        }
    }
}

/// 3D resource (VIRGL)
#[derive(Debug, Clone)]
struct Resource3D {
    /// Resource ID
    id: u32,
    /// Width
    width: u32,
    /// Height
    height: u32,
    /// Depth
    depth: u32,
    /// Format
    format: u32,
    /// Target (TEXTURE_2D, etc.)
    target: u32,
    /// Bind flags
    bind: u32,
    /// Number of samples
    nr_samples: u32,
    /// Flags
    flags: u32,
    /// Data buffer
    data: Vec<u8>,
}

impl Resource3D {
    fn new(
        id: u32,
        width: u32,
        height: u32,
        depth: u32,
        format: u32,
        target: u32,
        bind: u32,
        nr_samples: u32,
        flags: u32,
    ) -> Self {
        // Simplified size calculation - assumes 4 bytes per pixel
        let size = (width as usize) * (height as usize) * (depth as usize) * 4;
        Self {
            id,
            width,
            height,
            depth,
            format,
            target,
            bind,
            nr_samples,
            flags,
            data: vec![0u8; size],
        }
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
    /// VIRGL contexts
    virgl_contexts: Arc<Mutex<HashMap<u32, VirglContext>>>,
    /// 3D resources
    resources_3d: Arc<Mutex<HashMap<u32, Resource3D>>>,
    /// VIRGL feature enabled
    virgl_enabled: bool,
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
        Self::new_with_virgl(display_width, display_height, true, seccomp_action, exit_evt)
    }

    /// Create a new VirtIO GPU device with VIRGL option
    pub fn new_with_virgl(
        display_width: u32,
        display_height: u32,
        virgl_enabled: bool,
        seccomp_action: SeccompAction,
        exit_evt: EventFd,
    ) -> io::Result<Self> {
        let mut avail_features = 1u64 << VIRTIO_GPU_F_EDID;
        if virgl_enabled {
            avail_features |= 1u64 << VIRTIO_GPU_F_VIRGL;
        }

        Ok(Self {
            common: VirtioCommon {
                device_type: VirtioDeviceType::Gpu as u32,
                queue_sizes: QUEUE_SIZES.to_vec(),
                avail_features,
                paused_sync: Some(Arc::new(Barrier::new(2))),
                min_queues: NUM_QUEUES as u16,
                ..Default::default()
            },
            config: Arc::new(Mutex::new(GpuConfig::default())),
            display_width,
            display_height,
            resources: Arc::new(Mutex::new(HashMap::new())),
            virgl_contexts: Arc::new(Mutex::new(HashMap::new())),
            resources_3d: Arc::new(Mutex::new(HashMap::new())),
            virgl_enabled,
            seccomp_action,
            exit_evt,
            interrupt_cb: None,
        })
    }

    /// Get display dimensions
    pub fn display_dimensions(&self) -> (u32, u32) {
        (self.display_width, self.display_height)
    }

    /// Check if VIRGL is enabled
    pub fn is_virgl_enabled(&self) -> bool {
        self.virgl_enabled
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
    virgl_contexts: Arc<Mutex<HashMap<u32, VirglContext>>>,
    resources_3d: Arc<Mutex<HashMap<u32, Resource3D>>>,
    display_width: u32,
    display_height: u32,
    virgl_enabled: bool,
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

    // ============== VIRGL 3D Command Handlers ==============

    /// Handle VIRGL context create
    fn handle_ctx_create(&mut self, cmd: &CtxCreate) -> CtrlHeader {
        if !self.virgl_enabled {
            return Self::create_response_header(VIRTIO_GPU_RESP_ERR_UNSPEC);
        }

        // Extract context name from the fixed-size array
        let name_len = std::cmp::min(cmd.nctx as usize, cmd.context_name.len());
        let name = String::from_utf8_lossy(&cmd.context_name[..name_len]).into_owned();

        let ctx_id = cmd.header.ctx_id;
        let context = VirglContext::new(ctx_id, name);

        let mut contexts = self.virgl_contexts.lock().expect("Failed to lock virgl_contexts mutex: another thread panicked while holding the lock");
        contexts.insert(ctx_id, context);

        Self::create_response_header(VIRTIO_GPU_RESP_OK_NODATA)
    }

    /// Handle VIRGL context destroy
    fn handle_ctx_destroy(&mut self, cmd: &CtxDestroy) -> CtrlHeader {
        if !self.virgl_enabled {
            return Self::create_response_header(VIRTIO_GPU_RESP_ERR_UNSPEC);
        }

        let ctx_id = cmd.header.ctx_id;
        let mut contexts = self.virgl_contexts.lock().expect("Failed to lock virgl_contexts mutex: another thread panicked while holding the lock");
        contexts.remove(&ctx_id);

        Self::create_response_header(VIRTIO_GPU_RESP_OK_NODATA)
    }

    /// Handle VIRGL context attach resource
    fn handle_ctx_attach_resource(&mut self, cmd: &CtxAttachResource) -> CtrlHeader {
        if !self.virgl_enabled {
            return Self::create_response_header(VIRTIO_GPU_RESP_ERR_UNSPEC);
        }

        if cmd.resource_id == 0 {
            return Self::create_response_header(VIRTIO_GPU_RESP_ERR_INVALID_RESOURCE_ID);
        }

        let ctx_id = cmd.header.ctx_id;
        let mut contexts = self.virgl_contexts.lock().expect("Failed to lock virgl_contexts mutex: another thread panicked while holding the lock");

        if let Some(context) = contexts.get_mut(&ctx_id) {
            if !context.resources.contains(&cmd.resource_id) {
                context.resources.push(cmd.resource_id);
            }
            Self::create_response_header(VIRTIO_GPU_RESP_OK_NODATA)
        } else {
            Self::create_response_header(VIRTIO_GPU_RESP_ERR_UNSPEC)
        }
    }

    /// Handle VIRGL context detach resource
    fn handle_ctx_detach_resource(&mut self, cmd: &CtxDetachResource) -> CtrlHeader {
        if !self.virgl_enabled {
            return Self::create_response_header(VIRTIO_GPU_RESP_ERR_UNSPEC);
        }

        let ctx_id = cmd.header.ctx_id;
        let mut contexts = self.virgl_contexts.lock().expect("Failed to lock virgl_contexts mutex: another thread panicked while holding the lock");

        if let Some(context) = contexts.get_mut(&ctx_id) {
            context.resources.retain(|&id| id != cmd.resource_id);
            Self::create_response_header(VIRTIO_GPU_RESP_OK_NODATA)
        } else {
            Self::create_response_header(VIRTIO_GPU_RESP_ERR_UNSPEC)
        }
    }

    /// Handle resource create 3D
    fn handle_resource_create_3d(&mut self, cmd: &ResourceCreate3D) -> CtrlHeader {
        if !self.virgl_enabled {
            return Self::create_response_header(VIRTIO_GPU_RESP_ERR_UNSPEC);
        }

        if cmd.resource_id == 0 {
            return Self::create_response_header(VIRTIO_GPU_RESP_ERR_INVALID_RESOURCE_ID);
        }

        let resource = Resource3D::new(
            cmd.resource_id,
            cmd.width,
            cmd.height,
            cmd.depth,
            cmd.format,
            cmd.target,
            cmd.bind,
            cmd.nr_samples,
            cmd.flags,
        );

        let mut resources_3d = self.resources_3d.lock().expect("Failed to lock resources_3d mutex: another thread panicked while holding the lock");
        resources_3d.insert(cmd.resource_id, resource);

        Self::create_response_header(VIRTIO_GPU_RESP_OK_NODATA)
    }

    /// Handle transfer to host 3D
    fn handle_transfer_to_host_3d<M: GuestMemory>(
        &mut self,
        mem: &M,
        cmd: &Transfer3D,
    ) -> CtrlHeader {
        if !self.virgl_enabled {
            return Self::create_response_header(VIRTIO_GPU_RESP_ERR_UNSPEC);
        }

        if cmd.resource_id == 0 {
            return Self::create_response_header(VIRTIO_GPU_RESP_ERR_INVALID_RESOURCE_ID);
        }

        let mut resources_3d = self.resources_3d.lock().expect("Failed to lock resources_3d mutex: another thread panicked while holding the lock");

        if let Some(resource) = resources_3d.get_mut(&cmd.resource_id) {
            // Calculate destination offset based on box coordinates
            let dst_start = (cmd.box_.z as usize * resource.width as usize * resource.height as usize
                + cmd.box_.y as usize * resource.width as usize
                + cmd.box_.x as usize) * 4;

            let bytes_to_copy = std::cmp::min(
                (cmd.box_.w * cmd.box_.h * cmd.box_.d * 4) as usize,
                resource.data.len().saturating_sub(dst_start),
            );

            if bytes_to_copy > 0 && dst_start + bytes_to_copy <= resource.data.len() {
                let src_addr = GuestAddress(cmd.offset);
                if mem
                    .read_slice(&mut resource.data[dst_start..dst_start + bytes_to_copy], src_addr)
                    .is_err()
                {
                    error!("Failed to read from guest memory during TRANSFER_TO_HOST_3D");
                }
            }

            Self::create_response_header(VIRTIO_GPU_RESP_OK_NODATA)
        } else {
            Self::create_response_header(VIRTIO_GPU_RESP_ERR_INVALID_RESOURCE_ID)
        }
    }

    /// Handle transfer from host 3D
    fn handle_transfer_from_host_3d<M: GuestMemory>(
        &mut self,
        mem: &M,
        cmd: &Transfer3D,
    ) -> CtrlHeader {
        if !self.virgl_enabled {
            return Self::create_response_header(VIRTIO_GPU_RESP_ERR_UNSPEC);
        }

        if cmd.resource_id == 0 {
            return Self::create_response_header(VIRTIO_GPU_RESP_ERR_INVALID_RESOURCE_ID);
        }

        let resources_3d = self.resources_3d.lock().expect("Failed to lock resources_3d mutex: another thread panicked while holding the lock");

        if let Some(resource) = resources_3d.get(&cmd.resource_id) {
            // Calculate source offset based on box coordinates
            let src_start = (cmd.box_.z as usize * resource.width as usize * resource.height as usize
                + cmd.box_.y as usize * resource.width as usize
                + cmd.box_.x as usize) * 4;

            let bytes_to_copy = std::cmp::min(
                (cmd.box_.w * cmd.box_.h * cmd.box_.d * 4) as usize,
                resource.data.len().saturating_sub(src_start),
            );

            if bytes_to_copy > 0 && src_start + bytes_to_copy <= resource.data.len() {
                let dst_addr = GuestAddress(cmd.offset);
                if mem
                    .write_slice(&resource.data[src_start..src_start + bytes_to_copy], dst_addr)
                    .is_err()
                {
                    error!("Failed to write to guest memory during TRANSFER_FROM_HOST_3D");
                }
            }

            Self::create_response_header(VIRTIO_GPU_RESP_OK_NODATA)
        } else {
            Self::create_response_header(VIRTIO_GPU_RESP_ERR_INVALID_RESOURCE_ID)
        }
    }

    /// Handle submit 3D (Gallium command buffer)
    /// This is a simplified implementation that just acknowledges the command.
    /// A full implementation would parse and execute Gallium commands.
    fn handle_submit_3d(&mut self, ctx_id: u32, _cmd_data: &[u32]) -> CtrlHeader {
        if !self.virgl_enabled {
            return Self::create_response_header(VIRTIO_GPU_RESP_ERR_UNSPEC);
        }

        let contexts = self.virgl_contexts.lock().expect("Failed to lock virgl_contexts mutex: another thread panicked while holding the lock");

        if contexts.contains_key(&ctx_id) {
            // In a full implementation, this would parse and execute Gallium commands
            // For software rendering, this would be a simplified interpreter
            // For now, we just acknowledge the submission
            Self::create_response_header(VIRTIO_GPU_RESP_OK_NODATA)
        } else {
            Self::create_response_header(VIRTIO_GPU_RESP_ERR_UNSPEC)
        }
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
                // VIRGL 3D commands
                VIRTIO_GPU_CMD_CTX_CREATE => {
                    let cmd: CtxCreate = desc_chain
                        .memory()
                        .read_obj(head_desc.addr())
                        .map_err(Error::GuestMemory)?;
                    self.handle_ctx_create(&cmd)
                }
                VIRTIO_GPU_CMD_CTX_DESTROY => {
                    let cmd: CtxDestroy = desc_chain
                        .memory()
                        .read_obj(head_desc.addr())
                        .map_err(Error::GuestMemory)?;
                    self.handle_ctx_destroy(&cmd)
                }
                VIRTIO_GPU_CMD_CTX_ATTACH_RESOURCE => {
                    let cmd: CtxAttachResource = desc_chain
                        .memory()
                        .read_obj(head_desc.addr())
                        .map_err(Error::GuestMemory)?;
                    self.handle_ctx_attach_resource(&cmd)
                }
                VIRTIO_GPU_CMD_CTX_DETACH_RESOURCE => {
                    let cmd: CtxDetachResource = desc_chain
                        .memory()
                        .read_obj(head_desc.addr())
                        .map_err(Error::GuestMemory)?;
                    self.handle_ctx_detach_resource(&cmd)
                }
                VIRTIO_GPU_CMD_RESOURCE_CREATE_3D => {
                    let cmd: ResourceCreate3D = desc_chain
                        .memory()
                        .read_obj(head_desc.addr())
                        .map_err(Error::GuestMemory)?;
                    self.handle_resource_create_3d(&cmd)
                }
                VIRTIO_GPU_CMD_TRANSFER_TO_HOST_3D => {
                    let cmd: Transfer3D = desc_chain
                        .memory()
                        .read_obj(head_desc.addr())
                        .map_err(Error::GuestMemory)?;
                    self.handle_transfer_to_host_3d(desc_chain.memory(), &cmd)
                }
                VIRTIO_GPU_CMD_TRANSFER_FROM_HOST_3D => {
                    let cmd: Transfer3D = desc_chain
                        .memory()
                        .read_obj(head_desc.addr())
                        .map_err(Error::GuestMemory)?;
                    self.handle_transfer_from_host_3d(desc_chain.memory(), &cmd)
                }
                VIRTIO_GPU_CMD_SUBMIT_3D => {
                    let cmd: Submit3D = desc_chain
                        .memory()
                        .read_obj(head_desc.addr())
                        .map_err(Error::GuestMemory)?;

                    // Read command buffer from next descriptor
                    let mut cmd_data = Vec::new();
                    if let Some(cmd_desc) = desc_chain.next() {
                        if !cmd_desc.is_write_only() {
                            let num_words = cmd.size as usize / 4;
                            cmd_data.reserve(num_words);
                            for i in 0..num_words {
                                let offset = i * 4;
                                if let Some(addr) = cmd_desc.addr().checked_add(offset as u64) {
                                    if let Ok(word) = desc_chain.memory().read_obj::<u32>(addr) {
                                        cmd_data.push(word);
                                    }
                                }
                            }
                        }
                    }
                    self.handle_submit_3d(header.ctx_id, &cmd_data)
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
            virgl_contexts: self.virgl_contexts.clone(),
            resources_3d: self.resources_3d.clone(),
            display_width: self.display_width,
            display_height: self.display_height,
            virgl_enabled: self.virgl_enabled,
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
        // Should have VIRGL feature enabled by default
        assert!(gpu.features() & (1u64 << VIRTIO_GPU_F_VIRGL) != 0);
    }

    #[test]
    fn test_gpu_ack_features() {
        let exit_evt = EventFd::new(libc::EFD_NONBLOCK).unwrap();
        let mut gpu = Gpu::new(1024, 768, SeccompAction::Allow, exit_evt).unwrap();

        // Ack features should not panic
        gpu.ack_features(0);
        gpu.ack_features(1u64 << VIRTIO_GPU_F_EDID);
        gpu.ack_features(1u64 << VIRTIO_GPU_F_VIRGL);
    }

    #[test]
    fn test_gpu_virgl_disabled() {
        let exit_evt = EventFd::new(libc::EFD_NONBLOCK).unwrap();
        let gpu = Gpu::new_with_virgl(1024, 768, false, SeccompAction::Allow, exit_evt).unwrap();

        // Should have EDID feature
        assert!(gpu.features() & (1u64 << VIRTIO_GPU_F_EDID) != 0);
        // Should NOT have VIRGL feature
        assert!(gpu.features() & (1u64 << VIRTIO_GPU_F_VIRGL) == 0);
        assert!(!gpu.is_virgl_enabled());
    }

    #[test]
    fn test_gpu_virgl_enabled() {
        let exit_evt = EventFd::new(libc::EFD_NONBLOCK).unwrap();
        let gpu = Gpu::new_with_virgl(1024, 768, true, SeccompAction::Allow, exit_evt).unwrap();

        // Should have VIRGL feature
        assert!(gpu.features() & (1u64 << VIRTIO_GPU_F_VIRGL) != 0);
        assert!(gpu.is_virgl_enabled());
    }

    // ============== VIRGL 3D Tests ==============

    #[test]
    fn test_virgl_context_creation() {
        let context = VirglContext::new(1, "test_context".to_string());
        assert_eq!(context.id, 1);
        assert_eq!(context.name, "test_context");
        assert!(context.resources.is_empty());
    }

    #[test]
    fn test_resource_3d_creation() {
        let resource = Resource3D::new(
            1,      // id
            256,    // width
            256,    // height
            1,      // depth
            67,     // format (R8G8B8A8)
            2,      // target (TEXTURE_2D)
            1,      // bind
            0,      // nr_samples
            0,      // flags
        );
        assert_eq!(resource.id, 1);
        assert_eq!(resource.width, 256);
        assert_eq!(resource.height, 256);
        assert_eq!(resource.depth, 1);
        assert_eq!(resource.data.len(), 256 * 256 * 1 * 4);
    }

    #[test]
    fn test_box_3d() {
        let box3d = Box3D {
            x: 0,
            y: 0,
            z: 0,
            w: 100,
            h: 100,
            d: 1,
        };
        assert_eq!(box3d.x, 0);
        assert_eq!(box3d.w, 100);
        assert_eq!(box3d.h, 100);
        assert_eq!(box3d.d, 1);
    }

    #[test]
    fn test_ctx_create_struct() {
        let header = CtrlHeader {
            hdr_type: VIRTIO_GPU_CMD_CTX_CREATE,
            flags: 0,
            fence_id: 0,
            ctx_id: 1,
            padding: 0,
        };
        let mut name = [0u8; 64];
        let name_str = "test";
        name[..name_str.len()].copy_from_slice(name_str.as_bytes());

        let cmd = CtxCreate {
            header,
            nctx: name_str.len() as u32,
            context_name: name,
        };
        assert_eq!(cmd.header.ctx_id, 1);
        assert_eq!(cmd.nctx, 4);
    }

    #[test]
    fn test_resource_create_3d_struct() {
        let header = CtrlHeader {
            hdr_type: VIRTIO_GPU_CMD_RESOURCE_CREATE_3D,
            flags: 0,
            fence_id: 0,
            ctx_id: 0,
            padding: 0,
        };

        let cmd = ResourceCreate3D {
            header,
            resource_id: 1,
            format: 67,
            width: 512,
            height: 512,
            depth: 1,
            target: 2,
            bind: 1,
            nr_samples: 0,
            flags: 0,
            padding: 0,
        };
        assert_eq!(cmd.resource_id, 1);
        assert_eq!(cmd.width, 512);
        assert_eq!(cmd.height, 512);
    }

    #[test]
    fn test_transfer_3d_struct() {
        let header = CtrlHeader {
            hdr_type: VIRTIO_GPU_CMD_TRANSFER_TO_HOST_3D,
            flags: 0,
            fence_id: 0,
            ctx_id: 0,
            padding: 0,
        };

        let box3d = Box3D {
            x: 0,
            y: 0,
            z: 0,
            w: 64,
            h: 64,
            d: 1,
        };

        let cmd = Transfer3D {
            header,
            resource_id: 1,
            level: 0,
            stride: 64 * 4,
            layer_stride: 64 * 64 * 4,
            box_: box3d,
            offset: 0,
        };
        assert_eq!(cmd.resource_id, 1);
        assert_eq!(cmd.box_.w, 64);
        assert_eq!(cmd.box_.h, 64);
    }
}
