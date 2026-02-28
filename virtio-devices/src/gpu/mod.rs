// Copyright 2024 Cloud Hypervisor Authors. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! VirtIO GPU Device Stub
//!
//! This module provides a stub VirtIO GPU device for future implementation.

use std::io;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use virtio_queue::Queue;
use vm_memory::{ByteValued, GuestMemoryAtomic};
use vm_migration::{Migratable, Pausable, Snapshottable, Transportable};
use vmm_sys_util::eventfd::EventFd;

use super::{ActivateResult, VirtioCommon, VirtioDevice, VirtioDeviceType, VirtioInterrupt};
use crate::GuestMemoryMmap;

/// Queue sizes
const QUEUE_SIZE: u16 = 256;
const NUM_QUEUES: usize = 2;
const QUEUE_SIZES: &[u16] = &[QUEUE_SIZE; NUM_QUEUES];

/// EDID feature bit
pub const VIRTIO_GPU_F_EDID: u64 = 1;

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

/// VirtIO GPU device (stub implementation)
pub struct Gpu {
    /// Common virtio device data
    common: VirtioCommon,
    /// GPU configuration
    config: Arc<Mutex<GpuConfig>>,
    /// Display width
    display_width: u32,
    /// Display height
    display_height: u32,
    /// Interrupt callback
    interrupt_cb: Option<Arc<dyn VirtioInterrupt>>,
}

impl Gpu {
    /// Create a new VirtIO GPU device
    pub fn new(display_width: u32, display_height: u32) -> io::Result<Self> {
        Ok(Self {
            common: VirtioCommon {
                device_type: VirtioDeviceType::Gpu as u32,
                queue_sizes: QUEUE_SIZES.to_vec(),
                avail_features: 1u64 << VIRTIO_GPU_F_EDID,
                paused_sync: Some(Arc::new(std::sync::Barrier::new(2))),
                min_queues: NUM_QUEUES as u16,
                ..Default::default()
            },
            config: Arc::new(Mutex::new(GpuConfig::default())),
            display_width,
            display_height,
            interrupt_cb: None,
        })
    }

    /// Get display dimensions
    pub fn display_dimensions(&self) -> (u32, u32) {
        (self.display_width, self.display_height)
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
        let config = self.config.lock().unwrap();
        self.read_config_from_slice(config.as_slice(), offset, data);
    }

    fn write_config(&mut self, offset: u64, data: &[u8]) {
        // GPU config is mostly read-only for now
        let mut config = self.config.lock().unwrap();
        if offset + data.len() as u64 <= std::mem::size_of::<GpuConfig>() as u64 {
            let start = offset as usize;
            let end = start + data.len();
            config.as_mut_slice()[start..end].copy_from_slice(data);
        }
    }

    fn activate(
        &mut self,
        _mem: GuestMemoryAtomic<GuestMemoryMmap>,
        interrupt_cb: Arc<dyn VirtioInterrupt>,
        queues: Vec<(usize, Queue, EventFd)>,
    ) -> ActivateResult {
        self.common.activate(&queues, interrupt_cb.clone())?;
        self.interrupt_cb = Some(interrupt_cb);
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
        let gpu = Gpu::new(1024, 768).unwrap();
        assert_eq!(gpu.device_type(), VirtioDeviceType::Gpu as u32);
        assert_eq!(gpu.display_dimensions(), (1024, 768));
    }

    #[test]
    fn test_gpu_config() {
        let config = GpuConfig::default();
        assert_eq!(config.num_scanouts, 1);
    }
}
