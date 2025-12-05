//! Audio buffer management

use alloc::vec::Vec;

use crate::{AudioFormat, MAX_CHANNELS};

/// Audio buffer for streaming data
pub struct AudioBuffer {
    /// Raw sample data
    data: Vec<u8>,
    /// Number of frames (samples per channel)
    frames: u32,
    /// Number of channels
    channels: u8,
    /// Sample format
    format: AudioFormat,
}

impl AudioBuffer {
    /// Create new buffer
    pub fn new(frames: u32, channels: u8, format: AudioFormat) -> Self {
        let bytes = frames as usize * channels as usize * format.bytes_per_sample();
        Self {
            data: alloc::vec![0u8; bytes],
            frames,
            channels,
            format,
        }
    }

    /// Get raw data
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Get mutable raw data
    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }

    /// Get number of frames
    pub fn frames(&self) -> u32 {
        self.frames
    }

    /// Get number of channels
    pub fn channels(&self) -> u8 {
        self.channels
    }

    /// Get format
    pub fn format(&self) -> AudioFormat {
        self.format
    }

    /// Get size in bytes
    pub fn size_bytes(&self) -> usize {
        self.data.len()
    }
}

/// Ring buffer for audio streaming
pub struct RingBuffer {
    /// Buffer data
    buffer: Vec<u8>,
    /// Read position
    read_pos: usize,
    /// Write position
    write_pos: usize,
    /// Capacity in bytes
    capacity: usize,
}

impl RingBuffer {
    /// Create new ring buffer
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: alloc::vec![0u8; capacity],
            read_pos: 0,
            write_pos: 0,
            capacity,
        }
    }

    /// Write data to buffer
    pub fn write(&mut self, data: &[u8]) -> usize {
        let available = self.available_write();
        let to_write = data.len().min(available);

        for i in 0..to_write {
            self.buffer[(self.write_pos + i) % self.capacity] = data[i];
        }
        self.write_pos = (self.write_pos + to_write) % self.capacity;

        to_write
    }

    /// Read data from buffer
    pub fn read(&mut self, buf: &mut [u8]) -> usize {
        let available = self.available_read();
        let to_read = buf.len().min(available);

        for i in 0..to_read {
            buf[i] = self.buffer[(self.read_pos + i) % self.capacity];
        }
        self.read_pos = (self.read_pos + to_read) % self.capacity;

        to_read
    }

    /// Available space for writing
    pub fn available_write(&self) -> usize {
        if self.write_pos >= self.read_pos {
            self.capacity - (self.write_pos - self.read_pos) - 1
        } else {
            self.read_pos - self.write_pos - 1
        }
    }

    /// Available data for reading
    pub fn available_read(&self) -> usize {
        if self.write_pos >= self.read_pos {
            self.write_pos - self.read_pos
        } else {
            self.capacity - self.read_pos + self.write_pos
        }
    }

    /// Clear buffer
    pub fn clear(&mut self) {
        self.read_pos = 0;
        self.write_pos = 0;
    }
}
