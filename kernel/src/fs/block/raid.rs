//! Software RAID Support
//!
//! Implements basic RAID levels (0, 1, 5, 6, 10) for data redundancy and performance.

use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::RwLock;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::fs::{FsError, FsResult};
use super::device::{BlockDevice, BlockDeviceInfo, DeviceType};

/// RAID level
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RaidLevel {
    /// RAID 0 - Striping (no redundancy, best performance)
    Raid0,
    /// RAID 1 - Mirroring (full redundancy)
    Raid1,
    /// RAID 5 - Striping with distributed parity
    Raid5,
    /// RAID 6 - Striping with dual parity
    Raid6,
    /// RAID 10 - Mirrored stripes
    Raid10,
}

impl RaidLevel {
    /// Get minimum number of devices required
    pub fn min_devices(&self) -> usize {
        match self {
            RaidLevel::Raid0 => 2,
            RaidLevel::Raid1 => 2,
            RaidLevel::Raid5 => 3,
            RaidLevel::Raid6 => 4,
            RaidLevel::Raid10 => 4,
        }
    }

    /// Get maximum devices that can fail without data loss
    pub fn fault_tolerance(&self) -> usize {
        match self {
            RaidLevel::Raid0 => 0,
            RaidLevel::Raid1 => 1,
            RaidLevel::Raid5 => 1,
            RaidLevel::Raid6 => 2,
            RaidLevel::Raid10 => 1,
        }
    }

    /// Calculate usable size given device count and size
    pub fn usable_size(&self, num_devices: usize, device_size: u64) -> u64 {
        match self {
            RaidLevel::Raid0 => device_size * num_devices as u64,
            RaidLevel::Raid1 => device_size,
            RaidLevel::Raid5 => device_size * (num_devices - 1) as u64,
            RaidLevel::Raid6 => device_size * (num_devices - 2) as u64,
            RaidLevel::Raid10 => device_size * (num_devices / 2) as u64,
        }
    }
}

/// RAID array configuration
#[derive(Debug, Clone)]
pub struct RaidConfig {
    /// RAID level
    pub level: RaidLevel,
    /// Chunk/stripe size in bytes (must be power of 2)
    pub chunk_size: u32,
    /// Array name
    pub name: alloc::string::String,
}

impl RaidConfig {
    /// Create new RAID configuration
    pub fn new(level: RaidLevel, chunk_size: u32, name: alloc::string::String) -> Self {
        let chunk_size = if chunk_size.is_power_of_two() {
            chunk_size
        } else {
            chunk_size.next_power_of_two()
        };

        Self {
            level,
            chunk_size,
            name,
        }
    }
}

/// RAID array - Software RAID implementation
pub struct RaidArray {
    /// Configuration
    config: RaidConfig,
    /// Member devices
    devices: Vec<Arc<RwLock<dyn BlockDevice>>>,
    /// Device info (synthesized)
    info: BlockDeviceInfo,
    /// Failed device bitmap
    failed_devices: AtomicU64,
    /// Total operations
    operations: AtomicU64,
}

impl RaidArray {
    /// Create a new RAID array
    pub fn new(
        config: RaidConfig,
        devices: Vec<Arc<RwLock<dyn BlockDevice>>>,
    ) -> FsResult<Arc<RwLock<Self>>> {
        if devices.len() < config.level.min_devices() {
            return Err(FsError::InvalidArgument);
        }

        let first_device = devices[0].read();
        let device_size = first_device.size();
        let block_size = first_device.block_size();
        drop(first_device);

        for device in &devices {
            let dev = device.read();
            if dev.size() != device_size {
                return Err(FsError::InvalidArgument);
            }
            if dev.block_size() != block_size {
                return Err(FsError::InvalidArgument);
            }
        }

        let total_size = config.level.usable_size(devices.len(), device_size);

        let info = BlockDeviceInfo {
            name: config.name.clone(),
            size: total_size,
            block_size,
            read_only: false,
            device_type: DeviceType::Virtual,
            vendor: alloc::string::String::from("ExoOS"),
            model: alloc::format!("RAID{}", match config.level {
                RaidLevel::Raid0 => "0",
                RaidLevel::Raid1 => "1",
                RaidLevel::Raid5 => "5",
                RaidLevel::Raid6 => "6",
                RaidLevel::Raid10 => "10",
            }),
            serial: alloc::format!("RAID-{}", config.name),
        };

        Ok(Arc::new(RwLock::new(Self {
            config,
            devices,
            info,
            failed_devices: AtomicU64::new(0),
            operations: AtomicU64::new(0),
        })))
    }

    /// Calculate stripe information for an offset
    fn calculate_stripe(&self, offset: u64) -> StripeInfo {
        let chunk_size = self.config.chunk_size as u64;
        let num_devices = self.devices.len();

        match self.config.level {
            RaidLevel::Raid0 => {
                let stripe_num = offset / chunk_size;
                let device_idx = (stripe_num % num_devices as u64) as usize;
                let device_offset = (stripe_num / num_devices as u64) * chunk_size +
                                   (offset % chunk_size);

                StripeInfo {
                    device_indices: alloc::vec![device_idx],
                    device_offsets: alloc::vec![device_offset],
                    parity_indices: Vec::new(),
                }
            }
            RaidLevel::Raid1 => {
                StripeInfo {
                    device_indices: (0..num_devices).collect(),
                    device_offsets: alloc::vec![offset; num_devices],
                    parity_indices: Vec::new(),
                }
            }
            RaidLevel::Raid5 => {
                let data_disks = num_devices - 1;
                let stripe_num = offset / chunk_size;
                let chunk_in_stripe = stripe_num % data_disks as u64;
                let stripe_set = stripe_num / data_disks as u64;
                let parity_idx = (num_devices - 1 - (stripe_set % num_devices as u64) as usize) % num_devices;

                let mut device_idx = chunk_in_stripe as usize;
                if device_idx >= parity_idx {
                    device_idx += 1;
                }

                let device_offset = (stripe_set * chunk_size) + (offset % chunk_size);

                StripeInfo {
                    device_indices: alloc::vec![device_idx],
                    device_offsets: alloc::vec![device_offset],
                    parity_indices: alloc::vec![parity_idx],
                }
            }
            RaidLevel::Raid6 => {
                let data_disks = num_devices - 2;
                let stripe_num = offset / chunk_size;
                let chunk_in_stripe = stripe_num % data_disks as u64;
                let stripe_set = stripe_num / data_disks as u64;

                let parity1_idx = (num_devices - 1 - (stripe_set % num_devices as u64) as usize) % num_devices;
                let parity2_idx = (num_devices - 2 - (stripe_set % num_devices as u64) as usize) % num_devices;

                let mut device_idx = chunk_in_stripe as usize;
                if device_idx >= parity2_idx.min(parity1_idx) {
                    device_idx += 1;
                }
                if device_idx >= parity2_idx.max(parity1_idx) {
                    device_idx += 1;
                }

                let device_offset = (stripe_set * chunk_size) + (offset % chunk_size);

                StripeInfo {
                    device_indices: alloc::vec![device_idx],
                    device_offsets: alloc::vec![device_offset],
                    parity_indices: alloc::vec![parity1_idx, parity2_idx],
                }
            }
            RaidLevel::Raid10 => {
                let stripe_num = offset / chunk_size;
                let device_pair = (stripe_num % (num_devices / 2) as u64) as usize;
                let device_offset = (stripe_num / (num_devices / 2) as u64) * chunk_size +
                                   (offset % chunk_size);

                StripeInfo {
                    device_indices: alloc::vec![device_pair * 2, device_pair * 2 + 1],
                    device_offsets: alloc::vec![device_offset, device_offset],
                    parity_indices: Vec::new(),
                }
            }
        }
    }

    /// Check if device is failed
    fn is_device_failed(&self, device_idx: usize) -> bool {
        let failed = self.failed_devices.load(Ordering::Relaxed);
        (failed & (1 << device_idx)) != 0
    }

    /// Mark device as failed
    pub fn mark_device_failed(&self, device_idx: usize) {
        self.failed_devices.fetch_or(1 << device_idx, Ordering::Relaxed);
    }

    /// Get number of failed devices
    pub fn failed_count(&self) -> usize {
        let failed = self.failed_devices.load(Ordering::Relaxed);
        failed.count_ones() as usize
    }

    /// Check if array is degraded but functional
    pub fn is_degraded(&self) -> bool {
        let failed = self.failed_count();
        failed > 0 && failed <= self.config.level.fault_tolerance()
    }

    /// Check if array has failed
    pub fn is_failed(&self) -> bool {
        self.failed_count() > self.config.level.fault_tolerance()
    }
}

impl BlockDevice for RaidArray {
    fn read(&self, offset: u64, buf: &mut [u8]) -> FsResult<usize> {
        if self.is_failed() {
            return Err(FsError::IoError);
        }

        self.operations.fetch_add(1, Ordering::Relaxed);

        let stripe = self.calculate_stripe(offset);
        let len = buf.len().min(self.config.chunk_size as usize);

        match self.config.level {
            RaidLevel::Raid0 | RaidLevel::Raid5 | RaidLevel::Raid6 => {
                let device_idx = stripe.device_indices[0];
                let device_offset = stripe.device_offsets[0];

                if self.is_device_failed(device_idx) {
                    return Err(FsError::IoError);
                }

                self.devices[device_idx].read().read(device_offset, &mut buf[..len])
            }
            RaidLevel::Raid1 | RaidLevel::Raid10 => {
                for &device_idx in &stripe.device_indices {
                    if !self.is_device_failed(device_idx) {
                        let device_offset = stripe.device_offsets[0];
                        return self.devices[device_idx].read().read(device_offset, &mut buf[..len]);
                    }
                }
                Err(FsError::IoError)
            }
        }
    }

    fn write(&mut self, offset: u64, buf: &[u8]) -> FsResult<usize> {
        if self.is_failed() {
            return Err(FsError::IoError);
        }

        self.operations.fetch_add(1, Ordering::Relaxed);

        let stripe = self.calculate_stripe(offset);
        let len = buf.len().min(self.config.chunk_size as usize);

        match self.config.level {
            RaidLevel::Raid0 => {
                let device_idx = stripe.device_indices[0];
                let device_offset = stripe.device_offsets[0];

                if self.is_device_failed(device_idx) {
                    return Err(FsError::IoError);
                }

                self.devices[device_idx].write().write(device_offset, &buf[..len])
            }
            RaidLevel::Raid1 | RaidLevel::Raid10 => {
                let mut success = false;
                let mut last_result = Err(FsError::IoError);

                for (i, &device_idx) in stripe.device_indices.iter().enumerate() {
                    if !self.is_device_failed(device_idx) {
                        let device_offset = stripe.device_offsets[i];
                        match self.devices[device_idx].write().write(device_offset, &buf[..len]) {
                            Ok(n) => {
                                success = true;
                                last_result = Ok(n);
                            }
                            Err(e) => {
                                self.mark_device_failed(device_idx);
                                last_result = Err(e);
                            }
                        }
                    }
                }

                if success {
                    last_result
                } else {
                    Err(FsError::IoError)
                }
            }
            RaidLevel::Raid5 | RaidLevel::Raid6 => {
                Err(FsError::NotSupported)
            }
        }
    }

    fn flush(&mut self) -> FsResult<()> {
        let mut success = false;

        for (idx, device) in self.devices.iter().enumerate() {
            if !self.is_device_failed(idx) {
                if device.write().flush().is_ok() {
                    success = true;
                }
            }
        }

        if success || self.is_degraded() {
            Ok(())
        } else {
            Err(FsError::IoError)
        }
    }

    fn info(&self) -> &BlockDeviceInfo {
        &self.info
    }
}

/// Stripe information for RAID operations
struct StripeInfo {
    /// Device indices for data
    device_indices: Vec<usize>,
    /// Offsets on each device
    device_offsets: Vec<u64>,
    /// Parity device indices
    parity_indices: Vec<usize>,
}

/// RAID statistics
#[derive(Debug, Clone, Copy)]
pub struct RaidStats {
    /// Total operations
    pub operations: u64,
    /// Failed devices
    pub failed_devices: u64,
    /// Is degraded
    pub is_degraded: bool,
    /// Is failed
    pub is_failed: bool,
}

impl RaidArray {
    /// Get RAID statistics
    pub fn stats(&self) -> RaidStats {
        RaidStats {
            operations: self.operations.load(Ordering::Relaxed),
            failed_devices: self.failed_devices.load(Ordering::Relaxed),
            is_degraded: self.is_degraded(),
            is_failed: self.is_failed(),
        }
    }
}
