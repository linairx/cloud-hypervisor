// Copyright 2024 lg-capture Authors
// SPDX-License-Identifier: Apache-2.0

//! Shared memory access module
//!
//! Provides safe access to the IVSHMEM shared memory region.

use std::fs::OpenOptions;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use memmap2::MmapMut;

use crate::protocol::*;

/// Shared memory accessor for IVSHMEM region
pub struct SharedMemory {
    /// Memory mapped region
    mmap: MmapMut,
    /// Header pointer
    header_ptr: *mut FrameBufferHeader,
}

unsafe impl Send for SharedMemory {}
unsafe impl Sync for SharedMemory {}

impl SharedMemory {
    /// Open and map shared memory from a file
    pub fn open<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)?;

        let mmap = unsafe { MmapMut::map_mut(&file)? };

        let header_ptr = mmap.as_ptr() as *mut FrameBufferHeader;

        let shm = Self { mmap, header_ptr };

        // Validate header
        shm.validate()?;

        Ok(shm)
    }

    /// Validate the shared memory header
    pub fn validate(&self) -> io::Result<()> {
        let header = self.header();
        if header.magic != FRAME_BUFFER_MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid magic number",
            ));
        }
        if header.version != FRAME_BUFFER_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Unsupported version: {}", header.version),
            ));
        }
        Ok(())
    }

    /// Get header reference
    pub fn header(&self) -> &FrameBufferHeader {
        unsafe { &*self.header_ptr }
    }

    /// Get mutable header reference
    pub fn header_mut(&mut self) -> &mut FrameBufferHeader {
        unsafe { &mut *self.header_ptr }
    }

    /// Get command from host
    pub fn get_command(&self) -> GuestCommand {
        let header = self.header();
        let cmd = header.command;
        GuestCommand::try_from(cmd).unwrap_or(GuestCommand::None)
    }

    /// Set guest state
    pub fn set_guest_state(&self, state: GuestState) {
        let header_ptr = self.header_ptr;
        unsafe {
            let state_ptr = &raw const (*header_ptr).guest_state as *const AtomicU32;
            (*state_ptr).store(state as u32, Ordering::Release);
        }
    }

    /// Get current active buffer index
    pub fn get_active_index(&self) -> u32 {
        let header_ptr = self.header_ptr;
        unsafe {
            let active_ptr = &raw const (*header_ptr).active_index as *const AtomicU32;
            (*active_ptr).load(Ordering::Acquire)
        }
    }

    /// Get next buffer index (for writing)
    pub fn get_next_index(&self) -> u32 {
        let header = self.header();
        let current = self.get_active_index();
        (current + 1) % header.buffer_count
    }

    /// Get frame count
    pub fn get_frame_count(&self) -> u64 {
        let header_ptr = self.header_ptr;
        unsafe {
            let count_ptr = &raw const (*header_ptr).frame_count as *const AtomicU64;
            (*count_ptr).load(Ordering::Acquire)
        }
    }

    /// Increment frame count
    pub fn increment_frame_count(&self) -> u64 {
        let header_ptr = self.header_ptr;
        unsafe {
            let count_ptr = &raw const (*header_ptr).frame_count as *const AtomicU64;
            (*count_ptr).fetch_add(1, Ordering::AcqRel) + 1
        }
    }

    /// Get buffer data pointer
    pub fn get_buffer_ptr(&mut self, index: u32) -> *mut u8 {
        let header = self.header();
        if index >= header.buffer_count {
            return std::ptr::null_mut();
        }

        // Calculate offset: header + metadata array + buffer offset
        let header_size = std::mem::size_of::<FrameBufferHeader>();
        let metadata_size = header.buffer_count as usize * std::mem::size_of::<FrameMetadata>();
        let buffer_offset = index as usize * header.buffer_size as usize;

        let offset = header_size + metadata_size + buffer_offset;
        unsafe { self.mmap.as_mut_ptr().add(offset) }
    }

    /// Get buffer size
    pub fn get_buffer_size(&self) -> u64 {
        self.header().buffer_size
    }

    /// Get frame metadata pointer
    pub fn get_metadata_ptr(&mut self, index: u32) -> *mut FrameMetadata {
        let header = self.header();
        if index >= header.buffer_count {
            return std::ptr::null_mut();
        }

        let header_size = std::mem::size_of::<FrameBufferHeader>();
        let metadata_offset = index as usize * std::mem::size_of::<FrameMetadata>();

        unsafe {
            self.mmap.as_mut_ptr()
                .add(header_size + metadata_offset) as *mut FrameMetadata
        }
    }

    /// Write frame data to a buffer
    pub fn write_frame(&mut self, index: u32, data: &[u8]) -> io::Result<()> {
        let header = self.header();
        if index >= header.buffer_count {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "Buffer index out of range"));
        }

        let buffer_size = header.buffer_size as usize;
        if data.len() > buffer_size {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "Data too large for buffer"));
        }

        let buffer_ptr = self.get_buffer_ptr(index);
        if buffer_ptr.is_null() {
            return Err(io::Error::new(io::ErrorKind::Other, "Failed to get buffer pointer"));
        }

        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), buffer_ptr, data.len());
        }

        // Update metadata
        let metadata_ptr = self.get_metadata_ptr(index);
        if !metadata_ptr.is_null() {
            unsafe {
                (*metadata_ptr).data_size = data.len() as u32;
                (*metadata_ptr).frame_number = self.get_frame_count();
                (*metadata_ptr).timestamp_ns = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos() as u64;
                (*metadata_ptr).flags = FrameFlags::KEYFRAME;  // bitflags 2.x supports const
            }
        }

        Ok(())
    }

    /// Write frame data from a pointer (zero-copy path)
    /// Directly copy from source pointer to IVSHMEM buffer
    pub fn write_frame_from_ptr(
        &mut self,
        index: u32,
        src_ptr: *const u8,
        size: usize,
    ) -> io::Result<()> {
        let header = self.header();
        if index >= header.buffer_count {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "Buffer index out of range"));
        }

        let buffer_size = header.buffer_size as usize;
        if size > buffer_size {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "Data too large for buffer"));
        }

        if src_ptr.is_null() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "Source pointer is null"));
        }

        let buffer_ptr = self.get_buffer_ptr(index);
        if buffer_ptr.is_null() {
            return Err(io::Error::new(io::ErrorKind::Other, "Failed to get buffer pointer"));
        }

        unsafe {
            std::ptr::copy_nonoverlapping(src_ptr, buffer_ptr, size);
        }

        // Update metadata
        let metadata_ptr = self.get_metadata_ptr(index);
        if !metadata_ptr.is_null() {
            unsafe {
                (*metadata_ptr).data_size = size as u32;
                (*metadata_ptr).frame_number = self.get_frame_count();
                (*metadata_ptr).timestamp_ns = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos() as u64;
                (*metadata_ptr).flags = FrameFlags::KEYFRAME;
            }
        }

        Ok(())
    }

    /// Get direct pointer to IVSHMEM buffer for zero-copy write
    /// Returns a mutable pointer that can be written to directly
    pub fn get_buffer_ptr_for_write(&mut self, index: u32) -> io::Result<*mut u8> {
        let header = self.header();
        if index >= header.buffer_count {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "Buffer index out of range"));
        }

        let buffer_ptr = self.get_buffer_ptr(index);
        if buffer_ptr.is_null() {
            return Err(io::Error::new(io::ErrorKind::Other, "Failed to get buffer pointer"));
        }

        Ok(buffer_ptr)
    }

    /// Finalize frame after direct write
    pub fn finalize_frame(&mut self, index: u32, size: usize) -> io::Result<()> {
        let metadata_ptr = self.get_metadata_ptr(index);
        if metadata_ptr.is_null() {
            return Err(io::Error::new(io::ErrorKind::Other, "Failed to get metadata pointer"));
        }

        unsafe {
            (*metadata_ptr).data_size = size as u32;
            (*metadata_ptr).frame_number = self.get_frame_count();
            (*metadata_ptr).timestamp_ns = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos() as u64;
            (*metadata_ptr).flags = FrameFlags::KEYFRAME;
        }

        Ok(())
    }

    /// Commit a frame (update active index)
    pub fn commit_frame(&self, index: u32) {
        let header_ptr = self.header_ptr;
        let buffer_count = unsafe { (*header_ptr).buffer_count };

        unsafe {
            let active_ptr = &raw const (*header_ptr).active_index as *const AtomicU32;
            (*active_ptr).store(index % buffer_count, Ordering::Release);
        }

        self.increment_frame_count();
    }

    /// Get cursor data pointer
    pub fn get_cursor_ptr(&mut self) -> *mut u8 {
        let cursor_offset = self.header().cursor_offset;
        if cursor_offset == 0 {
            return std::ptr::null_mut();
        }
        unsafe { self.mmap.as_mut_ptr().add(cursor_offset as usize) }
    }

    /// Write cursor shape data
    pub fn write_cursor_shape(&mut self, data: &[u8], info: &CursorShapeInfo) -> io::Result<()> {
        let header = self.header();
        if header.cursor_offset == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Cursor data not allocated",
            ));
        }

        if data.len() > MAX_CURSOR_SIZE as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Cursor data too large",
            ));
        }

        // Write shape info
        let cursor_ptr = self.get_cursor_ptr();
        unsafe {
            let shape_info_ptr = cursor_ptr as *mut CursorShapeInfo;
            std::ptr::write(shape_info_ptr, *info);

            // Write cursor data after shape info
            let data_ptr = cursor_ptr.add(std::mem::size_of::<CursorShapeInfo>());
            std::ptr::copy_nonoverlapping(data.as_ptr(), data_ptr, data.len());
        }

        // Signal update
        let header_ptr = self.header_ptr;
        unsafe {
            let updated_ptr = &raw const (*header_ptr).cursor_updated as *const AtomicU32;
            (*updated_ptr).fetch_add(1, Ordering::AcqRel);
        }

        Ok(())
    }
}
