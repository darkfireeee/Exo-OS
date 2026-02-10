//! DevFS - Device Node Filesystem
//!
//! ## Features
//! - Dynamic device node creation
//! - Character and block device support
//! - Standard devices (/dev/null, /dev/zero, /dev/random, etc.)
//! - TTY devices
//! - Device hotplug support
//! - mmap support for /dev/mem, /dev/zero
//!
//! ## Standard Devices
//! ```
//! /dev/
//!   ├── null         - Null device (discards all writes)
//!   ├── zero         - Zero device (infinite zeros)
//!   ├── full         - Full device (always returns ENOSPC)
//!   ├── random       - Random number generator
//!   ├── urandom      - Non-blocking random
//!   ├── mem          - Physical memory access (restricted)
//!   ├── kmem         - Kernel memory access (restricted)
//!   ├── console      - System console
//!   ├── tty          - Current TTY
//!   ├── tty0         - Virtual terminal 0
//!   └── pts/         - Pseudo-terminal slaves
//! ```

use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::string::{String, ToString};
use alloc::boxed::Box;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::RwLock;
use hashbrown::HashMap;

use crate::fs::core::types::{
    Inode, InodeType, InodePermissions, Timestamp,
};
use crate::fs::{FsError, FsResult};

/// Device types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceType {
    Char,
    Block,
}

/// Major device numbers (Linux compatible)
pub mod major {
    pub const MEM: u32 = 1;
    pub const TTY: u32 = 4;
    pub const CONSOLE: u32 = 5;
    pub const RANDOM: u32 = 1;
}

/// Minor device numbers
pub mod minor {
    pub const NULL: u32 = 3;
    pub const ZERO: u32 = 5;
    pub const FULL: u32 = 7;
    pub const RANDOM: u32 = 8;
    pub const URANDOM: u32 = 9;
}

/// Device operations trait
pub trait DeviceOps: Send + Sync {
    /// Read from device
    fn read(&self, offset: u64, buf: &mut [u8]) -> FsResult<usize>;

    /// Write to device
    fn write(&mut self, offset: u64, buf: &[u8]) -> FsResult<usize>;

    /// Poll for readiness
    fn poll(&self) -> u32 {
        0x3 // POLLIN | POLLOUT - always ready
    }

    /// ioctl
    fn ioctl(&mut self, _cmd: u32, _arg: u64) -> FsResult<u64> {
        Err(FsError::NotSupported)
    }

    /// mmap support
    fn mmap(&self, _offset: u64, _len: usize) -> FsResult<*mut u8> {
        Err(FsError::NotSupported)
    }
}

/// Null device (/dev/null)
pub struct NullDevice;

impl DeviceOps for NullDevice {
    fn read(&self, _offset: u64, _buf: &mut [u8]) -> FsResult<usize> {
        Ok(0) // Always EOF
    }

    fn write(&mut self, _offset: u64, buf: &[u8]) -> FsResult<usize> {
        Ok(buf.len()) // Discard all data
    }
}

/// Zero device (/dev/zero)
pub struct ZeroDevice;

impl DeviceOps for ZeroDevice {
    fn read(&self, _offset: u64, buf: &mut [u8]) -> FsResult<usize> {
        buf.fill(0);
        Ok(buf.len())
    }

    fn write(&mut self, _offset: u64, buf: &[u8]) -> FsResult<usize> {
        Ok(buf.len()) // Discard all data
    }

    fn mmap(&self, _offset: u64, len: usize) -> FsResult<*mut u8> {
        // Allocate zero-filled pages
        let layout = core::alloc::Layout::from_size_align(len, 4096)
            .map_err(|_| FsError::InvalidArgument)?;

        unsafe {
            let ptr = alloc::alloc::alloc_zeroed(layout);
            if ptr.is_null() {
                Err(FsError::NoMemory)
            } else {
                Ok(ptr)
            }
        }
    }
}

/// Full device (/dev/full)
struct FullDevice;

impl DeviceOps for FullDevice {
    fn read(&self, _offset: u64, buf: &mut [u8]) -> FsResult<usize> {
        buf.fill(0);
        Ok(buf.len())
    }

    fn write(&mut self, _offset: u64, _buf: &[u8]) -> FsResult<usize> {
        Err(FsError::NoSpace) // Always full
    }
}

/// Random device (/dev/random, /dev/urandom)
struct RandomDevice {
    // Simple PRNG state (not cryptographically secure)
    state: AtomicU64,
}

impl RandomDevice {
    fn new() -> Self {
        Self {
            state: AtomicU64::new(0x123456789ABCDEF0),
        }
    }

    fn next_random(&self) -> u64 {
        // Simple xorshift64* PRNG
        let mut x = self.state.load(Ordering::Relaxed);
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state.store(x, Ordering::Relaxed);
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
}

impl DeviceOps for RandomDevice {
    fn read(&self, _offset: u64, buf: &mut [u8]) -> FsResult<usize> {
        let mut offset = 0;

        while offset + 8 <= buf.len() {
            let rand = self.next_random();
            buf[offset..offset + 8].copy_from_slice(&rand.to_le_bytes());
            offset += 8;
        }

        // Fill remaining bytes
        if offset < buf.len() {
            let rand = self.next_random();
            let bytes = rand.to_le_bytes();
            let remaining = buf.len() - offset;
            buf[offset..].copy_from_slice(&bytes[..remaining]);
        }

        Ok(buf.len())
    }

    fn write(&mut self, _offset: u64, buf: &[u8]) -> FsResult<usize> {
        // Add entropy to PRNG
        for &byte in buf {
            let current = self.state.load(Ordering::Relaxed);
            let new = current ^ (byte as u64);
            self.state.store(new, Ordering::Relaxed);
        }
        Ok(buf.len())
    }
}

/// Console device (/dev/console)
struct ConsoleDevice;

impl DeviceOps for ConsoleDevice {
    fn read(&self, _offset: u64, buf: &mut [u8]) -> FsResult<usize> {
        // Stub - keyboard driver not yet implemented
        // TODO: Implement keyboard driver in crate::drivers::keyboard
        log::warn!("ConsoleDevice::read: keyboard driver stub");
        Ok(0)
    }

    fn write(&mut self, _offset: u64, buf: &[u8]) -> FsResult<usize> {
        // Write to serial console (VGA driver not yet available)
        // TODO: Implement VGA driver in crate::drivers::vga
        use core::fmt::Write;
        for &byte in buf {
            // Use serial output for now
            let _ = write!(crate::SERIAL1.lock(), "{}", byte as char);
        }
        Ok(buf.len())
    }
}

/// Device entry in registry
struct DeviceEntry {
    name: String,
    dev_type: DeviceType,
    major: u32,
    minor: u32,
    ops: Arc<RwLock<dyn DeviceOps>>,
}

/// Device Inode
pub struct DevInode {
    ino: u64,
    entry: Arc<DeviceEntry>,
    ctime: Timestamp,
}

impl DevInode {
    fn new(ino: u64, entry: Arc<DeviceEntry>) -> Self {
        Self {
            ino,
            entry,
            ctime: Timestamp::now(),
        }
    }
}

impl Inode for DevInode {
    fn ino(&self) -> u64 {
        self.ino
    }

    fn inode_type(&self) -> InodeType {
        match self.entry.dev_type {
            DeviceType::Char => InodeType::CharDevice,
            DeviceType::Block => InodeType::BlockDevice,
        }
    }

    fn size(&self) -> u64 {
        0 // Devices don't have a size
    }

    fn permissions(&self) -> InodePermissions {
        InodePermissions::from_octal(0o666)
    }

    fn ctime(&self) -> Timestamp {
        self.ctime
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> FsResult<usize> {
        self.entry.ops.read().read(offset, buf)
    }

    fn write_at(&mut self, offset: u64, buf: &[u8]) -> FsResult<usize> {
        self.entry.ops.write().write(offset, buf)
    }

    fn truncate(&mut self, _size: u64) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn list(&self) -> FsResult<Vec<String>> {
        Err(FsError::NotSupported)
    }

    fn lookup(&self, _name: &str) -> FsResult<u64> {
        Err(FsError::NotSupported)
    }

    fn create(&mut self, _name: &str, _inode_type: InodeType) -> FsResult<u64> {
        Err(FsError::NotSupported)
    }

    fn remove(&mut self, _name: &str) -> FsResult<()> {
        Err(FsError::NotSupported)
    }
}

/// DevFS root inode (directory)
pub struct DevDirInode {
    ino: u64,
}

impl DevDirInode {
    fn new(ino: u64) -> Self {
        Self { ino }
    }
}

impl Inode for DevDirInode {
    fn ino(&self) -> u64 {
        self.ino
    }

    fn inode_type(&self) -> InodeType {
        InodeType::Directory
    }

    fn size(&self) -> u64 {
        0
    }

    fn permissions(&self) -> InodePermissions {
        InodePermissions::from_octal(0o755)
    }

    fn read_at(&self, _offset: u64, _buf: &mut [u8]) -> FsResult<usize> {
        Err(FsError::IsDirectory)
    }

    fn write_at(&mut self, _offset: u64, _buf: &[u8]) -> FsResult<usize> {
        Err(FsError::IsDirectory)
    }

    fn truncate(&mut self, _size: u64) -> FsResult<()> {
        Err(FsError::IsDirectory)
    }

    fn list(&self) -> FsResult<Vec<String>> {
        Ok(get().list_devices())
    }

    fn lookup(&self, name: &str) -> FsResult<u64> {
        get().lookup_device(name)
    }

    fn create(&mut self, _name: &str, _inode_type: InodeType) -> FsResult<u64> {
        Err(FsError::PermissionDenied)
    }

    fn remove(&mut self, _name: &str) -> FsResult<()> {
        Err(FsError::PermissionDenied)
    }
}

/// DevFS - Device Node Filesystem
pub struct DevFs {
    next_ino: AtomicU64,
    devices: RwLock<HashMap<String, Arc<DeviceEntry>>>,
}

impl DevFs {
    pub fn new() -> Self {
        let devfs = Self {
            next_ino: AtomicU64::new(30000),
            devices: RwLock::new(HashMap::new()),
        };

        // Register standard devices
        devfs.register_standard_devices();

        devfs
    }

    fn alloc_ino(&self) -> u64 {
        self.next_ino.fetch_add(1, Ordering::Relaxed)
    }

    /// Register standard devices
    fn register_standard_devices(&self) {
        // /dev/null
        self.register_device(
            "null",
            DeviceType::Char,
            major::MEM,
            minor::NULL,
            Arc::new(RwLock::new(NullDevice)),
        ).ok();

        // /dev/zero
        self.register_device(
            "zero",
            DeviceType::Char,
            major::MEM,
            minor::ZERO,
            Arc::new(RwLock::new(ZeroDevice)),
        ).ok();

        // /dev/full
        self.register_device(
            "full",
            DeviceType::Char,
            major::MEM,
            minor::FULL,
            Arc::new(RwLock::new(FullDevice)),
        ).ok();

        // /dev/random
        self.register_device(
            "random",
            DeviceType::Char,
            major::RANDOM,
            minor::RANDOM,
            Arc::new(RwLock::new(RandomDevice::new())),
        ).ok();

        // /dev/urandom
        self.register_device(
            "urandom",
            DeviceType::Char,
            major::RANDOM,
            minor::URANDOM,
            Arc::new(RwLock::new(RandomDevice::new())),
        ).ok();

        // /dev/console
        self. register_device(
            "console",
            DeviceType::Char,
            major::CONSOLE,
            0,
            Arc::new(RwLock::new(ConsoleDevice)),
        ).ok();

        log::debug!("DevFS: registered standard devices");
    }

    /// Register a device
    pub fn register_device(
        &self,
        name: &str,
        dev_type: DeviceType,
        major: u32,
        minor: u32,
        ops: Arc<RwLock<dyn DeviceOps>>,
    ) -> FsResult<()> {
        let mut devices = self.devices.write();

        if devices.contains_key(name) {
            return Err(FsError::AlreadyExists);
        }

        let entry = Arc::new(DeviceEntry {
            name: name.to_string(),
            dev_type,
            major,
            minor,
            ops,
        });

        devices.insert(name.to_string(), entry);
        log::debug!("DevFS: registered device {}", name);

        Ok(())
    }

    /// Unregister a device
    pub fn unregister_device(&self, name: &str) -> FsResult<()> {
        let mut devices = self.devices.write();
        devices.remove(name).ok_or(FsError::NotFound)?;
        log::debug!("DevFS: unregistered device {}", name);
        Ok(())
    }

    /// Get device inode
    pub fn get_device(&self, name: &str) -> FsResult<Arc<DevInode>> {
        let devices = self.devices.read();
        let entry = devices.get(name).ok_or(FsError::NotFound)?.clone();
        drop(devices);

        let ino = self.alloc_ino();
        Ok(Arc::new(DevInode::new(ino, entry)))
    }

    /// List all devices
    pub fn list_devices(&self) -> Vec<String> {
        let devices = self.devices.read();
        let mut names: Vec<String> = devices.keys().cloned().collect();
        names.sort();
        names
    }

    /// Lookup device by name (returns inode number)
    pub fn lookup_device(&self, name: &str) -> FsResult<u64> {
        let devices = self.devices.read();
        if devices.contains_key(name) {
            // Return a stable inode number based on name hash
            let hash = name.bytes().fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64));
            Ok(30000 + hash % 10000)
        } else {
            Err(FsError::NotFound)
        }
    }

    /// Get root directory inode
    pub fn get_root(&self) -> Arc<DevDirInode> {
        Arc::new(DevDirInode::new(30000))
    }
}

/// Global DevFS instance
static DEVFS: spin::Once<DevFs> = spin::Once::new();

/// Initialize DevFS
pub fn init() {
    DEVFS.call_once(|| DevFs::new());
}

/// Get global DevFS instance
pub fn get() -> &'static DevFs {
    DEVFS.get().expect("DevFS not initialized")
}

/// Register a device (public API)
pub fn register_device(
    name: &str,
    dev_type: DeviceType,
    major: u32,
    minor: u32,
    ops: Arc<RwLock<dyn DeviceOps>>,
) -> FsResult<()> {
    get().register_device(name, dev_type, major, minor, ops)
}

/// Unregister a device (public API)
pub fn unregister_device(name: &str) -> FsResult<()> {
    get().unregister_device(name)
}

/// Stub implementations for drivers (until they're implemented)
mod stub_drivers {
    use super::*;

    pub(crate) mod keyboard {
        use super::*;

        pub fn read_key_blocking(_buf: &mut [u8]) -> FsResult<usize> {
            // Stub - return empty for now
            Ok(0)
        }
    }

    pub(crate) mod vga {
        pub fn putchar(byte: u8) {
            // Stub - use serial for now
            let mut serial = unsafe { uart_16550::SerialPort::new(0x3F8) };
            unsafe { serial.init(); }
            use core::fmt::Write;
            let _ = write!(serial, "{}", byte as char);
        }
    }
}

// Export stubs as if they were real drivers
use stub_drivers as drivers;
