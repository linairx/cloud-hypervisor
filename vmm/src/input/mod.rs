// Copyright 2024 Cloud Hypervisor Authors. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Input Backend Abstraction Layer
//!
//! This module provides a unified abstraction layer for input injection
//! into VMs, supporting multiple backend types:
//!
//! - **PS/2 (i8042)**: Legacy keyboard and mouse, highest stealth
//! - **virtio-input**: Modern VirtIO input devices
//! - **USB HID**: USB Human Interface Devices
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
//! │ (i8042)     │   │ Input       │   │ Backend     │
//! │             │   │ Backend     │   │             │
//! │ Stealth:High│   │ Stealth:Low │   │ Stealth:Med │
//! └─────────────┘   └─────────────┘   └─────────────┘
//! ```
//!
//! # Usage
//!
//! ```ignore
//! use vmm::input::{InputManager, InputConfig, InputEvent, KeyboardAction};
//!
//! // Create input manager with default PS/2 backend
//! let config = InputConfig::default();
//! let manager = InputManager::new(config);
//!
//! // Inject a keyboard event
//! let event = InputEvent::keyboard(KeyboardAction::Type, 0x1E); // 'A' key
//! manager.inject_event(&event)?;
//! ```
//!
//! # Backends
//!
//! ## PS/2 Backend
//!
//! The PS/2 backend uses the i8042 controller for keyboard and mouse input.
//! It provides the highest stealth level as PS/2 devices are native to most
//! operating systems and cannot be easily distinguished from physical hardware.
//!
//! ## VirtIO Input Backend
//!
//! The VirtIO Input backend uses virtio-input devices for modern input support.
//! It provides the most features (multi-touch, absolute positioning) but has
//! low stealth due to Red Hat vendor ID in device identification.
//!
//! ## USB HID Backend
//!
//! The USB HID backend emulates USB Human Interface Devices. It provides
//! medium stealth and wide compatibility across operating systems.

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

/// Result type for input operations.
///
/// All input operations return this type, which wraps the success value
/// or an [`InputError`] on failure.
pub type Result<T> = std::result::Result<T, InputError>;

/// Input error types.
///
/// This enum defines all possible errors that can occur during input
/// injection operations.
///
/// # Example
///
/// ```ignore
/// use vmm::input::{InputError, Result};
///
/// fn handle_result(result: Result<()>) {
///     match result {
///         Ok(()) => println!("Input injected successfully"),
///         Err(InputError::DeviceNotReady) => eprintln!("Device not ready"),
///         Err(InputError::InjectionFailed(msg)) => eprintln!("Injection failed: {}", msg),
///         Err(e) => eprintln!("Error: {}", e),
///     }
/// }
/// ```
#[derive(Debug, thiserror::Error)]
pub enum InputError {
    /// The requested backend is not available or not configured.
    ///
    /// This error occurs when trying to use a backend that has not been
    /// initialized or is not supported on the current platform.
    #[error("Backend not available: {0}")]
    BackendNotAvailable(String),

    /// The input event is invalid or malformed.
    ///
    /// This error occurs when the event fails validation, such as
    /// invalid key codes or out-of-range coordinates.
    #[error("Invalid input event: {0}")]
    InvalidEvent(String),

    /// The input device is not ready to accept events.
    ///
    /// This error occurs when the backend has not completed initialization
    /// or the guest driver has not yet connected.
    #[error("Device not ready")]
    DeviceNotReady,

    /// The input injection operation failed.
    ///
    /// This error wraps the underlying error message from the device driver
    /// or backend implementation.
    #[error("Injection failed: {0}")]
    InjectionFailed(String),

    /// The requested action is not supported by the backend.
    ///
    /// This error occurs when trying to use a feature that the current
    /// backend does not support, such as absolute positioning on PS/2.
    #[error("Unsupported action: {0}")]
    UnsupportedAction(String),

    /// The input buffer has overflowed.
    ///
    /// This error occurs when too many events are queued and the buffer
    /// cannot accommodate additional events.
    #[error("Buffer overflow")]
    BufferOverflow,

    /// An I/O error occurred.
    ///
    /// This error wraps standard I/O errors that may occur during
    /// device communication.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
