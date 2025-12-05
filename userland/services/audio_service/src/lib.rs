//! # Audio Service for Exo-OS
//!
//! PipeWire-inspired audio service using Fusion Rings for ultra-low latency IPC.
//!
//! ## Performance Targets
//!
//! | Metric | Target | PipeWire Linux | Gain |
//! |--------|--------|----------------|------|
//! | Audio latency | < 1 ms | ~5 ms | 5x |
//! | Buffer underruns | ~0 | Occasional | ∞x |
//! | CPU usage (idle) | < 0.5% | ~1% | 2x |
//! | IPC transport | 347 cycles | ~2000 cycles | 5.8x |
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    Applications                             │
//! │  ┌──────────┐  ┌──────────┐  ┌──────────┐                  │
//! │  │ Browser  │  │  Music   │  │  Game    │                  │
//! │  │ (video)  │  │ Player   │  │ (3D)     │                  │
//! │  └────┬─────┘  └────┬─────┘  └────┬─────┘                  │
//! └───────┼─────────────┼─────────────┼────────────────────────┘
//!         │             │             │
//!         ▼             ▼             ▼
//! ┌─────────────────────────────────────────────────────────────┐
//! │                 Audio Graph Engine                          │
//! │  ┌──────────────────────────────────────────────────────┐  │
//! │  │              Processing Nodes                         │  │
//! │  │  [Source] → [Mixer] → [Effects] → [Resampler] → [Sink]│  │
//! │  └──────────────────────────────────────────────────────┘  │
//! │                                                             │
//! │  ┌─────────────────┐  ┌─────────────────┐                  │
//! │  │   Fusion Ring   │  │  Zero-Copy      │                  │
//! │  │   IPC Layer     │  │  Buffer Pool    │                  │
//! │  │  (347 cycles)   │  │  (0 copies)     │                  │
//! │  └─────────────────┘  └─────────────────┘                  │
//! └─────────────────────────────────────────────────────────────┘
//!         │
//!         ▼
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    Hardware Drivers                         │
//! │  ┌──────────┐  ┌──────────┐  ┌──────────┐                  │
//! │  │   ALSA   │  │   USB    │  │Bluetooth │                  │
//! │  │  Driver  │  │  Audio   │  │  Audio   │                  │
//! │  └──────────┘  └──────────┘  └──────────┘                  │
//! └─────────────────────────────────────────────────────────────┘
//! ```

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

extern crate alloc;

pub mod buffer;
pub mod device;
pub mod graph;
pub mod stream;

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

/// Audio service version
pub const VERSION: &str = "0.1.0";

/// Default sample rate (Hz)
pub const DEFAULT_SAMPLE_RATE: u32 = 48000;

/// Default buffer size (samples per channel)
/// 256 samples at 48kHz = 5.33ms, but we target < 1ms latency
/// by using smaller buffers and Fusion Rings
pub const DEFAULT_BUFFER_SIZE: u32 = 48; // ~1ms at 48kHz

/// Maximum supported channels
pub const MAX_CHANNELS: usize = 8;

/// Maximum concurrent streams
pub const MAX_STREAMS: usize = 256;

/// Fusion Ring buffer size for audio (optimized for low latency)
pub const AUDIO_RING_SIZE: usize = 64;

/// Audio format enumeration
///
/// Supported audio sample formats with their properties.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum AudioFormat {
    /// 16-bit signed integer, little-endian
    /// Dynamic range: ~96 dB, common for CD quality
    S16LE = 0,
    /// 24-bit signed integer, little-endian (packed)
    /// Dynamic range: ~144 dB, professional audio
    S24LE = 1,
    /// 32-bit signed integer, little-endian
    /// Dynamic range: ~192 dB, studio quality
    S32LE = 2,
    /// 32-bit float, little-endian
    /// Preferred for processing, infinite dynamic range
    F32LE = 3,
}

impl AudioFormat {
    /// Bytes per sample for this format
    #[inline]
    pub const fn bytes_per_sample(&self) -> usize {
        match self {
            AudioFormat::S16LE => 2,
            AudioFormat::S24LE => 3,
            AudioFormat::S32LE | AudioFormat::F32LE => 4,
        }
    }

    /// Bits per sample
    #[inline]
    pub const fn bits_per_sample(&self) -> u32 {
        match self {
            AudioFormat::S16LE => 16,
            AudioFormat::S24LE => 24,
            AudioFormat::S32LE | AudioFormat::F32LE => 32,
        }
    }

    /// Is this a floating point format?
    #[inline]
    pub const fn is_float(&self) -> bool {
        matches!(self, AudioFormat::F32LE)
    }

    /// Get format name
    #[inline]
    pub const fn name(&self) -> &'static str {
        match self {
            AudioFormat::S16LE => "S16_LE",
            AudioFormat::S24LE => "S24_LE",
            AudioFormat::S32LE => "S32_LE",
            AudioFormat::F32LE => "F32_LE",
        }
    }
}

impl Default for AudioFormat {
    fn default() -> Self {
        AudioFormat::F32LE // Best for processing
    }
}

/// Audio stream configuration
///
/// Defines the audio format, sample rate, and buffer parameters.
#[derive(Debug, Clone)]
pub struct AudioConfig {
    /// Sample rate in Hz (8000-192000)
    pub sample_rate: u32,
    /// Number of channels (1-8)
    pub channels: u8,
    /// Audio format
    pub format: AudioFormat,
    /// Buffer size in samples per channel
    pub buffer_size: u32,
    /// Number of periods (for double/triple buffering)
    pub periods: u8,
}

impl AudioConfig {
    /// Create new configuration with validation
    pub fn new(
        sample_rate: u32,
        channels: u8,
        format: AudioFormat,
        buffer_size: u32,
    ) -> Result<Self, AudioError> {
        // Validate sample rate
        if !(8000..=192000).contains(&sample_rate) {
            return Err(AudioError::InvalidConfig(
                "Sample rate must be 8000-192000 Hz".into(),
            ));
        }

        // Validate channels
        if channels == 0 || channels as usize > MAX_CHANNELS {
            return Err(AudioError::InvalidConfig(alloc::format!(
                "Channels must be 1-{}",
                MAX_CHANNELS
            )));
        }

        // Validate buffer size (must be reasonable)
        if buffer_size < 16 || buffer_size > 8192 {
            return Err(AudioError::InvalidConfig(
                "Buffer size must be 16-8192 samples".into(),
            ));
        }

        Ok(Self {
            sample_rate,
            channels,
            format,
            buffer_size,
            periods: 2, // Double buffering by default
        })
    }

    /// Calculate latency in microseconds
    #[inline]
    pub fn latency_us(&self) -> u64 {
        (self.buffer_size as u64 * 1_000_000) / self.sample_rate as u64
    }

    /// Calculate latency in milliseconds
    #[inline]
    pub fn latency_ms(&self) -> f32 {
        (self.buffer_size as f32 * 1000.0) / self.sample_rate as f32
    }

    /// Calculate bytes per frame (all channels)
    #[inline]
    pub fn bytes_per_frame(&self) -> usize {
        self.format.bytes_per_sample() * self.channels as usize
    }

    /// Calculate total buffer size in bytes
    #[inline]
    pub fn buffer_bytes(&self) -> usize {
        self.buffer_size as usize * self.bytes_per_frame()
    }

    /// Calculate bit rate in bits per second
    #[inline]
    pub fn bit_rate(&self) -> u64 {
        self.sample_rate as u64 * self.channels as u64 * self.format.bits_per_sample() as u64
    }
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            sample_rate: DEFAULT_SAMPLE_RATE,
            channels: 2, // Stereo
            format: AudioFormat::F32LE,
            buffer_size: DEFAULT_BUFFER_SIZE,
            periods: 2,
        }
    }
}

/// Audio service error types
#[derive(Debug, Clone)]
pub enum AudioError {
    /// Device not found
    DeviceNotFound(String),
    /// Invalid configuration
    InvalidConfig(String),
    /// Format not supported by device
    FormatNotSupported(String),
    /// Buffer underrun (playback starvation)
    BufferUnderrun,
    /// Buffer overrun (capture overflow)
    BufferOverrun,
    /// IPC error
    IpcError(String),
    /// Stream not found
    StreamNotFound(u64),
    /// Stream already exists
    StreamExists(u64),
    /// Device busy
    DeviceBusy(String),
    /// Permission denied
    PermissionDenied(String),
    /// Internal error
    InternalError(String),
}

impl AudioError {
    /// Get error code for IPC responses
    pub fn code(&self) -> i32 {
        match self {
            AudioError::DeviceNotFound(_) => -2,    // ENOENT
            AudioError::InvalidConfig(_) => -22,    // EINVAL
            AudioError::FormatNotSupported(_) => -38, // ENOSYS
            AudioError::BufferUnderrun => -5,       // EIO
            AudioError::BufferOverrun => -5,        // EIO
            AudioError::IpcError(_) => -5,          // EIO
            AudioError::StreamNotFound(_) => -2,    // ENOENT
            AudioError::StreamExists(_) => -17,     // EEXIST
            AudioError::DeviceBusy(_) => -16,       // EBUSY
            AudioError::PermissionDenied(_) => -13, // EACCES
            AudioError::InternalError(_) => -5,     // EIO
        }
    }
}

/// Audio service statistics (thread-safe)
#[derive(Debug)]
pub struct AudioStats {
    /// Total samples processed
    samples_processed: AtomicU64,
    /// Total frames processed
    frames_processed: AtomicU64,
    /// Buffer underruns
    underruns: AtomicU64,
    /// Buffer overruns
    overruns: AtomicU64,
    /// Total latency samples (for average calculation)
    total_latency_samples: AtomicU64,
    /// Latency measurements count
    latency_count: AtomicU64,
    /// Maximum observed latency (samples)
    max_latency_samples: AtomicU64,
    /// IPC messages sent
    ipc_messages_sent: AtomicU64,
    /// IPC messages received
    ipc_messages_recv: AtomicU64,
}

impl AudioStats {
    /// Create new statistics tracker
    pub const fn new() -> Self {
        Self {
            samples_processed: AtomicU64::new(0),
            frames_processed: AtomicU64::new(0),
            underruns: AtomicU64::new(0),
            overruns: AtomicU64::new(0),
            total_latency_samples: AtomicU64::new(0),
            latency_count: AtomicU64::new(0),
            max_latency_samples: AtomicU64::new(0),
            ipc_messages_sent: AtomicU64::new(0),
            ipc_messages_recv: AtomicU64::new(0),
        }
    }

    /// Record samples processed
    #[inline]
    pub fn record_samples(&self, count: u64, channels: u8) {
        self.samples_processed.fetch_add(count, Ordering::Relaxed);
        self.frames_processed
            .fetch_add(count / channels as u64, Ordering::Relaxed);
    }

    /// Record buffer underrun
    #[inline]
    pub fn record_underrun(&self) {
        self.underruns.fetch_add(1, Ordering::Relaxed);
        log::warn!("Audio buffer underrun detected");
    }

    /// Record buffer overrun
    #[inline]
    pub fn record_overrun(&self) {
        self.overruns.fetch_add(1, Ordering::Relaxed);
        log::warn!("Audio buffer overrun detected");
    }

    /// Record latency measurement
    #[inline]
    pub fn record_latency(&self, samples: u64) {
        self.total_latency_samples
            .fetch_add(samples, Ordering::Relaxed);
        self.latency_count.fetch_add(1, Ordering::Relaxed);

        // Update max if needed
        let mut current_max = self.max_latency_samples.load(Ordering::Relaxed);
        while samples > current_max {
            match self.max_latency_samples.compare_exchange_weak(
                current_max,
                samples,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(new_max) => current_max = new_max,
            }
        }
    }

    /// Get average latency in samples
    pub fn avg_latency_samples(&self) -> f64 {
        let count = self.latency_count.load(Ordering::Relaxed);
        if count == 0 {
            return 0.0;
        }
        self.total_latency_samples.load(Ordering::Relaxed) as f64 / count as f64
    }

    /// Get average latency in microseconds (assuming 48kHz)
    pub fn avg_latency_us(&self) -> f64 {
        self.avg_latency_samples() * 1_000_000.0 / 48000.0
    }

    /// Get underrun count
    pub fn underruns(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }

    /// Get overrun count
    pub fn overruns(&self) -> u64 {
        self.overruns.load(Ordering::Relaxed)
    }

    /// Generate statistics report
    pub fn report(&self) -> AudioStatsReport {
        AudioStatsReport {
            samples_processed: self.samples_processed.load(Ordering::Relaxed),
            frames_processed: self.frames_processed.load(Ordering::Relaxed),
            underruns: self.underruns.load(Ordering::Relaxed),
            overruns: self.overruns.load(Ordering::Relaxed),
            avg_latency_us: self.avg_latency_us(),
            max_latency_samples: self.max_latency_samples.load(Ordering::Relaxed),
            ipc_messages_sent: self.ipc_messages_sent.load(Ordering::Relaxed),
            ipc_messages_recv: self.ipc_messages_recv.load(Ordering::Relaxed),
        }
    }

    /// Reset statistics
    pub fn reset(&self) {
        self.samples_processed.store(0, Ordering::Relaxed);
        self.frames_processed.store(0, Ordering::Relaxed);
        self.underruns.store(0, Ordering::Relaxed);
        self.overruns.store(0, Ordering::Relaxed);
        self.total_latency_samples.store(0, Ordering::Relaxed);
        self.latency_count.store(0, Ordering::Relaxed);
        self.max_latency_samples.store(0, Ordering::Relaxed);
        self.ipc_messages_sent.store(0, Ordering::Relaxed);
        self.ipc_messages_recv.store(0, Ordering::Relaxed);
    }
}

impl Default for AudioStats {
    fn default() -> Self {
        Self::new()
    }
}

/// Statistics report snapshot
#[derive(Debug, Clone)]
pub struct AudioStatsReport {
    /// Total samples processed
    pub samples_processed: u64,
    /// Total frames processed
    pub frames_processed: u64,
    /// Buffer underruns
    pub underruns: u64,
    /// Buffer overruns
    pub overruns: u64,
    /// Average latency in microseconds
    pub avg_latency_us: f64,
    /// Maximum latency in samples
    pub max_latency_samples: u64,
    /// IPC messages sent
    pub ipc_messages_sent: u64,
    /// IPC messages received
    pub ipc_messages_recv: u64,
}

/// Audio service state
///
/// Main audio service managing devices, streams, and the processing graph.
pub struct AudioService {
    /// Service configuration
    config: AudioConfig,
    /// Active stream IDs
    streams: Vec<u64>,
    /// Next stream ID
    next_stream_id: u64,
    /// Statistics
    stats: AudioStats,
    /// Service running state
    running: bool,
}

impl AudioService {
    /// Create new audio service with configuration
    pub fn new(config: AudioConfig) -> Self {
        log::debug!(
            "Creating audio service: {}Hz, {} channels, {}, {} samples buffer",
            config.sample_rate,
            config.channels,
            config.format.name(),
            config.buffer_size
        );

        Self {
            config,
            streams: Vec::with_capacity(MAX_STREAMS),
            next_stream_id: 1,
            stats: AudioStats::new(),
            running: false,
        }
    }

    /// Create with default configuration
    pub fn with_defaults() -> Self {
        Self::new(AudioConfig::default())
    }

    /// Start the audio service
    ///
    /// Initializes the audio graph, discovers devices, and starts the main loop.
    pub fn start(&mut self) -> Result<(), AudioError> {
        if self.running {
            return Err(AudioError::InternalError("Service already running".into()));
        }

        log::info!(
            "Audio service starting: {}Hz, {} channels, {} format, {:.2}ms latency target",
            self.config.sample_rate,
            self.config.channels,
            self.config.format.name(),
            self.config.latency_ms()
        );

        // Initialize Fusion Ring for IPC
        // TODO: Create Fusion Ring with AUDIO_RING_SIZE slots

        // Enumerate and initialize audio devices
        // TODO: Discover ALSA/USB/Bluetooth devices

        self.running = true;
        log::info!("Audio service started successfully");
        Ok(())
    }

    /// Stop the audio service
    pub fn stop(&mut self) -> Result<(), AudioError> {
        if !self.running {
            return Ok(());
        }

        log::info!("Audio service stopping...");

        // Stop all streams
        self.streams.clear();

        // Disconnect Fusion Ring
        // TODO: Cleanup IPC resources

        self.running = false;
        log::info!(
            "Audio service stopped. Stats: {} samples, {} underruns, {:.2}µs avg latency",
            self.stats.samples_processed.load(Ordering::Relaxed),
            self.stats.underruns.load(Ordering::Relaxed),
            self.stats.avg_latency_us()
        );

        Ok(())
    }

    /// Check if service is running
    #[inline]
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Get service configuration
    #[inline]
    pub fn config(&self) -> &AudioConfig {
        &self.config
    }

    /// Create a new audio stream
    ///
    /// # Arguments
    /// * `config` - Stream configuration
    ///
    /// # Returns
    /// Stream ID on success
    pub fn create_stream(&mut self, config: AudioConfig) -> Result<u64, AudioError> {
        if !self.running {
            return Err(AudioError::InternalError("Service not running".into()));
        }

        if self.streams.len() >= MAX_STREAMS {
            return Err(AudioError::InternalError(alloc::format!(
                "Maximum streams ({}) reached",
                MAX_STREAMS
            )));
        }

        let stream_id = self.next_stream_id;
        self.next_stream_id += 1;

        self.streams.push(stream_id);

        log::debug!(
            "Created stream {}: {}Hz, {} channels, {} format",
            stream_id,
            config.sample_rate,
            config.channels,
            config.format.name()
        );

        Ok(stream_id)
    }

    /// Destroy an audio stream
    ///
    /// # Arguments
    /// * `stream_id` - Stream to destroy
    pub fn destroy_stream(&mut self, stream_id: u64) -> Result<(), AudioError> {
        if let Some(pos) = self.streams.iter().position(|&id| id == stream_id) {
            self.streams.remove(pos);
            log::debug!("Destroyed stream {}", stream_id);
            Ok(())
        } else {
            Err(AudioError::StreamNotFound(stream_id))
        }
    }

    /// Get number of active streams
    #[inline]
    pub fn stream_count(&self) -> usize {
        self.streams.len()
    }

    /// Get statistics reference
    #[inline]
    pub fn stats(&self) -> &AudioStats {
        &self.stats
    }

    /// Process one audio cycle
    ///
    /// Called by the audio thread to process buffers.
    /// This is the hot path that must complete within the buffer period.
    #[inline]
    pub fn process_cycle(&mut self) -> Result<(), AudioError> {
        // TODO: Implement actual audio processing
        // 1. Read from input streams via Fusion Ring
        // 2. Mix/process through audio graph
        // 3. Write to output streams via Fusion Ring

        // Record samples processed
        self.stats.record_samples(
            self.config.buffer_size as u64 * self.config.channels as u64,
            self.config.channels,
        );

        Ok(())
    }
}

impl Drop for AudioService {
    fn drop(&mut self) {
        if self.running {
            let _ = self.stop();
        }
    }
}
