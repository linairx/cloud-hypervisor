// Copyright 2024 lg-capture Authors
// SPDX-License-Identifier: Apache-2.0

//! Shared protocol definitions between Host and Guest
//!
//! These structures are mirrored in the Cloud Hypervisor frame_buffer module
//! and must be kept in sync.
//!
//! # Overview
//!
//! The protocol defines the shared memory layout and message types used
//! for communication between the host VMM and the guest agent.
//!
//! # Memory Layout
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                   Frame Buffer Header                        │
//! │  - Magic number, version, dimensions                        │
//! │  - Command/state synchronization                            │
//! │  - Cursor metadata                                          │
//! └─────────────────────────────────────────────────────────────┘
//! ┌─────────────────────────────────────────────────────────────┐
//! │                   Frame Metadata Array                       │
//! │  - One entry per buffer                                     │
//! │  - Frame number, timestamp, flags                           │
//! └─────────────────────────────────────────────────────────────┘
//! ┌─────────────────────────────────────────────────────────────┐
//! │                   Frame Data Buffers                         │
//! │  - Multiple buffers for triple buffering                    │
//! │  - Actual pixel data                                        │
//! └─────────────────────────────────────────────────────────────┘
//! ┌─────────────────────────────────────────────────────────────┐
//! │                   Cursor Data                                │
//! │  - Cursor shape (BGRA pixels)                               │
//! │  - Hotspot information                                      │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Synchronization
//!
//! The protocol uses atomic operations and flags for synchronization:
//! - Host writes commands, Guest reads and executes
//! - Guest writes state and frame data, Host reads
//! - Active buffer index is used for triple buffering

use serde::{Deserialize, Serialize};

/// Magic number for frame buffer header validation: "FBMP".
///
/// Used to validate that the shared memory region contains valid data.
pub const FRAME_BUFFER_MAGIC: u32 = 0x46424D50;

/// Current protocol version.
///
/// Incremented when the protocol structure changes incompatibly.
pub const FRAME_BUFFER_VERSION: u32 = 1;

/// Default number of buffers (triple buffering).
///
/// Triple buffering reduces tearing and improves frame rate.
pub const DEFAULT_BUFFER_COUNT: u32 = 3;

/// Maximum cursor data size (64KB - enough for 128x128 BGRA cursor).
pub const MAX_CURSOR_SIZE: u32 = 64 * 1024;

/// Default audio buffer size (1MB).
pub const DEFAULT_AUDIO_BUFFER_SIZE: u32 = 1024 * 1024;

/// Audio buffer magic number: "AUDI".
pub const AUDIO_BUFFER_MAGIC: u32 = 0x41554449;
/// Audio buffer version.
pub const AUDIO_BUFFER_VERSION: u32 = 1;

/// Guest Agent commands (Host -> Guest).
///
/// These commands are sent from the host VMM to control the guest agent.
///
/// # Example
///
/// ```ignore
/// use guest_agent::protocol::GuestCommand;
///
/// // Host sends start capture command
/// header.command = GuestCommand::StartCapture as u32;
///
/// // Guest reads and executes
/// let cmd = GuestCommand::try_from(header.command)?;
/// match cmd {
///     GuestCommand::StartCapture => agent.start_capture()?,
///     GuestCommand::StopCapture => agent.stop_capture()?,
///     // ...
/// }
/// ```
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GuestCommand {
    /// No command / idle
    None = 0,
    /// Start capturing frames
    StartCapture = 1,
    /// Stop capturing frames
    StopCapture = 2,
    /// Set format (width, height, pixel format)
    SetFormat = 3,
}

impl Default for GuestCommand {
    fn default() -> Self {
        GuestCommand::None
    }
}

impl TryFrom<u32> for GuestCommand {
    type Error = &'static str;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(GuestCommand::None),
            1 => Ok(GuestCommand::StartCapture),
            2 => Ok(GuestCommand::StopCapture),
            3 => Ok(GuestCommand::SetFormat),
            _ => Err("Invalid guest command value"),
        }
    }
}

/// Guest Agent state (Guest -> Host).
///
/// The guest agent reports its current state to the host through
/// the frame buffer header.
///
/// # State Transitions
///
/// ```text
/// Initializing -> Idle -> Capturing <-> Idle
///                    \-> Error
/// ```
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GuestState {
    /// Idle state, not capturing
    Idle = 0,
    /// Actively capturing frames
    Capturing = 1,
    /// Error state
    Error = 2,
    /// Initializing
    Initializing = 3,
}

impl Default for GuestState {
    fn default() -> Self {
        GuestState::Idle
    }
}

impl TryFrom<u32> for GuestState {
    type Error = &'static str;

    fn try_from(value: u32) -> Result<Self, &'static str> {
        match value {
            0 => Ok(GuestState::Idle),
            1 => Ok(GuestState::Capturing),
            2 => Ok(GuestState::Error),
            3 => Ok(GuestState::Initializing),
            _ => Err("Invalid guest state value"),
        }
    }
}

/// Frame format enumeration.
///
/// Defines the pixel format for captured frames.
///
/// # Formats
///
/// - **Bgra32**: 32-bit BGRA, 4 bytes per pixel, most common
/// - **Rgba32**: 32-bit RGBA, 4 bytes per pixel
/// - **Nv12**: YUV 4:2:0, 1.5 bytes per pixel, video encoding
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FrameFormat {
    /// 32-bit BGRA format (Blue, Green, Red, Alpha)
    Bgra32 = 0,
    /// 32-bit RGBA format (Red, Green, Blue, Alpha)
    Rgba32 = 1,
    /// NV12 format (YUV 4:2:0)
    Nv12 = 2,
}

impl Default for FrameFormat {
    fn default() -> Self {
        FrameFormat::Bgra32
    }
}

impl TryFrom<u32> for FrameFormat {
    type Error = &'static str;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(FrameFormat::Bgra32),
            1 => Ok(FrameFormat::Rgba32),
            2 => Ok(FrameFormat::Nv12),
            _ => Err("Invalid frame format value"),
        }
    }
}

impl FrameFormat {
    /// Get bytes per pixel for this format
    pub fn bytes_per_pixel(&self) -> u8 {
        match self {
            FrameFormat::Bgra32 | FrameFormat::Rgba32 => 4,
            FrameFormat::Nv12 => 1, // Average (Y plane + UV plane)
        }
    }
}

/// Frame metadata flags.
///
/// These flags provide additional information about each frame.
///
/// # Example
///
/// ```ignore
/// use guest_agent::protocol::FrameFlags;
///
/// // Mark frame as keyframe
/// metadata.flags = FrameFlags::KEYFRAME;
///
/// // Check if frame has error
/// if metadata.flags.contains(FrameFlags::ERROR) {
///     // Handle error
/// }
/// ```
bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct FrameFlags: u32 {
        /// No flags set
        const NONE = 0;
        /// Frame is a keyframe (full frame, not delta)
        const KEYFRAME = 1 << 0;
        /// Frame has been processed by guest
        const PROCESSED = 1 << 1;
        /// Frame is the last frame in stream
        const EOS = 1 << 2;
        /// Frame has error
        const ERROR = 1 << 3;
    }
}

impl Default for FrameFlags {
    fn default() -> Self {
        FrameFlags::empty()
    }
}

/// Frame metadata (placed after header, one per buffer).
///
/// Each frame buffer has associated metadata that describes the frame.
///
/// # Memory Layout
///
/// This structure is `#[repr(C)]` for stable memory layout across FFI.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameMetadata {
    /// Frame number (monotonically increasing).
    ///
    /// Used to detect dropped frames.
    pub frame_number: u64,
    /// Timestamp in nanoseconds.
    ///
    /// Typically from a monotonic clock for timing accuracy.
    pub timestamp_ns: u64,
    /// Frame flags.
    ///
    /// Indicates keyframe, error, etc.
    pub flags: FrameFlags,
    /// Actual data size in bytes.
    ///
    /// May be less than buffer size for compressed frames.
    pub data_size: u32,
    /// Reserved for future use
    pub reserved: [u8; 16],
}

/// Frame buffer header (fixed size, at start of shared memory).
///
/// This is the main control structure for the shared memory region.
/// It contains configuration, state, and synchronization fields.
///
/// # Memory Layout
///
/// This structure is `#[repr(C)]` for stable memory layout across FFI.
/// The header is followed by frame metadata, then frame data, then cursor data.
///
/// # Synchronization
///
/// - `active_index`: Points to the buffer currently being written
/// - `command`: Host sends commands, guest reads and executes
/// - `guest_state`: Guest reports state, host reads
/// - `cursor_updated`: Incremented when cursor shape changes
#[repr(C)]
#[derive(Debug)]
pub struct FrameBufferHeader {
    /// Magic number for validation (`FRAME_BUFFER_MAGIC`)
    pub magic: u32,
    /// Protocol version (`FRAME_BUFFER_VERSION`)
    pub version: u32,
    /// Number of buffers (typically 3 for triple buffering)
    pub buffer_count: u32,
    /// Size of each buffer in bytes
    pub buffer_size: u64,
    /// Frame width in pixels
    pub frame_width: u32,
    /// Frame height in pixels
    pub frame_height: u32,
    /// Pixel format
    pub format: FrameFormat,
    /// Current active buffer index (0-based)
    pub active_index: u32,
    /// Total frame count since capture started
    pub frame_count: u64,
    /// Host -> Guest command (see `GuestCommand`)
    pub command: u32,
    /// Guest -> Host state (see `GuestState`)
    pub guest_state: u32,
    /// Guest Agent PID (for debugging)
    pub guest_pid: u32,
    /// Cursor data offset from start of shared memory
    pub cursor_offset: u64,
    /// Cursor data size in bytes
    pub cursor_size: u32,
    /// Cursor hotspot X coordinate
    pub cursor_hot_x: i16,
    /// Cursor hotspot Y coordinate
    pub cursor_hot_y: i16,
    /// Cursor width in pixels
    pub cursor_width: u16,
    /// Cursor height in pixels
    pub cursor_height: u16,
    /// Cursor update counter (incremented on shape change)
    pub cursor_updated: u32,
}

/// Cursor metadata.
///
/// Contains cursor position and visibility information.
/// Updated by the guest agent on each frame.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct CursorMetadata {
    /// Cursor X position in pixels
    pub x: i32,
    /// Cursor Y position in pixels
    pub y: i32,
    /// Cursor visibility (0=hidden, 1=visible)
    pub visible: u32,
    /// Shape updated flag (non-zero if shape changed)
    pub shape_updated: u32,
}

/// Cursor shape info.
///
/// Describes the cursor image dimensions and hotspot.
/// The actual pixel data follows in BGRA format.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct CursorShapeInfo {
    /// Cursor width in pixels
    pub width: u16,
    /// Cursor height in pixels
    pub height: u16,
    /// Hotspot X coordinate (click point)
    pub hot_x: i16,
    /// Hotspot Y coordinate (click point)
    pub hot_y: i16,
    /// Size of cursor data in bytes (width * height * 4)
    pub data_size: u32,
}

/// Audio format.
///
/// Defines the sample format for audio capture.
///
/// # Formats
///
/// - **PcmS16Le**: 16-bit signed PCM, most common
/// - **PcmS24Le**: 24-bit signed PCM, higher quality
/// - **PcmS32Le**: 32-bit signed PCM, highest quality
/// - **FloatLe**: 32-bit float, professional use
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AudioFormat {
    /// 16-bit signed PCM, little-endian
    PcmS16Le = 0,
    /// 24-bit signed PCM, little-endian
    PcmS24Le = 1,
    /// 32-bit signed PCM, little-endian
    PcmS32Le = 2,
    /// 32-bit float, little-endian
    FloatLe = 3,
}

impl Default for AudioFormat {
    fn default() -> Self {
        AudioFormat::PcmS16Le
    }
}

/// Audio buffer header.
///
/// Controls the shared memory ring buffer for audio streaming.
/// Uses a producer-consumer model with atomic positions.
///
/// # Synchronization
///
/// - Guest writes audio data and updates `write_pos`
/// - Host reads audio data and updates `read_pos`
/// - Buffer wraps when position reaches `buffer_size`
#[repr(C)]
#[derive(Debug)]
pub struct AudioBufferHeader {
    /// Magic number for validation (`AUDIO_BUFFER_MAGIC`)
    pub magic: u32,
    /// Protocol version (`AUDIO_BUFFER_VERSION`)
    pub version: u32,
    /// Audio sample format
    pub format: AudioFormat,
    /// Sample rate in Hz (e.g., 44100, 48000)
    pub sample_rate: u32,
    /// Number of channels (1=mono, 2=stereo)
    pub channels: u8,
    /// Reserved for alignment
    pub reserved: [u8; 3],
    /// Total buffer size in bytes
    pub buffer_size: u32,
    /// Write position (atomic, updated by guest)
    pub write_pos: u32,
    /// Read position (atomic, updated by host)
    pub read_pos: u32,
    /// Total bytes written since start
    pub total_written: u64,
    /// Active flag (1=capturing, 0=stopped)
    pub active: u32,
}

/// VirtIO Input event (for VirtIO backend).
///
/// Represents a single input event in Linux evdev format.
/// Used by the VirtIO input backend for event injection.
///
/// # Event Types
///
/// - `0x00` (EV_SYN): Synchronization event
/// - `0x01` (EV_KEY): Keyboard/button event
/// - `0x02` (EV_REL): Relative mouse movement
/// - `0x03` (EV_ABS): Absolute mouse position
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct VirtioInputEvent {
    /// Event type (EV_KEY, EV_REL, EV_SYN, etc.)
    pub ev_type: u16,
    /// Event code (key code, button code, or axis)
    pub code: u16,
    /// Event value (1=press, 0=release, or movement delta)
    pub value: u32,
}

impl VirtioInputEvent {
    /// Create a keyboard event.
    ///
    /// # Arguments
    ///
    /// * `code` - Linux key code (e.g., KEY_A = 0x1E)
    /// * `pressed` - `true` for press, `false` for release
    pub fn keyboard(code: u16, pressed: bool) -> Self {
        Self {
            ev_type: 0x01, // EV_KEY
            code,
            value: if pressed { 1 } else { 0 },
        }
    }

    /// Create a relative mouse event.
    ///
    /// # Arguments
    ///
    /// * `code` - Relative axis (REL_X = 0x00, REL_Y = 0x01, REL_WHEEL = 0x08)
    /// * `value` - Movement delta
    pub fn rel(code: u16, value: i32) -> Self {
        Self {
            ev_type: 0x02, // EV_REL
            code,
            value: value as u32,
        }
    }

    /// Create a sync event.
    ///
    /// Sync events mark the end of a batch of input events.
    pub fn syn() -> Self {
        Self {
            ev_type: 0x00, // EV_SYN
            code: 0,
            value: 0,
        }
    }
}

/// Input event type (for protocol communication).
///
/// Tagged union of keyboard and mouse events for serialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InputEvent {
    /// Keyboard event
    Keyboard(KeyboardEvent),
    /// Mouse event
    Mouse(MouseEvent),
}

/// Keyboard event (protocol version).
///
/// Represents a keyboard action for input injection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyboardEvent {
    /// Key action (press, release, or type)
    pub action: KeyAction,
    /// Key code (scancode)
    pub code: u16,
    /// Active modifiers
    #[serde(default)]
    pub modifiers: KeyboardModifiers,
}

/// Key action type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyAction {
    /// Key press
    Press,
    /// Key release
    Release,
    /// Key press and release (convenience)
    Type,
}

/// Keyboard modifier states.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct KeyboardModifiers {
    /// Control key
    pub ctrl: bool,
    /// Alt key
    pub alt: bool,
    /// Shift key
    pub shift: bool,
    /// Meta/Windows key
    pub meta: bool,
}

/// Mouse event (protocol version).
///
/// Represents a mouse action for input injection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MouseEvent {
    /// Mouse action type
    pub action: MouseAction,
    /// X coordinate or delta
    pub x: i32,
    /// Y coordinate or delta
    pub y: i32,
    /// Z coordinate (scroll wheel)
    pub z: i32,
    /// Button for button actions
    pub button: Option<MouseButton>,
    /// All button states
    #[serde(default)]
    pub buttons: MouseButtons,
}

/// Mouse action type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseAction {
    /// Relative movement
    Move,
    /// Absolute positioning
    MoveAbsolute,
    /// Button press
    ButtonPress,
    /// Button release
    ButtonRelease,
    /// Button click (press + release)
    Click,
    /// Scroll wheel
    Scroll,
}

/// Mouse button identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseButton {
    /// Left button
    Left,
    /// Right button
    Right,
    /// Middle button
    Middle,
    /// Side button
    Side,
    /// Extra button
    Extra,
}

/// Mouse button states.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct MouseButtons {
    /// Left button state
    pub left: bool,
    /// Right button state
    pub right: bool,
    /// Middle button state
    pub middle: bool,
    /// Side button state
    pub side: bool,
    /// Extra button state
    pub extra: bool,
}
