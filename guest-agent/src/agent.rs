// Copyright 2024 lg-capture Authors
// SPDX-License-Identifier: Apache-2.0

//! Guest Agent main implementation
//!
//! Coordinates frame capture, cursor tracking, and audio capture.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use log::{debug, info, warn};

use crate::capture::{FrameCapture, DefaultCapture};
use crate::cursor::{CursorCapture, DefaultCursorCapture};
use crate::audio::{AudioCapture, DefaultAudioCapture, AudioConfig};
use crate::protocol::*;
use crate::shm::SharedMemory;

/// Guest Agent configuration
#[derive(Debug, Clone)]
pub struct AgentConfig {
    /// Shared memory path
    pub shm_path: String,
    /// Target frame rate
    pub target_fps: u32,
    /// Enable audio capture
    pub enable_audio: bool,
    /// Audio configuration
    pub audio_config: AudioConfig,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            shm_path: "/dev/shm/lg-capture".to_string(),
            target_fps: 60,
            enable_audio: false,
            audio_config: AudioConfig::default(),
        }
    }
}

/// Guest Agent
pub struct GuestAgent {
    /// Configuration
    config: AgentConfig,
    /// Shared memory
    shm: SharedMemory,
    /// Frame capture backend
    frame_capture: DefaultCapture,
    /// Cursor capture backend
    cursor_capture: DefaultCursorCapture,
    /// Audio capture backend (optional)
    audio_capture: Option<DefaultAudioCapture>,
    /// Running flag
    running: Arc<AtomicBool>,
    /// Frame thread handle
    frame_thread: Option<thread::JoinHandle<()>>,
    /// Cursor thread handle
    cursor_thread: Option<thread::JoinHandle<()>>,
    /// Audio thread handle
    audio_thread: Option<thread::JoinHandle<()>>,
}

impl GuestAgent {
    /// Create a new Guest Agent
    pub fn new(config: AgentConfig) -> io::Result<Self> {
        let shm = SharedMemory::open(&config.shm_path)?;
        let frame_capture = DefaultCapture::new()?;
        let cursor_capture = DefaultCursorCapture::new()?;
        let audio_capture = if config.enable_audio {
            Some(DefaultAudioCapture::new()?)
        } else {
            None
        };

        Ok(Self {
            config,
            shm,
            frame_capture,
            cursor_capture,
            audio_capture,
            running: Arc::new(AtomicBool::new(false)),
            frame_thread: None,
            cursor_thread: None,
            audio_thread: None,
        })
    }

    /// Start the guest agent
    pub fn start(&mut self) -> io::Result<()> {
        if self.running.load(Ordering::SeqCst) {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "Agent already running"));
        }

        self.running.store(true, Ordering::SeqCst);

        // Set initializing state
        self.shm.set_guest_state(GuestState::Initializing);

        // Get frame configuration from header
        let header = self.shm.header();
        let width = header.frame_width;
        let height = header.frame_height;
        let format = header.format;

        // Initialize frame capture
        self.frame_capture.init(width, height, format)?;
        self.frame_capture.start()?;

        // Initialize cursor capture
        let cursor_shape = self.cursor_capture.get_shape()?;
        if let Some(shape) = cursor_shape {
            self.shm.write_cursor_shape(&shape.data, &shape.info)?;
        }

        // Initialize audio capture if enabled
        if let Some(ref mut audio) = &mut self.audio_capture {
            audio.init(
                self.config.audio_config.sample_rate,
                self.config.audio_config.channels,
                self.config.audio_config.format,
            )?;
            audio.start()?;
        }

        // Note: Frame capture is done via run_iteration() in the main loop
        // This design allows the caller to control the frame rate
        self.shm.set_guest_state(GuestState::Capturing);

        info!("Guest agent started");
        Ok(())
    }

    /// Stop the guest agent
    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        self.shm.set_guest_state(GuestState::Idle);

        self.frame_capture.stop().ok();

        if let Some(ref mut audio) = self.audio_capture {
            audio.stop().ok();
        }

        // Wait for threads
        if let Some(handle) = self.frame_thread.take() {
            handle.join().ok();
        }
        if let Some(handle) = self.cursor_thread.take() {
            handle.join().ok();
        }
        if let Some(handle) = self.audio_thread.take() {
            handle.join().ok();
        }

        info!("Guest agent stopped");
    }

    /// Run one capture iteration
    pub fn run_iteration(&mut self) -> io::Result<()> {
        // Check for commands from host
        let command = self.shm.get_command();
        match command {
            GuestCommand::StartCapture => {
                if !self.frame_capture.is_active() {
                    self.frame_capture.start()?;
                    self.shm.set_guest_state(GuestState::Capturing);
                    info!("Started capture");
                }
            }
            GuestCommand::StopCapture => {
                if self.frame_capture.is_active() {
                    self.frame_capture.stop()?;
                    self.shm.set_guest_state(GuestState::Idle);
                    info!("Stopped capture");
                }
            }
            _ => {}
        }

        // Capture frame if active
        if self.frame_capture.is_active() {
            match self.frame_capture.capture_frame() {
                Ok(frame) => {
                    let next_index = self.shm.get_next_index();

                    // Use zero-copy path if available
                    if !frame.data_ptr.is_null() {
                        self.shm.write_frame_from_ptr(
                            next_index,
                            frame.data_ptr,
                            frame.data_size,
                        )?;
                    } else if let Some(ref data) = frame.data {
                        self.shm.write_frame(next_index, data)?;
                    }

                    self.shm.commit_frame(next_index);
                    debug!(
                        "Captured frame {}x{} ({} bytes)",
                        frame.width, frame.height, frame.data_size
                    );
                }
                Err(e) => {
                    warn!("Frame capture failed: {}", e);
                }
            }
        }

        // Update cursor
        if let Ok((x, y)) = self.cursor_capture.get_position() {
            // Would update cursor metadata in shared memory
            debug!("Cursor position: {}, {}", x, y);
        }

        Ok(())
    }

    /// Check if agent is running
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Get frame count
    pub fn frame_count(&self) -> u64 {
        self.shm.get_frame_count()
    }
}

impl Drop for GuestAgent {
    fn drop(&mut self) {
        self.stop();
    }
}
