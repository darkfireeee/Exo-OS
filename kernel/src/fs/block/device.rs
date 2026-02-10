//! Block Device Abstraction
//!
//! Provides a unified interface for all block devices (HDD, SSD, NVMe, RAM disk, etc.)
//! with async support and zero-copy capabilities.

use alloc::sync::Arc;
use alloc::string::String;
use alloc::vec::Vec;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};
use spin::RwLock;

use crate::fs::{FsError, FsResult};

/// Block device information
#[derive(Debug, Clone)]
pub struct BlockDeviceInfo {
    /// Device name (e.g., "sda", "nvme0n1")
    pub name: String,
    /// Total size in bytes
    pub size: u64,
    /// Block size in bytes (typically 512 or 4096)
    pub block_size: u32,
    /// Whether device is read-only
    pub read_only: bool,
    /// Device type
    pub device_type: DeviceType,
    /// Hardware vendor information
    pub vendor: String,
    /// Model information
    pub model: String,
    /// Serial number
    pub serial: String,
}

/// Type of block device
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceType {
    /// Hard disk drive
    HDD,
    /// Solid state drive
    SSD,
    /// NVMe device
    NVMe,
    /// RAM disk
    RamDisk,
    /// USB storage
    USB,
    /// Virtual disk
    Virtual,
    /// Unknown type
    Unknown,
}

/// Block device trait - Core interface for all block devices
///
/// ## Performance Targets
/// - read(): < 1000 cycles (cache hit)
/// - write(): < 1500 cycles (write-back mode)
/// - flush(): < 100000 cycles (full device sync)
///
/// ## Zero-Copy Philosophy
/// All operations use slices to enable DMA transfers without intermediate buffers.
pub trait BlockDevice: Send + Sync {
    /// Read blocks from device
    ///
    /// # Arguments
    /// - `offset`: Byte offset (must be block-aligned)
    /// - `buf`: Destination buffer (size must be multiple of block_size)
    ///
    /// # Returns
    /// Number of bytes read
    ///
    /// # Errors
    /// - InvalidArgument: offset or size not aligned
    /// - IoError: hardware error
    fn read(&self, offset: u64, buf: &mut [u8]) -> FsResult<usize>;

    /// Write blocks to device
    ///
    /// # Arguments
    /// - `offset`: Byte offset (must be block-aligned)
    /// - `buf`: Source buffer (size must be multiple of block_size)
    ///
    /// # Returns
    /// Number of bytes written
    ///
    /// # Errors
    /// - InvalidArgument: offset or size not aligned
    /// - IoError: hardware error
    /// - PermissionDenied: device is read-only
    fn write(&mut self, offset: u64, buf: &[u8]) -> FsResult<usize>;

    /// Flush all pending writes to physical media
    ///
    /// Ensures data persistence across power loss.
    fn flush(&mut self) -> FsResult<()>;

    /// Get device information
    fn info(&self) -> &BlockDeviceInfo;

    /// Get total size in bytes
    #[inline(always)]
    fn size(&self) -> u64 {
        self.info().size
    }

    /// Get block size in bytes
    #[inline(always)]
    fn block_size(&self) -> u32 {
        self.info().block_size
    }

    /// Check if device is read-only
    #[inline(always)]
    fn is_read_only(&self) -> bool {
        self.info().read_only
    }

    /// Discard/trim blocks (for SSD wear leveling)
    fn discard(&mut self, _offset: u64, _len: u64) -> FsResult<()> {
        Ok(())
    }

    /// Check if a range is block-aligned
    fn is_aligned(&self, offset: u64, len: usize) -> bool {
        let block_size = self.block_size() as u64;
        offset % block_size == 0 && (len as u64) % block_size == 0
    }

    /// Read multiple blocks (convenience method)
    ///
    /// # Arguments
    /// - `start_block`: Starting block number
    /// - `buf`: Destination buffer
    ///
    /// # Returns
    /// Number of bytes read
    fn read_blocks(&self, start_block: u64, buf: &mut [u8]) -> FsResult<usize> {
        let block_size = self.block_size() as u64;
        let offset = start_block * block_size;
        self.read(offset, buf)
    }

    /// Write multiple blocks (convenience method)
    ///
    /// # Arguments
    /// - `start_block`: Starting block number
    /// - `buf`: Source buffer
    ///
    /// # Returns
    /// Number of bytes written
    fn write_blocks(&mut self, start_block: u64, buf: &[u8]) -> FsResult<usize> {
        let block_size = self.block_size() as u64;
        let offset = start_block * block_size;
        self.write(offset, buf)
    }
}

/// Async read operation
pub struct AsyncRead<'a> {
    device: Arc<RwLock<dyn BlockDevice>>,
    offset: u64,
    buf: &'a mut [u8],
    completed: bool,
}

impl<'a> AsyncRead<'a> {
    pub fn new(device: Arc<RwLock<dyn BlockDevice>>, offset: u64, buf: &'a mut [u8]) -> Self {
        Self {
            device,
            offset,
            buf,
            completed: false,
        }
    }
}

impl<'a> Future for AsyncRead<'a> {
    type Output = FsResult<usize>;

    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Safety: We don't move out of self and AsyncRead doesn't implement Unpin
        let this = unsafe { self.get_unchecked_mut() };

        if this.completed {
            return Poll::Pending;
        }

        let offset = this.offset;
        let buf_ptr = this.buf as *mut [u8];

        let result = {
            let device = this.device.read();
            unsafe { device.read(offset, &mut *buf_ptr) }
        };

        this.completed = true;

        Poll::Ready(result)
    }
}

/// Async write operation
pub struct AsyncWrite<'a> {
    device: Arc<RwLock<dyn BlockDevice>>,
    offset: u64,
    buf: &'a [u8],
    completed: bool,
}

impl<'a> AsyncWrite<'a> {
    pub fn new(device: Arc<RwLock<dyn BlockDevice>>, offset: u64, buf: &'a [u8]) -> Self {
        Self {
            device,
            offset,
            buf,
            completed: false,
        }
    }
}

impl<'a> Future for AsyncWrite<'a> {
    type Output = FsResult<usize>;

    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.completed {
            return Poll::Pending;
        }

        let offset = self.offset;
        let buf_ptr = self.buf as *const [u8];

        let result = {
            let mut device = self.device.write();
            unsafe { device.write(offset, &*buf_ptr) }
        };

        self.completed = true;

        Poll::Ready(result)
    }
}

/// Simple RAM disk implementation for testing and tmpfs
pub struct RamDisk {
    info: BlockDeviceInfo,
    data: RwLock<Vec<u8>>,
}

impl RamDisk {
    /// Create a new RAM disk
    pub fn new(name: String, size: u64, block_size: u32) -> Arc<RwLock<Self>> {
        let info = BlockDeviceInfo {
            name,
            size,
            block_size,
            read_only: false,
            device_type: DeviceType::RamDisk,
            vendor: String::from("ExoOS"),
            model: String::from("RamDisk"),
            serial: String::from("RAMDISK-001"),
        };

        let data = RwLock::new(alloc::vec![0u8; size as usize]);

        Arc::new(RwLock::new(Self { info, data }))
    }
}

impl BlockDevice for RamDisk {
    fn read(&self, offset: u64, buf: &mut [u8]) -> FsResult<usize> {
        if !self.is_aligned(offset, buf.len()) {
            return Err(FsError::InvalidArgument);
        }

        let data = self.data.read();
        let start = offset as usize;
        let end = start + buf.len();

        if end > data.len() {
            return Err(FsError::InvalidArgument);
        }

        buf.copy_from_slice(&data[start..end]);
        Ok(buf.len())
    }

    fn write(&mut self, offset: u64, buf: &[u8]) -> FsResult<usize> {
        if !self.is_aligned(offset, buf.len()) {
            return Err(FsError::InvalidArgument);
        }

        let mut data = self.data.write();
        let start = offset as usize;
        let end = start + buf.len();

        if end > data.len() {
            return Err(FsError::InvalidArgument);
        }

        data[start..end].copy_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> FsResult<()> {
        Ok(())
    }

    fn info(&self) -> &BlockDeviceInfo {
        &self.info
    }
}

/// Block device registry - Global registry for all block devices
pub struct BlockDeviceRegistry {
    devices: RwLock<Vec<Arc<RwLock<dyn BlockDevice>>>>,
}

impl BlockDeviceRegistry {
    pub const fn new() -> Self {
        Self {
            devices: RwLock::new(Vec::new()),
        }
    }

    /// Register a new block device
    pub fn register(&self, device: Arc<RwLock<dyn BlockDevice>>) {
        let mut devices = self.devices.write();
        devices.push(device);
    }

    /// Get device by name
    pub fn get(&self, name: &str) -> Option<Arc<RwLock<dyn BlockDevice>>> {
        let devices = self.devices.read();
        devices
            .iter()
            .find(|d| d.read().info().name == name)
            .cloned()
    }

    /// List all devices
    pub fn list(&self) -> Vec<BlockDeviceInfo> {
        let devices = self.devices.read();
        devices.iter().map(|d| d.read().info().clone()).collect()
    }

    /// Get device count
    pub fn count(&self) -> usize {
        self.devices.read().len()
    }
}

impl Default for BlockDeviceRegistry {
    fn default() -> Self {
        Self::new()
    }
}
