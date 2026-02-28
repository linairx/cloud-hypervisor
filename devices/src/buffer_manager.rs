// Copyright 2024 Cloud Hypervisor Authors. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Buffer Manager
//!
//! Provides dynamic buffer management for IVSHMEM frame buffers.
//! Supports configurable buffer count, state monitoring, and allocation.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::frame_buffer::{
    FrameBufferHeader, FrameBufferLayout, FrameMetadata, FrameFlags,
    DEFAULT_BUFFER_COUNT, MAX_CURSOR_SIZE,
};

/// Minimum buffer count
pub const MIN_BUFFER_COUNT: u32 = 2;

/// Maximum buffer count
pub const MAX_BUFFER_COUNT: u32 = 16;

/// Default buffer count for different use cases
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferProfile {
    /// Low latency (2 buffers)
    LowLatency,
    /// Balanced (3 buffers, default)
    Balanced,
    /// High throughput (4 buffers)
    HighThroughput,
    /// Maximum (8 buffers)
    Maximum,
}

impl Default for BufferProfile {
    fn default() -> Self {
        BufferProfile::Balanced
    }
}

impl BufferProfile {
    /// Get buffer count for this profile
    pub fn buffer_count(&self) -> u32 {
        match self {
            BufferProfile::LowLatency => 2,
            BufferProfile::Balanced => 3,
            BufferProfile::HighThroughput => 4,
            BufferProfile::Maximum => 8,
        }
    }
}

/// Buffer state for monitoring
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferState {
    /// Buffer is free and available
    Free,
    /// Buffer is being written to
    Writing,
    /// Buffer contains valid data and is ready to read
    Ready,
    /// Buffer is being read
    Reading,
}

/// Individual buffer information
#[derive(Debug, Clone)]
pub struct BufferInfo {
    /// Buffer index
    pub index: u32,
    /// Buffer state
    pub state: BufferState,
    /// Frame number in this buffer
    pub frame_number: u64,
    /// Data size in bytes
    pub data_size: u32,
    /// Timestamp (nanoseconds)
    pub timestamp_ns: u64,
    /// Is keyframe
    pub is_keyframe: bool,
}

/// Buffer manager statistics
#[derive(Debug, Clone, Default)]
pub struct BufferStats {
    /// Total frames written
    pub frames_written: u64,
    /// Total frames read
    pub frames_read: u64,
    /// Buffer overflows (write too fast)
    pub overflows: u64,
    /// Buffer underflows (read too fast)
    pub underflows: u64,
    /// Current write index
    pub write_index: u32,
    /// Current read index
    pub read_index: u32,
    /// Average frame size
    pub avg_frame_size: f64,
}

/// Buffer manager configuration
#[derive(Debug, Clone)]
pub struct BufferConfig {
    /// Number of buffers
    pub buffer_count: u32,
    /// Size of each buffer in bytes
    pub buffer_size: u64,
    /// Enable overflow detection
    pub detect_overflow: bool,
    /// Enable statistics collection
    pub collect_stats: bool,
}

impl Default for BufferConfig {
    fn default() -> Self {
        Self {
            buffer_count: DEFAULT_BUFFER_COUNT,
            buffer_size: 1920 * 1080 * 4, // Default: 1080p BGRA
            detect_overflow: true,
            collect_stats: true,
        }
    }
}

impl BufferConfig {
    /// Create config from profile
    pub fn from_profile(profile: BufferProfile, buffer_size: u64) -> Self {
        Self {
            buffer_count: profile.buffer_count(),
            buffer_size,
            detect_overflow: true,
            collect_stats: true,
        }
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.buffer_count < MIN_BUFFER_COUNT {
            return Err(format!(
                "Buffer count {} is below minimum {}",
                self.buffer_count, MIN_BUFFER_COUNT
            ));
        }
        if self.buffer_count > MAX_BUFFER_COUNT {
            return Err(format!(
                "Buffer count {} exceeds maximum {}",
                self.buffer_count, MAX_BUFFER_COUNT
            ));
        }
        if self.buffer_size == 0 {
            return Err("Buffer size cannot be zero".to_string());
        }
        Ok(())
    }
}

/// Buffer manager for IVSHMEM frame buffers
pub struct BufferManager {
    /// Configuration
    config: BufferConfig,
    /// Layout calculator
    layout: FrameBufferLayout,
    /// Statistics
    stats: BufferStats,
    /// Frame count for averaging
    frame_count: u64,
    /// Total bytes written (for averaging)
    total_bytes: u64,
}

impl BufferManager {
    /// Create a new buffer manager
    pub fn new(config: BufferConfig) -> Result<Self, String> {
        config.validate()?;

        let layout = FrameBufferLayout::new(config.buffer_count, config.buffer_size);

        Ok(Self {
            config,
            layout,
            stats: BufferStats::default(),
            frame_count: 0,
            total_bytes: 0,
        })
    }

    /// Create with default configuration
    pub fn default_manager() -> Self {
        Self::new(BufferConfig::default()).expect("Default config should be valid")
    }

    /// Create from profile
    pub fn from_profile(profile: BufferProfile, buffer_size: u64) -> Self {
        Self::new(BufferConfig::from_profile(profile, buffer_size)).expect("Profile config should be valid")
    }

    /// Get configuration
    pub fn config(&self) -> &BufferConfig {
        &self.config
    }

    /// Get layout
    pub fn layout(&self) -> &FrameBufferLayout {
        &self.layout
    }

    /// Get buffer count
    pub fn buffer_count(&self) -> u32 {
        self.config.buffer_count
    }

    /// Get buffer size
    pub fn buffer_size(&self) -> u64 {
        self.config.buffer_size
    }

    /// Get total memory size required
    pub fn total_size(&self) -> usize {
        self.layout.total_size
    }

    /// Get next write index (lock-free)
    pub fn next_write_index(&self, current: u32) -> u32 {
        (current + 1) % self.config.buffer_count
    }

    /// Get next read index (lock-free)
    pub fn next_read_index(&self, current: u32) -> u32 {
        (current + 1) % self.config.buffer_count
    }

    /// Check if write would cause overflow
    pub fn would_overflow(&self, write_index: u32, read_index: u32) -> bool {
        if !self.config.detect_overflow {
            return false;
        }

        let next_write = self.next_write_index(write_index);
        // Overflow if next write would catch up to read
        next_write == read_index
    }

    /// Update statistics for a write
    pub fn record_write(&mut self, data_size: u32) {
        self.frame_count += 1;
        self.total_bytes += data_size as u64;
        self.stats.frames_written += 1;
        self.stats.avg_frame_size = self.total_bytes as f64 / self.frame_count as f64;
    }

    /// Update statistics for a read
    pub fn record_read(&mut self) {
        self.stats.frames_read += 1;
    }

    /// Record an overflow
    pub fn record_overflow(&mut self) {
        self.stats.overflows += 1;
    }

    /// Record an underflow
    pub fn record_underflow(&mut self) {
        self.stats.underflows += 1;
    }

    /// Get statistics
    pub fn stats(&self) -> &BufferStats {
        &self.stats
    }

    /// Reset statistics
    pub fn reset_stats(&mut self) {
        self.stats = BufferStats::default();
        self.frame_count = 0;
        self.total_bytes = 0;
    }

    /// Calculate memory layout for given parameters
    pub fn calculate_layout(buffer_count: u32, buffer_size: u64) -> FrameBufferLayout {
        FrameBufferLayout::new(buffer_count, buffer_size)
    }

    /// Get buffer info for monitoring
    pub fn get_buffer_info(&self, index: u32, metadata: &FrameMetadata) -> BufferInfo {
        BufferInfo {
            index,
            state: if metadata.is_initialized() {
                BufferState::Ready
            } else {
                BufferState::Free
            },
            frame_number: metadata.frame_number,
            data_size: metadata.data_size,
            timestamp_ns: metadata.timestamp_ns,
            is_keyframe: metadata.flags().contains(FrameFlags::KEYFRAME),
        }
    }

    /// Recommend buffer count based on frame rate and resolution
    pub fn recommend_buffer_count(fps: u32, resolution: (u32, u32)) -> u32 {
        // Higher FPS or resolution needs more buffers
        let pixels = resolution.0 * resolution.1;

        match (fps, pixels) {
            (0..=30, 0..=2_073_600) => 2,      // Up to 1080p @ 30fps
            (0..=30, _) => 3,                   // Above 1080p @ 30fps
            (31..=60, 0..=2_073_600) => 3,     // Up to 1080p @ 60fps
            (31..=60, _) => 4,                  // Above 1080p @ 60fps
            (61..=120, _) => 4,                 // High FPS
            _ => 6,                             // Very high FPS
        }
    }

    /// Calculate required buffer size for format
    pub fn calculate_buffer_size(width: u32, height: u32, bytes_per_pixel: u32) -> u64 {
        (width * height * bytes_per_pixel) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_buffer_config_validation() {
        let valid = BufferConfig::default();
        assert!(valid.validate().is_ok());

        let invalid_count = BufferConfig {
            buffer_count: 1,
            ..Default::default()
        };
        assert!(invalid_count.validate().is_err());

        let invalid_size = BufferConfig {
            buffer_size: 0,
            ..Default::default()
        };
        assert!(invalid_size.validate().is_err());
    }

    #[test]
    fn test_buffer_profiles() {
        assert_eq!(BufferProfile::LowLatency.buffer_count(), 2);
        assert_eq!(BufferProfile::Balanced.buffer_count(), 3);
        assert_eq!(BufferProfile::HighThroughput.buffer_count(), 4);
        assert_eq!(BufferProfile::Maximum.buffer_count(), 8);
    }

    #[test]
    fn test_buffer_manager_creation() {
        let manager = BufferManager::default_manager();
        assert_eq!(manager.buffer_count(), DEFAULT_BUFFER_COUNT);
    }

    #[test]
    fn test_overflow_detection() {
        let manager = BufferManager::new(BufferConfig {
            buffer_count: 3,
            buffer_size: 1024,
            detect_overflow: true,
            collect_stats: true,
        }).unwrap();

        // Write at index 0, read at index 1
        // Next write would be 1, which equals read -> overflow
        assert!(manager.would_overflow(0, 1));

        // Write at index 0, read at index 2
        // Next write would be 1, not equal to 2 -> no overflow
        assert!(!manager.would_overflow(0, 2));
    }

    #[test]
    fn test_statistics() {
        let mut manager = BufferManager::default_manager();

        manager.record_write(1024);
        manager.record_write(2048);

        assert_eq!(manager.stats().frames_written, 2);
        assert!((manager.stats().avg_frame_size - 1536.0).abs() < 0.1);
    }

    #[test]
    fn test_recommend_buffer_count() {
        // 1080p @ 30fps
        assert_eq!(BufferManager::recommend_buffer_count(30, (1920, 1080)), 2);

        // 1080p @ 60fps
        assert_eq!(BufferManager::recommend_buffer_count(60, (1920, 1080)), 3);

        // 4K @ 60fps
        assert_eq!(BufferManager::recommend_buffer_count(60, (3840, 2160)), 4);
    }

    #[test]
    fn test_layout_calculation() {
        let manager = BufferManager::new(BufferConfig {
            buffer_count: 4,
            buffer_size: 1920 * 1080 * 4,
            ..Default::default()
        }).unwrap();

        let layout = manager.layout();
        assert_eq!(layout.buffer_count, 4);
        assert!(layout.total_size > 0);
    }
}
