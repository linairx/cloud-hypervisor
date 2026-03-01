// Copyright 2024 lg-capture Authors
// SPDX-License-Identifier: Apache-2.0

//! lg-capture Guest Agent
//!
//! This crate provides the guest-side implementation for frame capture,
//! cursor tracking, and audio capture in virtual machines.
//!
//! # Overview
//!
//! The guest agent runs inside the VM and communicates with the host
//! through shared memory regions. It provides:
//!
//! - **Frame Capture**: Captures screen frames using DXGI on Windows
//! - **Cursor Tracking**: Tracks cursor position and shape
//! - **Audio Capture**: Captures audio output from the guest
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                      Guest Application                       │
//! └─────────────────────────────────────────────────────────────┘
//!                            │
//!                            ▼
//! ┌─────────────────────────────────────────────────────────────┐
//! │                      Guest Agent                            │
//! │  - Frame capture (DXGI)                                     │
//! │  - Cursor tracking                                          │
//! │  - Audio capture                                            │
//! └─────────────────────────────────────────────────────────────┘
//!                            │
//!                            ▼
//! ┌─────────────────────────────────────────────────────────────┐
//! │                   Shared Memory Region                       │
//! │  - Frame buffer header                                      │
//! │  - Frame metadata                                           │
//! │  - Frame data                                               │
//! │  - Cursor data                                              │
//! │  - Audio buffer                                             │
//! └─────────────────────────────────────────────────────────────┘
//!                            │
//!                            ▼
//! ┌─────────────────────────────────────────────────────────────┐
//! │                      Host VMM                                │
//! │  - Reads frames from shared memory                          │
//! │  - Processes cursor updates                                 │
//! │  - Streams audio                                            │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Usage
//!
//! ```ignore
//! use guest_agent::{GuestAgent, AgentConfig};
//!
//! // Create guest agent with default configuration
//! let config = AgentConfig::default();
//! let mut agent = GuestAgent::new(config)?;
//!
//! // Start capture
//! agent.start_capture()?;
//!
//! // Main loop
//! loop {
//!     agent.capture_frame()?;
//!     agent.update_cursor()?;
//!     // ...
//! }
//! ```
//!
//! # Modules
//!
//! - [`capture`]: Frame capture implementation
//! - [`cursor`]: Cursor tracking implementation
//! - [`audio`]: Audio capture implementation
//! - [`protocol`]: Shared protocol definitions
//! - [`shm`]: Shared memory management

pub mod capture;
pub mod protocol;
pub mod shm;
pub mod cursor;
pub mod audio;
pub mod agent;

pub use agent::GuestAgent;
pub use agent::AgentConfig;
pub use audio::AudioConfig;
pub use protocol::*;
