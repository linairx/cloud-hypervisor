// Copyright 2024 lg-capture Authors
// SPDX-License-Identifier: Apache-2.0

//! lg-capture Guest Agent
//!
//! This crate provides the guest-side implementation for frame capture,
//! cursor tracking, and audio capture in virtual machines.

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
