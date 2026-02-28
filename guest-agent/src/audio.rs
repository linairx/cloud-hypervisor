// Copyright 2024 lg-capture Authors
// SPDX-License-Identifier: Apache-2.0

//! Audio capture module
//!
//! Provides audio capture from the guest system.

use std::io;

use crate::protocol::AudioFormat;

/// Audio capture trait
pub trait AudioCapture: Send {
    /// Initialize audio capture
    fn init(&mut self, sample_rate: u32, channels: u8, format: AudioFormat) -> io::Result<()>;

    /// Start capturing
    fn start(&mut self) -> io::Result<()>;

    /// Stop capturing
    fn stop(&mut self) -> io::Result<()>;

    /// Read audio samples
    fn read_samples(&mut self, buffer: &mut [u8]) -> io::Result<usize>;

    /// Check if capturing
    fn is_capturing(&self) -> bool;
}

/// Audio configuration
#[derive(Debug, Clone)]
pub struct AudioConfig {
    pub sample_rate: u32,
    pub channels: u8,
    pub format: AudioFormat,
    pub buffer_duration_ms: u32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48000,
            channels: 2,
            format: AudioFormat::PcmS16Le,
            buffer_duration_ms: 20,
        }
    }
}

/// PulseAudio capture implementation
#[cfg(target_os = "linux")]
pub mod pulseaudio {
    use super::*;

    pub struct PulseAudioCapture {
        config: AudioConfig,
        active: bool,
    }

    impl PulseAudioCapture {
        pub fn new() -> io::Result<Self> {
            Ok(Self {
                config: AudioConfig::default(),
                active: false,
            })
        }
    }

    impl AudioCapture for PulseAudioCapture {
        fn init(&mut self, sample_rate: u32, channels: u8, format: AudioFormat) -> io::Result<()> {
            self.config.sample_rate = sample_rate;
            self.config.channels = channels;
            self.config.format = format;
            Ok(())
        }

        fn start(&mut self) -> io::Result<()> {
            // Would use libpulse-simple
            self.active = true;
            Ok(())
        }

        fn stop(&mut self) -> io::Result<()> {
            self.active = false;
            Ok(())
        }

        fn read_samples(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            if !self.active {
                return Err(io::Error::new(io::ErrorKind::NotConnected, "Not capturing"));
            }
            // Would read from PulseAudio
            Ok(0)
        }

        fn is_capturing(&self) -> bool {
            self.active
        }
    }
}

/// WASAPI capture implementation (Windows)
#[cfg(target_os = "windows")]
pub mod wasapi {
    use super::*;

    pub struct WasapiCapture {
        config: AudioConfig,
        active: bool,
    }

    impl WasapiCapture {
        pub fn new() -> io::Result<Self> {
            Ok(Self {
                config: AudioConfig::default(),
                active: false,
            })
        }
    }

    impl AudioCapture for WasapiCapture {
        fn init(&mut self, sample_rate: u32, channels: u8, format: AudioFormat) -> io::Result<()> {
                self.config.sample_rate = sample_rate;
                self.config.channels = channels;
                self.config.format = format;
                Ok(())
        }

        fn start(&mut self) -> io::Result<()> {
                self.active = true;
                Ok(())
        }

        fn stop(&mut self) -> io::Result<()> {
                self.active = false;
                Ok(())
        }

        fn read_samples(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
                Ok(0)
        }

        fn is_capturing(&self) -> bool {
                self.active
        }
    }
}

/// Stub audio capture
pub mod stub_audio {
    use super::*;

    pub struct StubAudioCapture {
        config: AudioConfig,
        active: bool,
        sample_count: u64,
    }

    impl StubAudioCapture {
        pub fn new() -> io::Result<Self> {
            Ok(Self {
                config: AudioConfig::default(),
                active: false,
                sample_count: 0,
            })
        }
    }

    impl AudioCapture for StubAudioCapture {
        fn init(&mut self, sample_rate: u32, channels: u8, format: AudioFormat) -> io::Result<()> {
            self.config.sample_rate = sample_rate;
            self.config.channels = channels;
            self.config.format = format;
            Ok(())
        }

        fn start(&mut self) -> io::Result<()> {
            self.active = true;
            Ok(())
        }

        fn stop(&mut self) -> io::Result<()> {
            self.active = false;
            Ok(())
        }

        fn read_samples(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if !self.active {
                return Err(io::Error::new(io::ErrorKind::NotConnected, "Not capturing"));
            }

            // Generate silence
            let bytes_per_sample = match self.config.format {
                AudioFormat::PcmS16Le => 2,
                AudioFormat::PcmS24Le => 3,
                AudioFormat::PcmS32Le | AudioFormat::FloatLe => 4,
            };

            let frame_size = bytes_per_sample * self.config.channels as usize;
            let frames = buffer.len() / frame_size;

            // Fill with silence (0 for PCM, 0.0 for float)
            buffer.fill(0);

            self.sample_count += frames as u64;
            Ok(frames * frame_size)
        }

        fn is_capturing(&self) -> bool {
            self.active
        }
    }
}

#[cfg(all(target_os = "linux", feature = "pulseaudio"))]
pub use pulseaudio::PulseAudioCapture as DefaultAudioCapture;

#[cfg(target_os = "windows")]
pub use wasapi::WasapiCapture as DefaultAudioCapture;

#[cfg(not(any(all(target_os = "linux", feature = "pulseaudio"), target_os = "windows")))]
pub use stub_audio::StubAudioCapture as DefaultAudioCapture;
