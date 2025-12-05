//! Audio stream management

use alloc::string::String;

use crate::{AudioConfig, AudioError};
use crate::buffer::RingBuffer;

/// Stream state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamState {
    /// Stream is stopped
    Stopped,
    /// Stream is running
    Running,
    /// Stream is paused
    Paused,
    /// Stream has error
    Error,
}

/// Audio stream direction
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamDirection {
    /// Playback (output)
    Playback,
    /// Capture (input)
    Capture,
}

/// Audio stream
pub struct AudioStream {
    /// Stream ID
    id: u64,
    /// Stream name
    name: String,
    /// Stream configuration
    config: AudioConfig,
    /// Stream direction
    direction: StreamDirection,
    /// Current state
    state: StreamState,
    /// Ring buffer for data
    buffer: RingBuffer,
    /// Frames written
    frames_written: u64,
    /// Frames read
    frames_read: u64,
}

impl AudioStream {
    /// Create new stream
    pub fn new(
        id: u64,
        name: String,
        config: AudioConfig,
        direction: StreamDirection,
    ) -> Self {
        // Calculate buffer size in bytes
        let buffer_bytes = config.buffer_size as usize
            * config.channels as usize
            * config.format.bytes_per_sample()
            * 4; // 4 periods

        Self {
            id,
            name,
            config,
            direction,
            state: StreamState::Stopped,
            buffer: RingBuffer::new(buffer_bytes),
            frames_written: 0,
            frames_read: 0,
        }
    }

    /// Get stream ID
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Get stream name
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get configuration
    pub fn config(&self) -> &AudioConfig {
        &self.config
    }

    /// Get direction
    pub fn direction(&self) -> StreamDirection {
        self.direction
    }

    /// Get current state
    pub fn state(&self) -> StreamState {
        self.state
    }

    /// Start stream
    pub fn start(&mut self) -> Result<(), AudioError> {
        if self.state == StreamState::Running {
            return Ok(());
        }
        self.state = StreamState::Running;
        log::debug!("Stream {} started", self.id);
        Ok(())
    }

    /// Stop stream
    pub fn stop(&mut self) -> Result<(), AudioError> {
        self.state = StreamState::Stopped;
        self.buffer.clear();
        log::debug!("Stream {} stopped", self.id);
        Ok(())
    }

    /// Pause stream
    pub fn pause(&mut self) -> Result<(), AudioError> {
        if self.state == StreamState::Running {
            self.state = StreamState::Paused;
            log::debug!("Stream {} paused", self.id);
        }
        Ok(())
    }

    /// Resume stream
    pub fn resume(&mut self) -> Result<(), AudioError> {
        if self.state == StreamState::Paused {
            self.state = StreamState::Running;
            log::debug!("Stream {} resumed", self.id);
        }
        Ok(())
    }

    /// Write audio data (for playback streams)
    pub fn write(&mut self, data: &[u8]) -> Result<usize, AudioError> {
        if self.direction != StreamDirection::Playback {
            return Err(AudioError::InternalError(
                "Cannot write to capture stream".into(),
            ));
        }

        let written = self.buffer.write(data);
        let frames = written / (self.config.channels as usize * self.config.format.bytes_per_sample());
        self.frames_written += frames as u64;

        if written < data.len() {
            log::trace!("Buffer overrun on stream {}", self.id);
            Err(AudioError::BufferOverrun)
        } else {
            Ok(written)
        }
    }

    /// Read audio data (for capture streams)
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, AudioError> {
        if self.direction != StreamDirection::Capture {
            return Err(AudioError::InternalError(
                "Cannot read from playback stream".into(),
            ));
        }

        let read = self.buffer.read(buf);
        let frames = read / (self.config.channels as usize * self.config.format.bytes_per_sample());
        self.frames_read += frames as u64;

        if read < buf.len() && self.state == StreamState::Running {
            log::trace!("Buffer underrun on stream {}", self.id);
            Err(AudioError::BufferUnderrun)
        } else {
            Ok(read)
        }
    }

    /// Get available space for writing
    pub fn available_write(&self) -> usize {
        self.buffer.available_write()
    }

    /// Get available data for reading
    pub fn available_read(&self) -> usize {
        self.buffer.available_read()
    }
}
