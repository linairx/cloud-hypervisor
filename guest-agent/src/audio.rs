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
#[cfg(all(target_os = "linux", feature = "pulseaudio"))]
pub mod pulseaudio {
    use super::*;
    use libpulse_binding::{sample, stream::Direction};
    use libpulse_simple_binding::Simple;

    pub struct PulseAudioCapture {
        config: AudioConfig,
        active: bool,
        /// PulseAudio simple connection
        pa_simple: Option<Simple>,
        /// Buffer for reading
        buffer: Vec<u8>,
    }

    impl PulseAudioCapture {
        pub fn new() -> io::Result<Self> {
            Ok(Self {
                config: AudioConfig::default(),
                active: false,
                pa_simple: None,
                buffer: Vec::new(),
            })
        }

        /// Convert AudioFormat to PulseAudio sample format
        fn to_pa_format(format: AudioFormat) -> sample::Format {
            match format {
                AudioFormat::PcmS16Le => sample::Format::S16le,
                AudioFormat::PcmS24Le => sample::Format::S24le,
                AudioFormat::PcmS32Le => sample::Format::S32le,
                AudioFormat::FloatLe => sample::Format::F32le,
            }
        }

        /// Create PulseAudio sample spec
        fn create_spec(&self) -> sample::Spec {
            sample::Spec {
                format: Self::to_pa_format(self.config.format),
                channels: self.config.channels,
                rate: self.config.sample_rate,
            }
        }
    }

    impl AudioCapture for PulseAudioCapture {
        fn init(&mut self, sample_rate: u32, channels: u8, format: AudioFormat) -> io::Result<()> {
            self.config.sample_rate = sample_rate;
            self.config.channels = channels;
            self.config.format = format;

            // Pre-allocate buffer for one buffer duration
            let bytes_per_sample = match format {
                AudioFormat::PcmS16Le => 2,
                AudioFormat::PcmS24Le => 3,
                AudioFormat::PcmS32Le | AudioFormat::FloatLe => 4,
            };
            let buffer_size = sample_rate as usize
                * (self.config.buffer_duration_ms as usize) / 1000
                * channels as usize
                * bytes_per_sample;
            self.buffer.resize(buffer_size, 0);

            Ok(())
        }

        fn start(&mut self) -> io::Result<()> {
            if self.active {
                return Ok(());
            }

            let spec = self.create_spec();

            // Connect to PulseAudio for recording (monitor source)
            let simple = Simple::new(
                None,               // Use default server
                "lg-guest-agent",   // Application name
                Direction::Record,  // Recording direction
                None,               // Use default device (monitor)
                "audio capture",    // Stream description
                &spec,              // Sample spec
                None,               // Default channel map
                None,               // Default buffering attributes
            ).map_err(|e| io::Error::new(io::ErrorKind::ConnectionRefused, format!("{}", e)))?;

            self.pa_simple = Some(simple);
            self.active = true;
            Ok(())
        }

        fn stop(&mut self) -> io::Result<()> {
            self.pa_simple = None;
            self.active = false;
            Ok(())
        }

        fn read_samples(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if !self.active {
                return Err(io::Error::new(io::ErrorKind::NotConnected, "Not capturing"));
            }

            let simple = self.pa_simple.as_ref().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotConnected, "PulseAudio not connected")
            })?;

            let to_read = buffer.len().min(self.buffer.len());

            simple.read(&mut buffer[..to_read])
                .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("{}", e)))?;

            Ok(to_read)
        }

        fn is_capturing(&self) -> bool {
            self.active
        }
    }
}

/// PulseAudio capture stub (when feature is disabled)
#[cfg(all(target_os = "linux", not(feature = "pulseaudio")))]
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
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "PulseAudio support not enabled. Rebuild with --features pulseaudio",
            ))
        }

        fn stop(&mut self) -> io::Result<()> {
            self.active = false;
            Ok(())
        }

        fn read_samples(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "PulseAudio support not enabled",
            ))
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
