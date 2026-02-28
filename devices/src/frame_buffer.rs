// Copyright 2024 Tencent Corporation. All rights reserved.
//
// SPDX-License-Identifier: Apache-2.0
//
// Frame buffer data structures for IVSHMEM-based lg-capture functionality.
// Supports multi-buffering (triple buffering) for efficient frame data sharing
// between host and guest via shared memory.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Guest Agent 命令（Host -> Guest）
/// 由 Host 写入，Guest Agent 读取并执行
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestCommand {
    /// 无命令/空闲状态
    None = 0,
    /// 开始捕获帧数据
    StartCapture = 1,
    /// 停止捕获帧数据
    StopCapture = 2,
    /// 设置帧格式（需要参数）
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

/// Guest Agent 状态（Guest -> Host）
/// 由 Guest Agent 写入，Host 读取
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestState {
    /// 空闲状态，未捕获
    Idle = 0,
    /// 正在捕获帧数据
    Capturing = 1,
    /// 错误状态
    Error = 2,
    /// 正在初始化
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

/// Magic number for frame buffer header validation: "FBMP" (Frame Buffer Multi-buffer Protocol)
pub const FRAME_BUFFER_MAGIC: u32 = 0x46424D50;

/// Current version of the frame buffer protocol
pub const FRAME_BUFFER_VERSION: u32 = 1;

/// Default number of buffers (triple buffering)
pub const DEFAULT_BUFFER_COUNT: u32 = 3;

/// Frame format enumeration
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

bitflags::bitflags! {
    /// Frame metadata flags
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
        FrameFlags::NONE
    }
}

/// Cursor metadata (placed after frame buffers)
///
/// This structure contains cursor position and visibility information.
/// The actual cursor shape data is stored separately after this structure.
#[repr(C)]
#[derive(Debug, Clone, Default)]
pub struct CursorMetadata {
    /// Cursor X position in pixels
    pub x: i32,
    /// Cursor Y position in pixels
    pub y: i32,
    /// Whether the cursor is visible (0 = hidden, 1 = visible)
    pub visible: u32,
    /// Whether the cursor shape has been updated (0 = no change, 1 = updated)
    pub shape_updated: u32,
    /// Reserved for future use
    pub reserved: [u8; 16],
}

/// Cursor shape information
///
/// This structure describes the cursor shape dimensions and hotspot.
#[repr(C)]
#[derive(Debug, Clone, Default)]
pub struct CursorShapeInfo {
    /// Cursor width in pixels
    pub width: u16,
    /// Cursor height in pixels
    pub height: u16,
    /// Cursor hotspot X offset (relative to top-left)
    pub hot_x: i16,
    /// Cursor hotspot Y offset (relative to top-left)
    pub hot_y: i16,
    /// Cursor data size in bytes (width * height * 4 for BGRA)
    pub data_size: u32,
    /// Reserved for future use
    pub reserved: [u8; 20],
}

/// Frame buffer header (fixed size, placed at the beginning of shared memory)
///
/// Memory layout:
/// ```text
/// +------------------+
/// | FrameBufferHeader|  (80 bytes, cache-line aligned)
/// +------------------+
/// | FrameMetadata[0] |  (40 bytes each)
/// | FrameMetadata[1] |
/// | FrameMetadata[2] |
/// | ...              |
/// +------------------+
/// | Buffer[0] data   |  (buffer_size bytes each)
/// | Buffer[1] data   |
/// | Buffer[2] data   |
/// | ...              |
/// +------------------+
/// | CursorMetadata   |  (32 bytes)
/// +------------------+
/// | CursorShapeInfo  |  (32 bytes)
/// +------------------+
/// | Cursor data      |  (cursor_size bytes, max 64KB)
/// +------------------+
/// ```
#[repr(C)]
#[derive(Debug)]
pub struct FrameBufferHeader {
    /// Magic number for validation: 0x46424D50 ("FBMP")
    pub magic: u32,
    /// Protocol version number
    pub version: u32,
    /// Number of buffers in the ring
    pub buffer_count: u32,
    /// Size of each buffer in bytes
    pub buffer_size: u64,
    /// Frame width in pixels
    pub frame_width: u32,
    /// Frame height in pixels
    pub frame_height: u32,
    /// Pixel format
    pub format: FrameFormat,
    /// Current active buffer index (atomic for lock-free access)
    pub active_index: AtomicU32,
    /// Total frame count (atomic for lock-free access)
    pub frame_count: AtomicU64,
    /// Host 写入的命令，Guest Agent 读取执行
    pub command: AtomicU32,
    /// Guest Agent 写入的状态，Host 读取
    pub guest_state: AtomicU32,
    /// Guest Agent PID（用于调试）
    pub guest_pid: AtomicU32,
    /// Cursor data offset from the start of shared memory (0 = no cursor data)
    pub cursor_offset: u64,
    /// Cursor data size in bytes (max 64KB)
    pub cursor_size: u32,
    /// Cursor hotspot X
    pub cursor_hot_x: i16,
    /// Cursor hotspot Y
    pub cursor_hot_y: i16,
    /// Cursor width in pixels
    pub cursor_width: u16,
    /// Cursor height in pixels
    pub cursor_height: u16,
    /// Cursor update flag (incremented when cursor shape or position changes)
    pub cursor_updated: AtomicU32,
}

/// Maximum cursor data size (64KB - enough for 128x128 BGRA cursor)
pub const MAX_CURSOR_SIZE: u32 = 64 * 1024;

impl FrameBufferHeader {
    /// Creates a new frame buffer header with the given parameters
    pub fn new(
        buffer_count: u32,
        buffer_size: u64,
        frame_width: u32,
        frame_height: u32,
        format: FrameFormat,
    ) -> Self {
        FrameBufferHeader {
            magic: FRAME_BUFFER_MAGIC,
            version: FRAME_BUFFER_VERSION,
            buffer_count,
            buffer_size,
            frame_width,
            frame_height,
            format,
            active_index: AtomicU32::new(0),
            frame_count: AtomicU64::new(0),
            command: AtomicU32::new(GuestCommand::None as u32),
            guest_state: AtomicU32::new(GuestState::Idle as u32),
            guest_pid: AtomicU32::new(0),
            cursor_offset: 0,
            cursor_size: 0,
            cursor_hot_x: 0,
            cursor_hot_y: 0,
            cursor_width: 0,
            cursor_height: 0,
            cursor_updated: AtomicU32::new(0),
        }
    }

    /// Validates the header magic number and version
    pub fn validate(&self) -> bool {
        self.magic == FRAME_BUFFER_MAGIC && self.version == FRAME_BUFFER_VERSION
    }

    /// Gets the current active buffer index
    pub fn active_index(&self) -> u32 {
        self.active_index.load(Ordering::Acquire)
    }

    /// Sets the active buffer index
    pub fn set_active_index(&self, index: u32) {
        self.active_index.store(index, Ordering::Release);
    }

    /// Gets the total frame count
    pub fn frame_count(&self) -> u64 {
        self.frame_count.load(Ordering::Acquire)
    }

    /// Increments and returns the new frame count
    pub fn increment_frame_count(&self) -> u64 {
        self.frame_count.fetch_add(1, Ordering::AcqRel) + 1
    }

    // ========== 命令操作（Host 写入，Guest 读取）==========

    /// Gets the current command (for Guest Agent to read)
    pub fn get_command(&self) -> GuestCommand {
        GuestCommand::try_from(self.command.load(Ordering::Acquire))
            .unwrap_or(GuestCommand::None)
    }

    /// Sets the command (for Host to write)
    pub fn set_command(&self, command: GuestCommand) {
        self.command.store(command as u32, Ordering::Release);
    }

    /// Clears the command (sets to None)
    pub fn clear_command(&self) {
        self.command.store(GuestCommand::None as u32, Ordering::Release);
    }

    // ========== 状态操作（Guest 写入，Host 读取）==========

    /// Gets the current guest state (for Host to read)
    pub fn get_guest_state(&self) -> GuestState {
        GuestState::try_from(self.guest_state.load(Ordering::Acquire))
            .unwrap_or(GuestState::Idle)
    }

    /// Sets the guest state (for Guest Agent to write)
    pub fn set_guest_state(&self, state: GuestState) {
        self.guest_state.store(state as u32, Ordering::Release);
    }

    // ========== PID 操作（用于调试）==========

    /// Gets the guest agent PID
    pub fn get_guest_pid(&self) -> u32 {
        self.guest_pid.load(Ordering::Acquire)
    }

    /// Sets the guest agent PID
    pub fn set_guest_pid(&self, pid: u32) {
        self.guest_pid.store(pid, Ordering::Release);
    }

    // ========== 帧写入协议（Guest Agent 使用）==========

    /// 开始写入新帧（Guest Agent 调用）
    /// 返回下一个可写入的缓冲区索引
    pub fn begin_write_frame(&self) -> u32 {
        // 获取下一个缓冲区（跳过当前活跃的）
        let current = self.active_index();
        let next = (current + 1) % self.buffer_count;
        next
    }

    /// 完成帧写入（Guest Agent 调用）
    /// 更新 active_index 和 frame_count
    pub fn end_write_frame(&self, buffer_index: u32) -> u64 {
        // 先更新 active_index，再递增 frame_count
        // 这确保 Host 在看到新的 frame_count 时，active_index 已经更新
        self.set_active_index(buffer_index);
        self.increment_frame_count()
    }

    // ========== 帧读取协议（Host 使用）==========

    /// 读取当前帧信息（Host 调用）
    /// 返回 (active_index, frame_count)
    pub fn read_frame_info(&self) -> (u32, u64) {
        // 先读取 frame_count，再读取 active_index
        // 这确保如果 Guest 在此期间写入新帧，我们可能会看到：
        // - 旧的 frame_count + 旧的 active_index（安全）
        // - 新的 frame_count + 新的 active_index（安全）
        // - 新的 frame_count + 旧的 active_index（可能错过一帧，但不会读到不一致数据）
        let count = self.frame_count();
        let index = self.active_index();
        (index, count)
    }

    /// Calculates the stride (bytes per row) for the given format
    pub fn stride(&self) -> u64 {
        match self.format {
            FrameFormat::Bgra32 | FrameFormat::Rgba32 => self.frame_width as u64 * 4,
            FrameFormat::Nv12 => self.frame_width as u64,
        }
    }

    /// Calculates the expected data size for the current frame dimensions and format
    pub fn expected_data_size(&self) -> u64 {
        match self.format {
            FrameFormat::Bgra32 | FrameFormat::Rgba32 => {
                self.frame_width as u64 * self.frame_height as u64 * 4
            }
            FrameFormat::Nv12 => {
                // Y plane + UV plane (half resolution)
                let y_size = self.frame_width as u64 * self.frame_height as u64;
                let uv_size = (self.frame_width as u64 / 2) * (self.frame_height as u64 / 2) * 2;
                y_size + uv_size
            }
        }
    }

    // ========== 光标操作（Guest Agent 写入，Host 读取）==========

    /// Gets the cursor update counter
    pub fn cursor_update_count(&self) -> u32 {
        self.cursor_updated.load(Ordering::Acquire)
    }

    /// Writes cursor shape information (Guest Agent calls this)
    /// This only updates the shape metadata, not the actual pixel data.
    /// The caller must also write the pixel data to the cursor data region.
    ///
    /// # Arguments
    /// * `width` - Cursor width in pixels
    /// * `height` - Cursor height in pixels
    /// * `hot_x` - Hotspot X offset
    /// * `hot_y` - Hotspot Y offset
    /// * `data_size` - Size of cursor pixel data in bytes
    ///
    /// # Returns
    /// The cursor data offset where pixel data should be written
    pub fn set_cursor_shape_info(
        &self,
        width: u16,
        height: u16,
        hot_x: i16,
        hot_y: i16,
        data_size: u32,
    ) {
        // SAFETY: These are plain fields, not accessed via atomics
        // We use a pointer to write multiple fields atomically from the guest side
        // In practice, the guest should ensure it's not racing with itself
        let header = self as *const FrameBufferHeader as *mut FrameBufferHeader;

        // Write fields
        unsafe {
            (*header).cursor_width = width;
            (*header).cursor_height = height;
            (*header).cursor_hot_x = hot_x;
            (*header).cursor_hot_y = hot_y;
            (*header).cursor_size = data_size.min(MAX_CURSOR_SIZE);
        }

        // Increment update counter to signal change
        self.cursor_updated.fetch_add(1, Ordering::AcqRel);
    }

    /// Sets the cursor data offset (called during layout initialization)
    pub fn set_cursor_offset(&self, offset: u64) {
        // SAFETY: This is called once during initialization
        let header = self as *const FrameBufferHeader as *mut FrameBufferHeader;
        unsafe {
            (*header).cursor_offset = offset;
        }
    }

    /// Gets cursor shape information
    pub fn get_cursor_shape(&self) -> CursorShapeInfo {
        CursorShapeInfo {
            width: self.cursor_width,
            height: self.cursor_height,
            hot_x: self.cursor_hot_x,
            hot_y: self.cursor_hot_y,
            data_size: self.cursor_size,
            reserved: [0u8; 20],
        }
    }

    /// Checks if cursor data is available
    pub fn has_cursor_data(&self) -> bool {
        self.cursor_offset > 0 && self.cursor_size > 0
    }
}

/// Frame metadata (one per buffer)
#[repr(C)]
#[derive(Debug, Clone, Default)]
pub struct FrameMetadata {
    /// Frame sequence number
    pub frame_number: u64,
    /// Timestamp in nanoseconds (monotonic clock)
    pub timestamp_ns: u64,
    /// Frame flags
    pub flags: u32,
    /// Actual data size in bytes
    pub data_size: u32,
    /// Reserved for future use
    pub reserved: [u8; 16],
}

impl FrameMetadata {
    /// Creates a new frame metadata with the given parameters
    pub fn new(frame_number: u64, timestamp_ns: u64, data_size: u32, flags: FrameFlags) -> Self {
        FrameMetadata {
            frame_number,
            timestamp_ns,
            flags: flags.bits(),
            data_size,
            reserved: [0u8; 16],
        }
    }

    /// Checks if this metadata has been initialized (non-zero frame number or timestamp)
    pub fn is_initialized(&self) -> bool {
        self.frame_number != 0 || self.timestamp_ns != 0
    }

    /// Gets the frame flags
    pub fn flags(&self) -> FrameFlags {
        FrameFlags::from_bits_truncate(self.flags)
    }
}

/// Frame buffer layout calculator
#[derive(Debug, Clone)]
pub struct FrameBufferLayout {
    /// Offset to the header (always 0)
    pub header_offset: usize,
    /// Offset to the metadata array
    pub metadata_offset: usize,
    /// Offset to the data buffers
    pub data_offset: usize,
    /// Offset to the cursor metadata
    pub cursor_metadata_offset: usize,
    /// Offset to the cursor shape info
    pub cursor_shape_offset: usize,
    /// Offset to the cursor data
    pub cursor_data_offset: usize,
    /// Total size of the frame buffer region
    pub total_size: usize,
    /// Number of buffers
    pub buffer_count: u32,
    /// Size of each buffer
    pub buffer_size: u64,
}

impl FrameBufferLayout {
    /// Size of the header in bytes (actual struct size with alignment)
    pub const HEADER_SIZE: usize = 88;

    /// Size of each metadata entry in bytes (actual struct size with alignment)
    pub const METADATA_SIZE: usize = 40;

    /// Size of cursor metadata in bytes
    pub const CURSOR_METADATA_SIZE: usize = 32;

    /// Size of cursor shape info in bytes
    pub const CURSOR_SHAPE_SIZE: usize = 32;

    /// Maximum cursor data size (64KB)
    pub const CURSOR_DATA_SIZE: usize = MAX_CURSOR_SIZE as usize;

    /// Creates a new layout calculator with the given buffer configuration
    pub fn new(buffer_count: u32, buffer_size: u64) -> Self {
        // Ensure alignment to 64-byte cache line
        const CACHE_LINE_SIZE: usize = 64;

        let header_offset = 0;
        let metadata_offset = Self::HEADER_SIZE;

        // Align data section to cache line
        let metadata_total = Self::METADATA_SIZE * buffer_count as usize;
        let data_offset = (metadata_offset + metadata_total + CACHE_LINE_SIZE - 1)
            & !(CACHE_LINE_SIZE - 1);

        let data_total = buffer_size as usize * buffer_count as usize;

        // Cursor region starts after frame data, aligned to cache line
        let cursor_metadata_offset = (data_offset + data_total + CACHE_LINE_SIZE - 1)
            & !(CACHE_LINE_SIZE - 1);
        let cursor_shape_offset = cursor_metadata_offset + Self::CURSOR_METADATA_SIZE;
        let cursor_data_offset = cursor_shape_offset + Self::CURSOR_SHAPE_SIZE;

        // Total size includes cursor data region
        let total_size = cursor_data_offset + Self::CURSOR_DATA_SIZE;

        FrameBufferLayout {
            header_offset,
            metadata_offset,
            data_offset,
            cursor_metadata_offset,
            cursor_shape_offset,
            cursor_data_offset,
            total_size,
            buffer_count,
            buffer_size,
        }
    }

    /// Creates a layout from a header
    pub fn from_header(header: &FrameBufferHeader) -> Self {
        Self::new(header.buffer_count, header.buffer_size)
    }

    /// Calculates the offset to a specific metadata entry
    pub fn metadata_offset_for(&self, index: u32) -> usize {
        assert!(index < self.buffer_count, "Buffer index out of range");
        self.metadata_offset + (index as usize * Self::METADATA_SIZE)
    }

    /// Calculates the offset to a specific data buffer
    pub fn data_offset_for(&self, index: u32) -> usize {
        assert!(index < self.buffer_count, "Buffer index out of range");
        self.data_offset + (index as usize * self.buffer_size as usize)
    }

    /// Gets the next buffer index (wraps around)
    pub fn next_index(&self, current: u32) -> u32 {
        (current + 1) % self.buffer_count
    }

    /// Validates that the given region size is sufficient for this layout
    pub fn validate_region_size(&self, region_size: usize) -> bool {
        region_size >= self.total_size
    }

    /// Gets the cursor metadata offset
    pub fn cursor_metadata_offset_for(&self) -> usize {
        self.cursor_metadata_offset
    }

    /// Gets the cursor shape info offset
    pub fn cursor_shape_offset_for(&self) -> usize {
        self.cursor_shape_offset
    }

    /// Gets the cursor data offset
    pub fn cursor_data_offset_for(&self) -> usize {
        self.cursor_data_offset
    }
}

// ============================================================================
// Audio Support (Phase 6.5)
// ============================================================================

/// Audio format enumeration
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

impl TryFrom<u32> for AudioFormat {
    type Error = &'static str;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(AudioFormat::PcmS16Le),
            1 => Ok(AudioFormat::PcmS24Le),
            2 => Ok(AudioFormat::PcmS32Le),
            3 => Ok(AudioFormat::FloatLe),
            _ => Err("Invalid audio format value"),
        }
    }
}

impl AudioFormat {
    /// Returns the number of bytes per sample
    pub fn bytes_per_sample(&self) -> u8 {
        match self {
            AudioFormat::PcmS16Le => 2,
            AudioFormat::PcmS24Le => 3,
            AudioFormat::PcmS32Le => 4,
            AudioFormat::FloatLe => 4,
        }
    }
}

/// Audio buffer header (placed after cursor data in shared memory)
#[repr(C)]
#[derive(Debug)]
pub struct AudioBufferHeader {
    /// Magic number for validation: 0x41554449 ("AUDI")
    pub magic: u32,
    /// Version number
    pub version: u32,
    /// Audio format
    pub format: AudioFormat,
    /// Sample rate in Hz (e.g., 48000)
    pub sample_rate: u32,
    /// Number of channels (1 = mono, 2 = stereo)
    pub channels: u8,
    /// Reserved
    pub reserved: [u8; 3],
    /// Ring buffer size in bytes
    pub buffer_size: u32,
    /// Write position (atomic, updated by guest)
    pub write_pos: AtomicU32,
    /// Read position (atomic, updated by host)
    pub read_pos: AtomicU32,
    /// Total bytes written
    pub total_written: AtomicU64,
    /// Audio stream active flag
    pub active: AtomicU32,
    /// Reserved for future use
    pub reserved2: [u8; 32],
}

/// Audio buffer magic number: "AUDI"
pub const AUDIO_BUFFER_MAGIC: u32 = 0x41554449;
/// Audio buffer version
pub const AUDIO_BUFFER_VERSION: u32 = 1;
/// Default audio buffer size (1MB)
pub const DEFAULT_AUDIO_BUFFER_SIZE: u32 = 1024 * 1024;
/// Default sample rate
pub const DEFAULT_SAMPLE_RATE: u32 = 48000;
/// Default channels
pub const DEFAULT_CHANNELS: u8 = 2;

impl AudioBufferHeader {
    /// Creates a new audio buffer header
    pub fn new(format: AudioFormat, sample_rate: u32, channels: u8, buffer_size: u32) -> Self {
        AudioBufferHeader {
            magic: AUDIO_BUFFER_MAGIC,
            version: AUDIO_BUFFER_VERSION,
            format,
            sample_rate,
            channels,
            reserved: [0; 3],
            buffer_size,
            write_pos: AtomicU32::new(0),
            read_pos: AtomicU32::new(0),
            total_written: AtomicU64::new(0),
            active: AtomicU32::new(0),
            reserved2: [0; 32],
        }
    }

    /// Validates the header
    pub fn validate(&self) -> bool {
        self.magic == AUDIO_BUFFER_MAGIC && self.version == AUDIO_BUFFER_VERSION
    }

    /// Returns available bytes to read
    pub fn available_to_read(&self) -> u32 {
        let write = self.write_pos.load(Ordering::Acquire);
        let read = self.read_pos.load(Ordering::Acquire);
        if write >= read {
            write - read
        } else {
            self.buffer_size - read + write
        }
    }

    /// Returns available space to write
    pub fn available_to_write(&self) -> u32 {
        self.buffer_size - self.available_to_read() - 1 // -1 to distinguish full from empty
    }

    /// Returns the total layout size including header and buffer
    pub fn total_layout_size(&self) -> usize {
        std::mem::size_of::<AudioBufferHeader>() + self.buffer_size as usize
    }
}

impl Clone for AudioBufferHeader {
    fn clone(&self) -> Self {
        Self {
            magic: self.magic,
            version: self.version,
            format: self.format,
            sample_rate: self.sample_rate,
            channels: self.channels,
            reserved: self.reserved,
            buffer_size: self.buffer_size,
            write_pos: AtomicU32::new(self.write_pos.load(Ordering::Acquire)),
            read_pos: AtomicU32::new(self.read_pos.load(Ordering::Acquire)),
            total_written: AtomicU64::new(self.total_written.load(Ordering::Acquire)),
            active: AtomicU32::new(self.active.load(Ordering::Acquire)),
            reserved2: self.reserved2,
        }
    }
}

impl Default for AudioBufferHeader {
    fn default() -> Self {
        Self::new(
            AudioFormat::PcmS16Le,
            DEFAULT_SAMPLE_RATE,
            DEFAULT_CHANNELS,
            DEFAULT_AUDIO_BUFFER_SIZE,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frame_format_conversion() {
        assert_eq!(FrameFormat::try_from(0).unwrap(), FrameFormat::Bgra32);
        assert_eq!(FrameFormat::try_from(1).unwrap(), FrameFormat::Rgba32);
        assert_eq!(FrameFormat::try_from(2).unwrap(), FrameFormat::Nv12);
        assert!(FrameFormat::try_from(3).is_err());
    }

    #[test]
    fn test_frame_buffer_header_validation() {
        let header = FrameBufferHeader::new(3, 1920 * 1080 * 4, 1920, 1080, FrameFormat::Bgra32);
        assert!(header.validate());
        assert_eq!(header.magic, FRAME_BUFFER_MAGIC);
        assert_eq!(header.version, FRAME_BUFFER_VERSION);
    }

    #[test]
    fn test_frame_buffer_header_stride() {
        let header = FrameBufferHeader::new(3, 1920 * 1080 * 4, 1920, 1080, FrameFormat::Bgra32);
        assert_eq!(header.stride(), 1920 * 4);

        let header_nv12 = FrameBufferHeader::new(3, 1920 * 1080 * 3 / 2, 1920, 1080, FrameFormat::Nv12);
        assert_eq!(header_nv12.stride(), 1920);
    }

    #[test]
    fn test_frame_buffer_header_expected_size() {
        let header = FrameBufferHeader::new(3, 1920 * 1080 * 4, 1920, 1080, FrameFormat::Bgra32);
        assert_eq!(header.expected_data_size(), 1920 * 1080 * 4);

        let header_nv12 = FrameBufferHeader::new(3, 1920 * 1080 * 3 / 2, 1920, 1080, FrameFormat::Nv12);
        // NV12: Y plane (1920*1080) + UV plane (960*540*2)
        assert_eq!(header_nv12.expected_data_size(), 1920 * 1080 + 960 * 540 * 2);
    }

    #[test]
    fn test_frame_buffer_layout_basic() {
        let layout = FrameBufferLayout::new(3, 1920 * 1080 * 4);

        assert_eq!(layout.header_offset, 0);
        assert!(layout.metadata_offset > 0);
        assert!(layout.data_offset > layout.metadata_offset);
        assert!(layout.total_size > layout.data_offset);

        // Verify sizes
        assert_eq!(layout.buffer_count, 3);
        assert_eq!(layout.buffer_size, 1920 * 1080 * 4);
    }

    #[test]
    fn test_frame_buffer_layout_offsets() {
        let layout = FrameBufferLayout::new(3, 1024);

        // Metadata offsets
        let meta0 = layout.metadata_offset_for(0);
        let meta1 = layout.metadata_offset_for(1);
        let meta2 = layout.metadata_offset_for(2);
        assert_eq!(meta1 - meta0, FrameBufferLayout::METADATA_SIZE);
        assert_eq!(meta2 - meta1, FrameBufferLayout::METADATA_SIZE);

        // Data offsets
        let data0 = layout.data_offset_for(0);
        let data1 = layout.data_offset_for(1);
        let data2 = layout.data_offset_for(2);
        assert_eq!(data1 - data0, 1024);
        assert_eq!(data2 - data1, 1024);
    }

    #[test]
    fn test_frame_buffer_layout_next_index() {
        let layout = FrameBufferLayout::new(3, 1024);

        assert_eq!(layout.next_index(0), 1);
        assert_eq!(layout.next_index(1), 2);
        assert_eq!(layout.next_index(2), 0); // Wraps around
    }

    #[test]
    fn test_frame_buffer_layout_validate_region_size() {
        let layout = FrameBufferLayout::new(3, 1024);

        assert!(layout.validate_region_size(layout.total_size));
        assert!(layout.validate_region_size(layout.total_size + 1000));
        assert!(!layout.validate_region_size(layout.total_size - 1));
    }

    #[test]
    fn test_frame_metadata() {
        let meta = FrameMetadata::new(1, 12345678, 1024, FrameFlags::KEYFRAME);

        assert_eq!(meta.frame_number, 1);
        assert_eq!(meta.timestamp_ns, 12345678);
        assert_eq!(meta.data_size, 1024);
        assert!(meta.is_initialized());
        assert!(meta.flags().contains(FrameFlags::KEYFRAME));
    }

    #[test]
    fn test_frame_flags() {
        let flags = FrameFlags::KEYFRAME | FrameFlags::PROCESSED;
        assert!(flags.contains(FrameFlags::KEYFRAME));
        assert!(flags.contains(FrameFlags::PROCESSED));
        assert!(!flags.contains(FrameFlags::EOS));

        let default_flags = FrameFlags::default();
        assert_eq!(default_flags, FrameFlags::NONE);
    }

    #[test]
    fn test_atomic_operations() {
        let header = FrameBufferHeader::new(3, 1024, 100, 100, FrameFormat::Bgra32);

        assert_eq!(header.active_index(), 0);
        header.set_active_index(2);
        assert_eq!(header.active_index(), 2);

        assert_eq!(header.frame_count(), 0);
        assert_eq!(header.increment_frame_count(), 1);
        assert_eq!(header.frame_count(), 1);
        assert_eq!(header.increment_frame_count(), 2);
        assert_eq!(header.frame_count(), 2);
    }

    #[test]
    #[should_panic(expected = "Buffer index out of range")]
    fn test_layout_metadata_offset_out_of_range() {
        let layout = FrameBufferLayout::new(3, 1024);
        layout.metadata_offset_for(3);
    }

    #[test]
    #[should_panic(expected = "Buffer index out of range")]
    fn test_layout_data_offset_out_of_range() {
        let layout = FrameBufferLayout::new(3, 1024);
        layout.data_offset_for(3);
    }

    #[test]
    fn test_layout_from_header() {
        let header = FrameBufferHeader::new(5, 2048, 640, 480, FrameFormat::Rgba32);
        let layout = FrameBufferLayout::from_header(&header);

        assert_eq!(layout.buffer_count, 5);
        assert_eq!(layout.buffer_size, 2048);
    }

    #[test]
    fn test_sizes_are_correct() {
        // Verify struct sizes match expectations
        assert_eq!(std::mem::size_of::<FrameBufferHeader>(), FrameBufferLayout::HEADER_SIZE);
        assert_eq!(std::mem::size_of::<FrameMetadata>(), FrameBufferLayout::METADATA_SIZE);
    }

    #[test]
    fn test_guest_command_conversion() {
        assert_eq!(GuestCommand::try_from(0).unwrap(), GuestCommand::None);
        assert_eq!(GuestCommand::try_from(1).unwrap(), GuestCommand::StartCapture);
        assert_eq!(GuestCommand::try_from(2).unwrap(), GuestCommand::StopCapture);
        assert_eq!(GuestCommand::try_from(3).unwrap(), GuestCommand::SetFormat);
        assert!(GuestCommand::try_from(4).is_err());
    }

    #[test]
    fn test_guest_state_conversion() {
        assert_eq!(GuestState::try_from(0).unwrap(), GuestState::Idle);
        assert_eq!(GuestState::try_from(1).unwrap(), GuestState::Capturing);
        assert_eq!(GuestState::try_from(2).unwrap(), GuestState::Error);
        assert_eq!(GuestState::try_from(3).unwrap(), GuestState::Initializing);
        assert!(GuestState::try_from(4).is_err());
    }

    #[test]
    fn test_command_state_operations() {
        let header = FrameBufferHeader::new(3, 1024, 100, 100, FrameFormat::Bgra32);

        // Test command operations
        assert_eq!(header.get_command(), GuestCommand::None);
        header.set_command(GuestCommand::StartCapture);
        assert_eq!(header.get_command(), GuestCommand::StartCapture);
        header.clear_command();
        assert_eq!(header.get_command(), GuestCommand::None);

        // Test state operations
        assert_eq!(header.get_guest_state(), GuestState::Idle);
        header.set_guest_state(GuestState::Capturing);
        assert_eq!(header.get_guest_state(), GuestState::Capturing);
        header.set_guest_state(GuestState::Error);
        assert_eq!(header.get_guest_state(), GuestState::Error);

        // Test PID operations
        assert_eq!(header.get_guest_pid(), 0);
        header.set_guest_pid(12345);
        assert_eq!(header.get_guest_pid(), 12345);
    }

    #[test]
    fn test_frame_write_protocol() {
        let header = FrameBufferHeader::new(3, 1024, 100, 100, FrameFormat::Bgra32);

        // Initial state
        assert_eq!(header.active_index(), 0);
        assert_eq!(header.frame_count(), 0);

        // Begin write frame should return next buffer
        let next_buf = header.begin_write_frame();
        assert_eq!(next_buf, 1); // (0 + 1) % 3 = 1

        // End write frame should update active_index and frame_count
        let new_count = header.end_write_frame(1);
        assert_eq!(new_count, 1);
        assert_eq!(header.active_index(), 1);
        assert_eq!(header.frame_count(), 1);

        // Next frame
        let next_buf = header.begin_write_frame();
        assert_eq!(next_buf, 2); // (1 + 1) % 3 = 2
        let new_count = header.end_write_frame(2);
        assert_eq!(new_count, 2);

        // Wrap around
        let next_buf = header.begin_write_frame();
        assert_eq!(next_buf, 0); // (2 + 1) % 3 = 0
        let new_count = header.end_write_frame(0);
        assert_eq!(new_count, 3);
        assert_eq!(header.active_index(), 0);
    }

    #[test]
    fn test_frame_read_protocol() {
        let header = FrameBufferHeader::new(3, 1024, 100, 100, FrameFormat::Bgra32);

        // Initial read
        let (index, count) = header.read_frame_info();
        assert_eq!(index, 0);
        assert_eq!(count, 0);

        // Simulate Guest writing a frame
        header.end_write_frame(1);

        // Read after write
        let (index, count) = header.read_frame_info();
        assert_eq!(index, 1);
        assert_eq!(count, 1);
    }
}
