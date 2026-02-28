// Copyright 2024 Cloud Hypervisor Authors. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Input Event Batching
//!
//! This module provides event batching to improve performance by:
//! - Collecting multiple events before processing
//! - Reducing virtio queue operations
//! - Minimizing context switches

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::event::{InputEvent, InputRequest, KeyboardEvent, MouseEvent};
use super::{InputError, Result};

/// Default batch size (events)
pub const DEFAULT_BATCH_SIZE: usize = 16;

/// Default flush interval (microseconds)
pub const DEFAULT_FLUSH_INTERVAL_US: u64 = 1000; // 1ms

/// Event batcher configuration
#[derive(Clone, Debug)]
pub struct BatchConfig {
    /// Maximum events per batch
    pub max_batch_size: usize,
    /// Maximum time to wait before flushing (microseconds)
    pub flush_interval_us: u64,
    /// Enable adaptive batching
    pub adaptive: bool,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            max_batch_size: DEFAULT_BATCH_SIZE,
            flush_interval_us: DEFAULT_FLUSH_INTERVAL_US,
            adaptive: true,
        }
    }
}

/// Batched event container
#[derive(Clone, Debug)]
pub struct EventBatch {
    /// Keyboard events
    pub keyboard: Vec<KeyboardEvent>,
    /// Mouse events
    pub mouse: Vec<MouseEvent>,
    /// Creation timestamp
    pub created_at: Instant,
}

impl Default for EventBatch {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBatch {
    /// Create a new empty batch
    pub fn new() -> Self {
        Self {
            keyboard: Vec::new(),
            mouse: Vec::new(),
            created_at: Instant::now(),
        }
    }

    /// Create batch with capacity
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            keyboard: Vec::with_capacity(capacity),
            mouse: Vec::with_capacity(capacity),
            created_at: Instant::now(),
        }
    }

    /// Add keyboard event
    pub fn push_keyboard(&mut self, event: KeyboardEvent) {
        self.keyboard.push(event);
    }

    /// Add mouse event
    pub fn push_mouse(&mut self, event: MouseEvent) {
        self.mouse.push(event);
    }

    /// Add generic event
    pub fn push(&mut self, event: InputEvent) {
        match event {
            InputEvent::Keyboard(kb) => self.keyboard.push(kb),
            InputEvent::Mouse(m) => self.mouse.push(m),
        }
    }

    /// Total event count
    pub fn len(&self) -> usize {
        self.keyboard.len() + self.mouse.len()
    }

    /// Check if batch is empty
    pub fn is_empty(&self) -> bool {
        self.keyboard.is_empty() && self.mouse.is_empty()
    }

    /// Clear the batch
    pub fn clear(&mut self) {
        self.keyboard.clear();
        self.mouse.clear();
        self.created_at = Instant::now();
    }

    /// Convert to InputRequest
    pub fn into_request(self) -> InputRequest {
        InputRequest {
            backend: None,
            keyboard: self.keyboard,
            mouse: self.mouse,
        }
    }

    /// Age of the batch
    pub fn age(&self) -> Duration {
        self.created_at.elapsed()
    }

    /// Check if batch should be flushed based on age
    pub fn should_flush(&self, interval_us: u64) -> bool {
        !self.is_empty() && self.age().as_micros() as u64 >= interval_us
    }
}

/// Event batcher with adaptive timing
pub struct EventBatcher {
    /// Configuration
    config: BatchConfig,
    /// Current batch
    current_batch: EventBatch,
    /// Pending batches queue
    pending: VecDeque<EventBatch>,
    /// Statistics
    stats: BatchStats,
}

/// Batch statistics for adaptive tuning
#[derive(Clone, Debug, Default)]
pub struct BatchStats {
    /// Total batches processed
    pub batches_processed: u64,
    /// Total events processed
    pub events_processed: u64,
    /// Average batch size
    pub avg_batch_size: f64,
    /// Average latency (microseconds)
    pub avg_latency_us: f64,
    /// Last flush time
    last_flush: Option<Instant>,
}

impl EventBatcher {
    /// Create a new batcher
    pub fn new(config: BatchConfig) -> Self {
        Self {
            config,
            current_batch: EventBatch::with_capacity(DEFAULT_BATCH_SIZE),
            pending: VecDeque::with_capacity(8),
            stats: BatchStats::default(),
        }
    }

    /// Create batcher with default config
    pub fn default_batcher() -> Self {
        Self::new(BatchConfig::default())
    }

    /// Push an event into the batcher
    pub fn push(&mut self, event: InputEvent) {
        self.current_batch.push(event);

        // Check if we should flush
        if self.current_batch.len() >= self.config.max_batch_size {
            self.flush_current();
        }
    }

    /// Push keyboard event
    pub fn push_keyboard(&mut self, event: KeyboardEvent) {
        self.current_batch.push_keyboard(event);

        if self.current_batch.len() >= self.config.max_batch_size {
            self.flush_current();
        }
    }

    /// Push mouse event
    pub fn push_mouse(&mut self, event: MouseEvent) {
        self.current_batch.push_mouse(event);

        if self.current_batch.len() >= self.config.max_batch_size {
            self.flush_current();
        }
    }

    /// Push multiple events from InputRequest
    pub fn push_request(&mut self, request: InputRequest) {
        for kb in request.keyboard {
            self.push_keyboard(kb);
        }
        for m in request.mouse {
            self.push_mouse(m);
        }
    }

    /// Flush current batch to pending queue
    fn flush_current(&mut self) {
        if self.current_batch.is_empty() {
            return;
        }

        // Update statistics
        let batch_size = self.current_batch.len();
        self.stats.batches_processed += 1;
        self.stats.events_processed += batch_size as u64;
        self.stats.avg_batch_size = self.stats.events_processed as f64
            / self.stats.batches_processed as f64;
        self.stats.last_flush = Some(Instant::now());

        // Move to pending queue
        let batch = std::mem::replace(
            &mut self.current_batch,
            EventBatch::with_capacity(self.config.max_batch_size),
        );
        self.pending.push_back(batch);
    }

    /// Check if timer-based flush is needed
    pub fn should_timer_flush(&self) -> bool {
        self.current_batch.should_flush(self.config.flush_interval_us)
    }

    /// Timer-based flush (call periodically)
    pub fn timer_flush(&mut self) {
        if self.should_timer_flush() {
            self.flush_current();
        }
    }

    /// Get next pending batch
    pub fn pop_batch(&mut self) -> Option<EventBatch> {
        self.pending.pop_front()
    }

    /// Get next pending batch as InputRequest
    pub fn pop_request(&mut self) -> Option<InputRequest> {
        self.pop_batch().map(|b| b.into_request())
    }

    /// Peek at next pending batch without removing
    pub fn peek_batch(&self) -> Option<&EventBatch> {
        self.pending.front()
    }

    /// Number of pending batches
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Check if there are pending batches
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Force flush all events
    pub fn flush_all(&mut self) {
        self.flush_current();
    }

    /// Get statistics
    pub fn stats(&self) -> &BatchStats {
        &self.stats
    }

    /// Reset statistics
    pub fn reset_stats(&mut self) {
        self.stats = BatchStats::default();
    }

    /// Update configuration
    pub fn set_config(&mut self, config: BatchConfig) {
        self.config = config;
    }

    /// Get current configuration
    pub fn config(&self) -> &BatchConfig {
        &self.config
    }

    /// Adaptive tuning based on load
    pub fn adaptive_tune(&mut self) {
        if !self.config.adaptive {
            return;
        }

        // Increase batch size if we're frequently hitting the limit
        if self.stats.batches_processed > 100 {
            let full_batches = self.stats.batches_processed as f64;
            let avg_size = self.stats.avg_batch_size;

            // If average is close to max, increase batch size
            if avg_size > self.config.max_batch_size as f64 * 0.8 {
                self.config.max_batch_size =
                    (self.config.max_batch_size * 2).min(128);
            }
            // If average is very low, decrease batch size for lower latency
            else if avg_size < self.config.max_batch_size as f64 * 0.2 {
                self.config.max_batch_size =
                    (self.config.max_batch_size / 2).max(4);
            }
        }
    }
}

impl Default for EventBatcher {
    fn default() -> Self {
        Self::default_batcher()
    }
}

/// Batch processor trait for custom processing logic
pub trait BatchProcessor: Send {
    /// Process a batch of events
    fn process_batch(&mut self, batch: EventBatch) -> Result<()>;

    /// Flush any internal buffers
    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::event::{KeyboardAction, MouseAction, MouseButton};

    #[test]
    fn test_event_batch() {
        let mut batch = EventBatch::new();

        batch.push_keyboard(KeyboardEvent {
            action: KeyboardAction::Press,
            code: 0x1E,
            modifiers: Default::default(),
        });
        batch.push_mouse(MouseEvent {
            action: MouseAction::Move,
            x: 10,
            y: 20,
            z: 0,
            button: None,
            buttons: Default::default(),
        });

        assert_eq!(batch.len(), 2);
        assert!(!batch.is_empty());
    }

    #[test]
    fn test_batcher_basic() {
        let config = BatchConfig {
            max_batch_size: 4,
            flush_interval_us: 1000000, // 1 second
            adaptive: false,
        };
        let mut batcher = EventBatcher::new(config);

        // Add events
        for i in 0..3 {
            batcher.push_keyboard(KeyboardEvent {
                action: KeyboardAction::Press,
                code: i,
                modifiers: Default::default(),
            });
        }

        // Should not flush yet
        assert!(!batcher.has_pending());

        // Add one more to trigger flush
        batcher.push_keyboard(KeyboardEvent {
            action: KeyboardAction::Press,
            code: 3,
            modifiers: Default::default(),
        });

        // Should have pending batch
        assert!(batcher.has_pending());

        let batch = batcher.pop_batch().unwrap();
        assert_eq!(batch.len(), 4);
    }

    #[test]
    fn test_batch_into_request() {
        let mut batch = EventBatch::new();
        batch.push_keyboard(KeyboardEvent {
            action: KeyboardAction::Type,
            code: 0x1E,
            modifiers: Default::default(),
        });
        batch.push_mouse(MouseEvent {
            action: MouseAction::Click,
            x: 0,
            y: 0,
            z: 0,
            button: Some(MouseButton::Left),
            buttons: Default::default(),
        });

        let request = batch.into_request();
        assert_eq!(request.keyboard.len(), 1);
        assert_eq!(request.mouse.len(), 1);
        assert_eq!(request.event_count(), 2);
    }

    #[test]
    fn test_timer_flush() {
        let config = BatchConfig {
            max_batch_size: 100,
            flush_interval_us: 1, // 1 microsecond (will trigger immediately)
            adaptive: false,
        };
        let mut batcher = EventBatcher::new(config);

        batcher.push_keyboard(KeyboardEvent {
            action: KeyboardAction::Press,
            code: 0,
            modifiers: Default::default(),
        });

        // Wait a bit
        std::thread::sleep(Duration::from_micros(10));

        // Timer flush should trigger
        batcher.timer_flush();
        assert!(batcher.has_pending());
    }

    #[test]
    fn test_stats() {
        let mut batcher = EventBatcher::default_batcher();

        for i in 0..20 {
            batcher.push_keyboard(KeyboardEvent {
                action: KeyboardAction::Press,
                code: i,
                modifiers: Default::default(),
            });
        }

        let stats = batcher.stats();
        assert!(stats.batches_processed > 0);
        assert!(stats.events_processed > 0);
    }
}
