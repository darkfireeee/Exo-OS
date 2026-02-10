//! SysFS - Kernel Object Hierarchy Filesystem
//!
//! ## Structure
//! ```
//! /sys/
//!   ├── block/           - Block devices
//!   ├── bus/             - Bus types
//!   ├── class/           - Device classes
//!   ├── dev/             - Device char/block mappings
//!   ├── devices/         - Device hierarchy
//!   ├── firmware/        - Firmware information
//!   ├── fs/              - Filesystem information
//!   ├── kernel/          - Kernel parameters
//!   │   ├── hostname     - System hostname (rw)
//!   │   ├── ostype       - OS type
//!   │   ├── osrelease    - OS release
//!   │   ├── version      - Kernel version
//!   │   └── debug/       - Debug parameters
//!   ├── module/          - Loaded kernel modules
//!   └── power/           - Power management
//!       ├── state        - System power state
//!       └── disk         - Disk power mode
//! ```
//!
//! ## Features
//! - Hierarchical kernel object representation
//! - Some writable parameters (hostname, power state)
//! - Attribute-based interface
//! - Hotplug support

use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use alloc::string::{String, ToString};
use alloc::format;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::RwLock;
use hashbrown::HashMap;

use crate::fs::core::types::{
    Inode, InodeType, InodePermissions, Timestamp,
};
use crate::fs::{FsError, FsResult};

/// SysFS entry types
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SysEntry {
    /// Root directory
    Root,

    /// Top-level directories
    Block,
    Bus,
    Class,
    Dev,
    Devices,
    Firmware,
    Fs,
    Kernel,
    Module,
    Power,

    /// Kernel parameters
    KernelHostname,
    KernelOstype,
    KernelOsrelease,
    KernelVersion,
    KernelDebug,

    /// Power management
    PowerState,
    PowerDisk,

    /// Filesystem info
    FsInfo(String),

    /// Custom entry
    Custom(String),
}

/// System hostname (writable)
static HOSTNAME: RwLock<String> = RwLock::new(String::new());

/// Initialize hostname
fn init_hostname() {
    *HOSTNAME.write() = "exo-os".to_string();
}

/// Generate content for SysFS entries
fn generate_content(entry: &SysEntry) -> Vec<u8> {
    match entry {
        SysEntry::KernelHostname => {
            let hostname = HOSTNAME.read();
            format!("{}\n", *hostname).into_bytes()
        }
        SysEntry::KernelOstype => {
            "Exo-OS\n".as_bytes().to_vec()
        }
        SysEntry::KernelOsrelease => {
            "0.7.0\n".as_bytes().to_vec()
        }
        SysEntry::KernelVersion => {
            "#1 SMP Mon Jan 1 00:00:00 UTC 2025\n".as_bytes().to_vec()
        }
        SysEntry::PowerState => {
            "mem disk\n".as_bytes().to_vec()
        }
        SysEntry::PowerDisk => {
            "platform shutdown reboot\n".as_bytes().to_vec()
        }
        SysEntry::FsInfo(name) => {
            match name.as_str() {
                "ext4plus" => "ext4plus supports: journal, extents, 64bit\n".as_bytes().to_vec(),
                "tmpfs" => "tmpfs is a memory-based filesystem\n".as_bytes().to_vec(),
                _ => Vec::new(),
            }
        }
        _ => Vec::new(),
    }
}

/// Check if an entry is writable
fn is_writable(entry: &SysEntry) -> bool {
    matches!(
        entry,
        SysEntry::KernelHostname | SysEntry::PowerState | SysEntry::PowerDisk
    )
}

/// Write to a SysFS entry
fn write_content(entry: &SysEntry, data: &[u8]) -> FsResult<()> {
    match entry {
        SysEntry::KernelHostname => {
            let s = core::str::from_utf8(data)
                .map_err(|_| FsError::InvalidData)?;
            let hostname = s.trim().to_string();

            if hostname.len() > 64 {
                return Err(FsError::InvalidArgument);
            }

            *HOSTNAME.write() = hostname;
            Ok(())
        }
        SysEntry::PowerState => {
            // Stub - would trigger power state change
            log::info!("Power state change requested: {:?}", core::str::from_utf8(data));
            Ok(())
        }
        SysEntry::PowerDisk => {
            // Stub - would configure disk power mode
            log::info!("Disk power mode change requested: {:?}", core::str::from_utf8(data));
            Ok(())
        }
        _ => Err(FsError::PermissionDenied),
    }
}

/// SysFS Inode
pub struct SysInode {
    ino: u64,
    entry: SysEntry,
}

impl SysInode {
    fn new(ino: u64, entry: SysEntry) -> Self {
        Self { ino, entry }
    }

    fn is_directory(&self) -> bool {
        matches!(
            self.entry,
            SysEntry::Root
                | SysEntry::Block
                | SysEntry::Bus
                | SysEntry::Class
                | SysEntry::Dev
                | SysEntry::Devices
                | SysEntry::Firmware
                | SysEntry::Fs
                | SysEntry::Kernel
                | SysEntry::Module
                | SysEntry::Power
                | SysEntry::KernelDebug
        )
    }

    fn list_directory(&self) -> Vec<String> {
        match &self.entry {
            SysEntry::Root => {
                vec![
                    "block".to_string(),
                    "bus".to_string(),
                    "class".to_string(),
                    "dev".to_string(),
                    "devices".to_string(),
                    "firmware".to_string(),
                    "fs".to_string(),
                    "kernel".to_string(),
                    "module".to_string(),
                    "power".to_string(),
                ]
            }
            SysEntry::Kernel => {
                vec![
                    "hostname".to_string(),
                    "ostype".to_string(),
                    "osrelease".to_string(),
                    "version".to_string(),
                    "debug".to_string(),
                ]
            }
            SysEntry::Power => {
                vec![
                    "state".to_string(),
                    "disk".to_string(),
                ]
            }
            SysEntry::Fs => {
                vec![
                    "ext4plus".to_string(),
                    "tmpfs".to_string(),
                ]
            }
            SysEntry::Block => {
                // Stub - would list actual block devices
                vec!["ram0".to_string()]
            }
            SysEntry::Class => {
                // Stub - would list device classes
                vec!["block".to_string(), "net".to_string()]
            }
            _ => Vec::new(),
        }
    }
}

impl Inode for SysInode {
    fn ino(&self) -> u64 {
        self.ino
    }

    fn inode_type(&self) -> InodeType {
        if self.is_directory() {
            InodeType::Directory
        } else {
            InodeType::File
        }
    }

    fn size(&self) -> u64 {
        if self.is_directory() {
            0
        } else {
            generate_content(&self.entry).len() as u64
        }
    }

    fn permissions(&self) -> InodePermissions {
        if self.is_directory() {
            InodePermissions::from_octal(0o555)
        } else if is_writable(&self.entry) {
            InodePermissions::from_octal(0o644)
        } else {
            InodePermissions::from_octal(0o444)
        }
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> FsResult<usize> {
        if self.is_directory() {
            return Err(FsError::IsDirectory);
        }

        let content = generate_content(&self.entry);
        let offset = offset as usize;

        if offset >= content.len() {
            return Ok(0);
        }

        let to_read = buf.len().min(content.len() - offset);
        buf[..to_read].copy_from_slice(&content[offset..offset + to_read]);

        Ok(to_read)
    }

    fn write_at(&mut self, _offset: u64, buf: &[u8]) -> FsResult<usize> {
        if !is_writable(&self.entry) {
            return Err(FsError::PermissionDenied);
        }

        write_content(&self.entry, buf)?;
        Ok(buf.len())
    }

    fn truncate(&mut self, _size: u64) -> FsResult<()> {
        if is_writable(&self.entry) {
            Ok(())
        } else {
            Err(FsError::PermissionDenied)
        }
    }

    fn list(&self) -> FsResult<Vec<String>> {
        if !self.is_directory() {
            return Err(FsError::NotDirectory);
        }

        Ok(self.list_directory())
    }

    fn lookup(&self, name: &str) -> FsResult<u64> {
        if !self.is_directory() {
            return Err(FsError::NotDirectory);
        }

        let entries = self.list_directory();
        if entries.contains(&name.to_string()) {
            Ok(self.ino + name.len() as u64)
        } else {
            Err(FsError::NotFound)
        }
    }

    fn create(&mut self, _name: &str, _inode_type: InodeType) -> FsResult<u64> {
        Err(FsError::PermissionDenied)
    }

    fn remove(&mut self, _name: &str) -> FsResult<()> {
        Err(FsError::PermissionDenied)
    }
}

/// SysFS - Kernel Object Hierarchy Filesystem
pub struct SysFs {
    next_ino: AtomicU64,
    entries: RwLock<HashMap<String, Arc<SysInode>>>,
}

impl SysFs {
    pub fn new() -> Self {
        Self {
            next_ino: AtomicU64::new(20000),
            entries: RwLock::new(HashMap::new()),
        }
    }

    fn alloc_ino(&self) -> u64 {
        self.next_ino.fetch_add(1, Ordering::Relaxed)
    }

    /// Get or create an inode for a sysfs entry
    pub fn get_inode(&self, path: &str) -> FsResult<Arc<SysInode>> {
        let entry = self.parse_path(path)?;

        let mut entries = self.entries.write();

        if let Some(inode) = entries.get(path) {
            return Ok(inode.clone());
        }

        let ino = self.alloc_ino();
        let inode = Arc::new(SysInode::new(ino, entry));
        entries.insert(path.to_string(), inode.clone());

        Ok(inode)
    }

    /// Parse a path to determine the SysEntry type
    fn parse_path(&self, path: &str) -> FsResult<SysEntry> {
        let parts: Vec<&str> = path
            .trim_start_matches('/')
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();

        if parts.is_empty() {
            return Ok(SysEntry::Root);
        }

        match parts[0] {
            "block" => Ok(SysEntry::Block),
            "bus" => Ok(SysEntry::Bus),
            "class" => Ok(SysEntry::Class),
            "dev" => Ok(SysEntry::Dev),
            "devices" => Ok(SysEntry::Devices),
            "firmware" => Ok(SysEntry::Firmware),
            "fs" => {
                if parts.len() == 1 {
                    Ok(SysEntry::Fs)
                } else {
                    Ok(SysEntry::FsInfo(parts[1].to_string()))
                }
            }
            "kernel" => {
                if parts.len() == 1 {
                    Ok(SysEntry::Kernel)
                } else {
                    match parts[1] {
                        "hostname" => Ok(SysEntry::KernelHostname),
                        "ostype" => Ok(SysEntry::KernelOstype),
                        "osrelease" => Ok(SysEntry::KernelOsrelease),
                        "version" => Ok(SysEntry::KernelVersion),
                        "debug" => Ok(SysEntry::KernelDebug),
                        _ => Err(FsError::NotFound),
                    }
                }
            }
            "module" => Ok(SysEntry::Module),
            "power" => {
                if parts.len() == 1 {
                    Ok(SysEntry::Power)
                } else {
                    match parts[1] {
                        "state" => Ok(SysEntry::PowerState),
                        "disk" => Ok(SysEntry::PowerDisk),
                        _ => Err(FsError::NotFound),
                    }
                }
            }
            _ => Ok(SysEntry::Custom(path.to_string())),
        }
    }

    /// Register a custom sysfs entry (for device drivers)
    pub fn register_entry(&self, path: &str, entry: SysEntry) -> FsResult<()> {
        let mut entries = self.entries.write();

        if entries.contains_key(path) {
            return Err(FsError::AlreadyExists);
        }

        let ino = self.alloc_ino();
        let inode = Arc::new(SysInode::new(ino, entry));
        entries.insert(path.to_string(), inode);

        Ok(())
    }

    /// Unregister a custom sysfs entry
    pub fn unregister_entry(&self, path: &str) -> FsResult<()> {
        let mut entries = self.entries.write();
        entries.remove(path).ok_or(FsError::NotFound)?;
        Ok(())
    }
}

/// Global SysFS instance
static SYSFS: spin::Once<SysFs> = spin::Once::new();

/// Initialize SysFS
pub fn init() {
    init_hostname();
    SYSFS.call_once(|| SysFs::new());
}

/// Get global SysFS instance
pub fn get() -> &'static SysFs {
    SYSFS.get().expect("SysFS not initialized")
}

/// Get current hostname
pub fn get_hostname() -> String {
    HOSTNAME.read().clone()
}

/// Set hostname
pub fn set_hostname(name: &str) -> FsResult<()> {
    if name.len() > 64 {
        return Err(FsError::InvalidArgument);
    }

    *HOSTNAME.write() = name.to_string();
    Ok(())
}
