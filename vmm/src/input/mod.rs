// Copyright 2024 Cloud Hypervisor Authors. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Input Backend Abstraction Layer
//!
//! This module provides a unified abstraction layer for input injection
//! into VMs, supporting multiple backend types:
//!
//! - **PS/2 (i8042)**: Legacy keyboard and mouse, highest stealth
//! - **virtio-input**: Modern VirtIO input devices
//! - **USB HID (planned)**: USB Human Interface Devices
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                      HTTP API Layer                         │
//! │              POST /vm/{id}/inject-input                     │
//! └─────────────────────────────────────────────────────────────┘
//!                            │
//!                            ▼
//! ┌─────────────────────────────────────────────────────────────┐
//! │                   Input Manager                             │
//! │  - Backend routing                                          │
//! │  - Event validation                                          │
//! │  - Multi-VM support                                          │
//! └─────────────────────────────────────────────────────────────┘
//!                            │
//!          ┌─────────────────┼─────────────────┐
//!          ▼                 ▼                 ▼
//! ┌─────────────┐   ┌─────────────┐   ┌─────────────┐
//! │ PS/2 Backend│   │ VirtIO      │   │ USB HID     │
//! │ (i8042)     │   │ Input       │   │ (planned)   │
//! │             │   │ Backend     │   │             │
//! │ Stealth:High│   │ Stealth:Low │   │ Stealth:Med │
//! └─────────────┘   └─────────────┘   └─────────────┘
//! ```

mod backend;
mod batch;
mod event;
mod manager;

pub use backend::{InputBackend, InputCapabilities, Ps2Backend, StealthLevel, UsbHidBackend, VirtioInputBackend};
pub use batch::{
    BatchConfig, BatchProcessor, BatchStats, EventBatch, EventBatcher,
    DEFAULT_BATCH_SIZE, DEFAULT_FLUSH_INTERVAL_US,
};
pub use event::{
    InputAction, InputDevice, InputEvent, InputRequest, KeyboardAction, KeyboardEvent,
    KeyboardModifiers, MouseAction, MouseButton, MouseButtons, MouseEvent,
};
pub use manager::{InputConfig, InputManager};

/// Result type for input operations
pub type Result<T> = std::result::Result<T, InputError>;

/// Input error types
#[derive(Debug, thiserror::Error)]
pub enum InputError {
    #[error("Backend not available: {0}")]
    BackendNotAvailable(String),

    #[error("Invalid input event: {0}")]
    InvalidEvent(String),

    #[error("Device not ready")]
    DeviceNotReady,

    #[error("Injection failed: {0}")]
    InjectionFailed(String),

    #[error("Unsupported action: {0}")]
    UnsupportedAction(String),

    #[error("Buffer overflow")]
    BufferOverflow,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
