// Copyright 2024 lg-capture Authors
// SPDX-License-Identifier: Apache-2.0

//! Shared protocol definitions between Host and Guest
//!
//! These structures are mirrored in the Cloud Hypervisor frame_buffer module
//! and must be kept in sync.

use serde::{Deserialize, Serialize};

/// Magic number for frame buffer header validation: "FBMP"
pub const FRAME_BUFFER_MAGIC: u32 = 0x46424D50;

/// Current protocol version
pub const FRAME_BUFFER_VERSION: u32 = 1;

/// Default number of buffers (triple buffering)
pub const DEFAULT_BUFFER_COUNT: u32 = 3;

/// Maximum cursor data size (64KB - enough for 128x128 BGRA cursor)
pub const MAX_CURSOR_SIZE: u32 = 64 * 1024;

/// Default audio buffer size (1MB)
pub const DEFAULT_AUDIO_BUFFER_SIZE: u32 = 1024 * 1024;

/// Guest Agent commands (Host -> Guest)
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

/// Guest Agent state (Guest -> Host)
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

/// Frame format enumeration
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

/// Frame metadata flags
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

/// Frame metadata (placed after header, one per buffer)
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameMetadata {
    /// Frame number (monotonically increasing)
    pub frame_number: u64,
    /// Timestamp in nanoseconds
    pub timestamp_ns: u64,
    /// Frame flags
    pub flags: FrameFlags,
    /// Actual data size in bytes
    pub data_size: u32,
    /// Reserved for future use
    pub reserved: [u8; 16],
}

/// Frame buffer header (fixed size, at start of shared memory)
#[repr(C)]
#[derive(Debug)]
pub struct FrameBufferHeader {
    /// Magic number for validation
    pub magic: u32,
    /// Protocol version
    pub version: u32,
    /// Number of buffers
    pub buffer_count: u32,
    /// Size of each buffer
    pub buffer_size: u64,
    /// Frame width
    pub frame_width: u32,
    /// Frame height
    pub frame_height: u32,
    /// Pixel format
    pub format: FrameFormat,
    /// Current active buffer index
    pub active_index: u32,
    /// Total frame count
    pub frame_count: u64,
    /// Host -> Guest command
    pub command: u32,
    /// Guest -> Host state
    pub guest_state: u32,
    /// Guest Agent PID
    pub guest_pid: u32,
    /// Cursor data offset
    pub cursor_offset: u64,
    /// Cursor data size
    pub cursor_size: u32,
    /// Cursor hotspot X
    pub cursor_hot_x: i16,
    /// Cursor hotspot Y
    pub cursor_hot_y: i16,
    /// Cursor width
    pub cursor_width: u16,
    /// Cursor height
    pub cursor_height: u16,
    /// Cursor update counter
    pub cursor_updated: u32,
}

/// Cursor metadata
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct CursorMetadata {
    /// Cursor X position
    pub x: i32,
    /// Cursor Y position
    pub y: i32,
    /// Cursor visibility
    pub visible: u32,
    /// Shape updated flag
    pub shape_updated: u32,
}

/// Cursor shape info
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct CursorShapeInfo {
    /// Cursor width
    pub width: u16,
    /// Cursor height
    pub height: u16,
    /// Hotspot X
    pub hot_x: i16,
    /// Hotspot Y
    pub hot_y: i16,
    /// Data size
    pub data_size: u32,
}

/// Audio format
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

/// Audio buffer header
#[repr(C)]
#[derive(Debug)]
pub struct AudioBufferHeader {
    /// Magic: "AUDI"
    pub magic: u32,
    /// Version
    pub version: u32,
    /// Audio format
    pub format: AudioFormat,
    /// Sample rate
    pub sample_rate: u32,
    /// Channels
    pub channels: u8,
    /// Reserved
    pub reserved: [u8; 3],
    /// Buffer size
    pub buffer_size: u32,
    /// Write position
    pub write_pos: u32,
    /// Read position
    pub read_pos: u32,
    /// Total written
    pub total_written: u64,
    /// Active flag
    pub active: u32,
}

/// VirtIO Input event (for VirtIO backend)
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct VirtioInputEvent {
    /// Event type
    pub ev_type: u16,
    /// Event code
    pub code: u16,
    /// Event value
    pub value: u32,
}

impl VirtioInputEvent {
    /// Create a keyboard event
    pub fn keyboard(code: u16, pressed: bool) -> Self {
        Self {
            ev_type: 0x01, // EV_KEY
            code,
            value: if pressed { 1 } else { 0 },
        }
    }

    /// Create a relative mouse event
    pub fn rel(code: u16, value: i32) -> Self {
        Self {
            ev_type: 0x02, // EV_REL
            code,
            value: value as u32,
        }
    }

    /// Create a sync event
    pub fn syn() -> Self {
        Self {
            ev_type: 0x00, // EV_SYN
            code: 0,
            value: 0,
        }
    }
}

/// Input event type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InputEvent {
    /// Keyboard event
    Keyboard(KeyboardEvent),
    /// Mouse event
    Mouse(MouseEvent),
}

/// Keyboard event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyboardEvent {
    /// Key action
    pub action: KeyAction,
    /// Key code
    pub code: u16,
    /// Modifiers
    #[serde(default)]
    pub modifiers: KeyboardModifiers,
}

/// Key action
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyAction {
    Press,
    Release,
    Type,
}

/// Keyboard modifiers
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct KeyboardModifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
}

/// Mouse event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MouseEvent {
    /// Action
    pub action: MouseAction,
    /// X coordinate or delta
    pub x: i32,
    /// Y coordinate or delta
    pub y: i32,
    /// Z coordinate (scroll wheel)
    pub z: i32,
    /// Button
    pub button: Option<MouseButton>,
    /// Button states
    #[serde(default)]
    pub buttons: MouseButtons,
}

/// Mouse action
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseAction {
    Move,
    MoveAbsolute,
    ButtonPress,
    ButtonRelease,
    Click,
    Scroll,
}

/// Mouse button
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Side,
    Extra,
}

/// Mouse button states
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct MouseButtons {
    pub left: bool,
    pub right: bool,
    pub middle: bool,
    pub side: bool,
    pub extra: bool,
}
