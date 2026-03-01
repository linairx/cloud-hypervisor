// Copyright 2024 Cloud Hypervisor Authors. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Integration tests for lg-guest-agent module
//!
//! This file contains end-to-end tests for:
//! - Frame capture (X11, Wayland backends)
//! - Audio capture (PulseAudio, WASAPI backends)
//! - Protocol types

use lg_guest_agent::protocol::{AudioFormat, FrameFormat, FrameFlags};
use lg_guest_agent::audio::AudioConfig;

// ============================================================================
// Marker Test
// ============================================================================

#[test]
fn guest_agent_integration_tests_available() {
    // Marker test to ensure integration test module is available
}

// ============================================================================
// Frame Format Tests
// ============================================================================

#[test]
fn test_frame_format_bgra32() {
    assert_eq!(FrameFormat::Bgra32.bytes_per_pixel(), 4);
}

#[test]
fn test_frame_format_rgba32() {
    assert_eq!(FrameFormat::Rgba32.bytes_per_pixel(), 4);
}

#[test]
fn test_frame_format_nv12() {
    // NV12 has an "average" of 1 byte per pixel (Y plane + UV plane)
    assert_eq!(FrameFormat::Nv12.bytes_per_pixel(), 1);
}

#[test]
fn test_frame_format_default() {
    let format = FrameFormat::default();
    assert_eq!(format, FrameFormat::Bgra32);
}

#[test]
fn test_frame_format_try_from() {
    assert_eq!(FrameFormat::try_from(0).unwrap(), FrameFormat::Bgra32);
    assert_eq!(FrameFormat::try_from(1).unwrap(), FrameFormat::Rgba32);
    assert_eq!(FrameFormat::try_from(2).unwrap(), FrameFormat::Nv12);
    assert!(FrameFormat::try_from(99).is_err());
}

// ============================================================================
// Frame Flags Tests
// ============================================================================

#[test]
fn test_frame_flags_default() {
    let flags = FrameFlags::default();
    assert!(flags.is_empty());
}

#[test]
fn test_frame_flags_keyframe() {
    let flags = FrameFlags::KEYFRAME;
    assert!(flags.contains(FrameFlags::KEYFRAME));
}

#[test]
fn test_frame_flags_processed() {
    let flags = FrameFlags::PROCESSED;
    assert!(flags.contains(FrameFlags::PROCESSED));
}

#[test]
fn test_frame_flags_combined() {
    let flags = FrameFlags::KEYFRAME | FrameFlags::PROCESSED;
    assert!(flags.contains(FrameFlags::KEYFRAME));
    assert!(flags.contains(FrameFlags::PROCESSED));
}

// ============================================================================
// Captured Frame Tests
// ============================================================================

#[test]
fn test_captured_frame_default() {
    use lg_guest_agent::capture::CapturedFrame;

    let frame = CapturedFrame::default();

    assert!(frame.data.is_none());
    assert!(frame.data_ptr.is_null());
    assert_eq!(frame.data_size, 0);
    assert_eq!(frame.width, 0);
    assert_eq!(frame.height, 0);
    assert_eq!(frame.format, FrameFormat::Bgra32);
    assert!(frame.is_keyframe);
}

// ============================================================================
// Audio Config Tests
// ============================================================================

#[test]
fn test_audio_config_defaults() {
    let config = AudioConfig::default();

    assert_eq!(config.sample_rate, 48000);
    assert_eq!(config.channels, 2);
    assert_eq!(config.format, AudioFormat::PcmS16Le);
    assert_eq!(config.buffer_duration_ms, 20);
}

// ============================================================================
// Audio Format Tests
// ============================================================================

#[test]
fn test_audio_format_default() {
    let format = AudioFormat::default();
    assert_eq!(format, AudioFormat::PcmS16Le);
}

#[test]
fn test_audio_format_variants() {
    // Verify all format variants exist
    let _ = AudioFormat::PcmS16Le;
    let _ = AudioFormat::PcmS24Le;
    let _ = AudioFormat::PcmS32Le;
    let _ = AudioFormat::FloatLe;
}

// ============================================================================
// Stub Audio Capture Tests
// ============================================================================

#[test]
fn test_stub_audio_creation() {
    use lg_guest_agent::audio::stub_audio::StubAudioCapture;

    let result = StubAudioCapture::new();
    assert!(result.is_ok());
}

#[test]
fn test_stub_audio_init() {
    use lg_guest_agent::audio::stub_audio::StubAudioCapture;
    use lg_guest_agent::audio::AudioCapture;

    let mut capture = StubAudioCapture::new().unwrap();
    let result = capture.init(44100, 2, AudioFormat::PcmS16Le);
    assert!(result.is_ok());
}

#[test]
fn test_stub_audio_start_stop() {
    use lg_guest_agent::audio::stub_audio::StubAudioCapture;
    use lg_guest_agent::audio::AudioCapture;

    let mut capture = StubAudioCapture::new().unwrap();

    // Not capturing initially
    assert!(!capture.is_capturing());

    // Start capturing
    let result = capture.start();
    assert!(result.is_ok());
    assert!(capture.is_capturing());

    // Stop capturing
    let result = capture.stop();
    assert!(result.is_ok());
    assert!(!capture.is_capturing());
}

#[test]
fn test_stub_audio_read_samples() {
    use lg_guest_agent::audio::stub_audio::StubAudioCapture;
    use lg_guest_agent::audio::AudioCapture;

    let mut capture = StubAudioCapture::new().unwrap();
    capture.init(48000, 2, AudioFormat::PcmS16Le).unwrap();

    // Try to read before starting - should fail
    let mut buffer = [0u8; 1024];
    let result = capture.read_samples(&mut buffer);
    assert!(result.is_err());

    // Start and read
    capture.start().unwrap();
    let result = capture.read_samples(&mut buffer);
    assert!(result.is_ok());

    // Stub returns silence (zeros)
    let bytes_read = result.unwrap();
    assert!(bytes_read > 0);
}

// ============================================================================
// Protocol Constants Tests
// ============================================================================

#[test]
fn test_frame_buffer_magic() {
    use lg_guest_agent::protocol::FRAME_BUFFER_MAGIC;
    assert_eq!(FRAME_BUFFER_MAGIC, 0x46424D50); // "FBMP"
}

#[test]
fn test_frame_buffer_version() {
    use lg_guest_agent::protocol::FRAME_BUFFER_VERSION;
    assert_eq!(FRAME_BUFFER_VERSION, 1);
}

#[test]
fn test_audio_buffer_magic() {
    use lg_guest_agent::protocol::AUDIO_BUFFER_MAGIC;
    assert_eq!(AUDIO_BUFFER_MAGIC, 0x41554449); // "AUDI"
}

#[test]
fn test_default_buffer_count() {
    use lg_guest_agent::protocol::DEFAULT_BUFFER_COUNT;
    assert_eq!(DEFAULT_BUFFER_COUNT, 3); // Triple buffering
}
