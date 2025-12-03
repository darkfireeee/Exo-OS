//! Named Channels - System-Wide IPC Endpoints
//!
//! Provides named IPC channels similar to Unix named pipes/FIFOs
//! but with much higher performance.
//!
//! ## Features:
//! - Hierarchical namespace (/ipc/service/endpoint)
//! - Access control (permissions like files)
//! - Persistent channels (survive process death)
//! - Multi-client servers (like Unix domain sockets)
//! - Broadcast channels (pub/sub pattern)
//!
//! ## Performance:
//! - Name lookup: O(log n) via B-tree
//! - Connect: ~500 cycles
//! - Send/Recv: Same as regular IPC (~150 cycles inline)

use ::core::sync::atomic::{AtomicU64, AtomicU32, AtomicBool, Ordering};
use alloc::string::{String, ToString};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use alloc::vec;
use alloc::boxed::Box;
use alloc::sync::Arc;
use spin::{Mutex, RwLock};

use super::core::{MpmcRing, ChannelHandle};
use super::IpcError;

/// Maximum channel name length
pub const MAX_CHANNEL_NAME: usize = 256;

/// Channel permissions (similar to file permissions)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelPermissions {
    /// Owner can read
    pub owner_read: bool,
    /// Owner can write
    pub owner_write: bool,
    /// Group can read
    pub group_read: bool,
    /// Group can write
    pub group_write: bool,
    /// Others can read
    pub other_read: bool,
    /// Others can write
    pub other_write: bool,
}

impl ChannelPermissions {
    /// Default permissions (owner rw, group r, others none)
    pub const fn default() -> Self {
        Self {
            owner_read: true,
            owner_write: true,
            group_read: true,
            group_write: false,
            other_read: false,
            other_write: false,
        }
    }
    
    /// Public read/write
    pub const fn public() -> Self {
        Self {
            owner_read: true,
            owner_write: true,
            group_read: true,
            group_write: true,
            other_read: true,
            other_write: true,
        }
    }
    
    /// Owner only
    pub const fn private() -> Self {
        Self {
            owner_read: true,
            owner_write: true,
            group_read: false,
            group_write: false,
            other_read: false,
            other_write: false,
        }
    }
    
    /// Create from Unix-style octal (e.g., 0o644)
    pub fn from_octal(mode: u32) -> Self {
        Self {
            owner_read: (mode & 0o400) != 0,
            owner_write: (mode & 0o200) != 0,
            group_read: (mode & 0o040) != 0,
            group_write: (mode & 0o020) != 0,
            other_read: (mode & 0o004) != 0,
            other_write: (mode & 0o002) != 0,
        }
    }
    
    /// Convert to Unix-style octal
    pub fn to_octal(&self) -> u32 {
        let mut mode = 0u32;
        if self.owner_read { mode |= 0o400; }
        if self.owner_write { mode |= 0o200; }
        if self.group_read { mode |= 0o040; }
        if self.group_write { mode |= 0o020; }
        if self.other_read { mode |= 0o004; }
        if self.other_write { mode |= 0o002; }
        mode
    }
}

impl Default for ChannelPermissions {
    fn default() -> Self {
        Self::default()
    }
}

/// Channel type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelType {
    /// Point-to-point: one sender, one receiver
    Pipe,
    /// Stream: multiple writers, one reader (like Unix FIFO)
    Fifo,
    /// Datagram: preserves message boundaries
    Datagram,
    /// Server: accepts multiple client connections
    Server,
    /// Broadcast: one sender, multiple receivers (pub/sub)
    Broadcast,
}

/// Channel flags
#[derive(Debug, Clone, Copy)]
pub struct ChannelFlags(u32);

impl ChannelFlags {
    pub const NONE: u32 = 0;
    pub const NONBLOCK: u32 = 1 << 0;
    pub const PERSISTENT: u32 = 1 << 1;  // Survives creator death
    pub const EXCLUSIVE: u32 = 1 << 2;   // Only one connection allowed
    pub const CLOEXEC: u32 = 1 << 3;     // Close on exec
    
    pub const fn new(flags: u32) -> Self {
        Self(flags)
    }
    
    pub fn is_nonblock(&self) -> bool { self.0 & Self::NONBLOCK != 0 }
    pub fn is_persistent(&self) -> bool { self.0 & Self::PERSISTENT != 0 }
    pub fn is_exclusive(&self) -> bool { self.0 & Self::EXCLUSIVE != 0 }
    pub fn is_cloexec(&self) -> bool { self.0 & Self::CLOEXEC != 0 }
}

/// Named channel entry in the namespace
struct ChannelEntry {
    /// Channel name
    name: String,
    /// Channel type
    channel_type: ChannelType,
    /// Channel permissions
    permissions: ChannelPermissions,
    /// Channel flags
    flags: ChannelFlags,
    /// Owner process ID
    owner_pid: u64,
    /// Owner group ID
    owner_gid: u64,
    /// Creation timestamp (TSC)
    created_at: u64,
    /// Reference count
    ref_count: AtomicU32,
    /// Is channel active
    active: AtomicBool,
    /// Underlying ring buffer for data
    ring: Arc<MpmcRing>,
    /// Connected client count (for server type)
    client_count: AtomicU32,
    /// Statistics
    stats: ChannelEntryStats,
}

impl ChannelEntry {
    fn new(
        name: String,
        channel_type: ChannelType,
        permissions: ChannelPermissions,
        flags: ChannelFlags,
        owner_pid: u64,
        owner_gid: u64,
    ) -> Self {
        Self {
            name,
            channel_type,
            permissions,
            flags,
            owner_pid,
            owner_gid,
            created_at: super::core::benchmark::rdtsc(),
            ref_count: AtomicU32::new(1),
            active: AtomicBool::new(true),
            ring: Arc::new(MpmcRing::new(1024)),
            client_count: AtomicU32::new(0),
            stats: ChannelEntryStats::new(),
        }
    }
    
    fn add_ref(&self) {
        self.ref_count.fetch_add(1, Ordering::Relaxed);
    }
    
    fn release(&self) -> bool {
        self.ref_count.fetch_sub(1, Ordering::Release) == 1
    }
    
    fn check_read_permission(&self, pid: u64, gid: u64) -> bool {
        if pid == self.owner_pid {
            self.permissions.owner_read
        } else if gid == self.owner_gid {
            self.permissions.group_read
        } else {
            self.permissions.other_read
        }
    }
    
    fn check_write_permission(&self, pid: u64, gid: u64) -> bool {
        if pid == self.owner_pid {
            self.permissions.owner_write
        } else if gid == self.owner_gid {
            self.permissions.group_write
        } else {
            self.permissions.other_write
        }
    }
}

/// Channel statistics
struct ChannelEntryStats {
    messages_sent: AtomicU64,
    messages_recv: AtomicU64,
    bytes_sent: AtomicU64,
    bytes_recv: AtomicU64,
    connect_count: AtomicU64,
}

impl ChannelEntryStats {
    const fn new() -> Self {
        Self {
            messages_sent: AtomicU64::new(0),
            messages_recv: AtomicU64::new(0),
            bytes_sent: AtomicU64::new(0),
            bytes_recv: AtomicU64::new(0),
            connect_count: AtomicU64::new(0),
        }
    }
}

/// Named channel namespace
pub struct ChannelNamespace {
    /// Channel entries indexed by name
    channels: RwLock<BTreeMap<String, Arc<ChannelEntry>>>,
    /// Channel ID counter
    next_id: AtomicU64,
    /// Total channels created
    total_created: AtomicU64,
    /// Currently active channels
    active_count: AtomicU32,
}

impl ChannelNamespace {
    pub const fn new() -> Self {
        Self {
            channels: RwLock::new(BTreeMap::new()),
            next_id: AtomicU64::new(1),
            total_created: AtomicU64::new(0),
            active_count: AtomicU32::new(0),
        }
    }
    
    /// Create a new named channel
    pub fn create(
        &self,
        name: &str,
        channel_type: ChannelType,
        permissions: ChannelPermissions,
        flags: ChannelFlags,
        pid: u64,
        gid: u64,
    ) -> Result<NamedChannelHandle, IpcError> {
        // Validate name
        if name.is_empty() || name.len() > MAX_CHANNEL_NAME {
            return Err(IpcError::InvalidName);
        }
        
        if !name.starts_with('/') {
            return Err(IpcError::InvalidName);
        }
        
        let mut channels = self.channels.write();
        
        // Check if exists
        if channels.contains_key(name) {
            return Err(IpcError::AlreadyExists);
        }
        
        let entry = Arc::new(ChannelEntry::new(
            name.to_string(),
            channel_type,
            permissions,
            flags,
            pid,
            gid,
        ));
        
        channels.insert(name.to_string(), entry.clone());
        
        self.total_created.fetch_add(1, Ordering::Relaxed);
        self.active_count.fetch_add(1, Ordering::Relaxed);
        
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        
        Ok(NamedChannelHandle {
            id,
            name: name.to_string(),
            entry,
            can_read: true,
            can_write: true,
        })
    }
    
    /// Open an existing named channel
    pub fn open(
        &self,
        name: &str,
        read: bool,
        write: bool,
        pid: u64,
        gid: u64,
    ) -> Result<NamedChannelHandle, IpcError> {
        let channels = self.channels.read();
        
        let entry = channels.get(name).ok_or(IpcError::NotFound)?;
        
        if !entry.active.load(Ordering::Acquire) {
            return Err(IpcError::ChannelClosed);
        }
        
        // Check permissions
        if read && !entry.check_read_permission(pid, gid) {
            return Err(IpcError::PermissionDenied);
        }
        
        if write && !entry.check_write_permission(pid, gid) {
            return Err(IpcError::PermissionDenied);
        }
        
        // Check exclusive
        if entry.flags.is_exclusive() && entry.client_count.load(Ordering::Relaxed) > 0 {
            return Err(IpcError::Busy);
        }
        
        entry.add_ref();
        entry.client_count.fetch_add(1, Ordering::Relaxed);
        entry.stats.connect_count.fetch_add(1, Ordering::Relaxed);
        
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        
        Ok(NamedChannelHandle {
            id,
            name: name.to_string(),
            entry: entry.clone(),
            can_read: read,
            can_write: write,
        })
    }
    
    /// Unlink (remove) a named channel
    pub fn unlink(&self, name: &str, pid: u64) -> Result<(), IpcError> {
        let mut channels = self.channels.write();
        
        let entry = channels.get(name).ok_or(IpcError::NotFound)?;
        
        // Only owner can unlink
        if entry.owner_pid != pid {
            return Err(IpcError::PermissionDenied);
        }
        
        entry.active.store(false, Ordering::Release);
        channels.remove(name);
        
        self.active_count.fetch_sub(1, Ordering::Relaxed);
        
        Ok(())
    }
    
    /// List channels matching prefix
    pub fn list(&self, prefix: &str) -> Vec<String> {
        let channels = self.channels.read();
        
        channels
            .keys()
            .filter(|name| name.starts_with(prefix))
            .cloned()
            .collect()
    }
    
    /// Get channel info
    pub fn stat(&self, name: &str) -> Option<ChannelInfo> {
        let channels = self.channels.read();
        
        channels.get(name).map(|entry| ChannelInfo {
            name: entry.name.clone(),
            channel_type: entry.channel_type,
            permissions: entry.permissions,
            owner_pid: entry.owner_pid,
            owner_gid: entry.owner_gid,
            ref_count: entry.ref_count.load(Ordering::Relaxed),
            client_count: entry.client_count.load(Ordering::Relaxed),
            messages_sent: entry.stats.messages_sent.load(Ordering::Relaxed),
            messages_recv: entry.stats.messages_recv.load(Ordering::Relaxed),
        })
    }
}

/// Public channel information
#[derive(Debug, Clone)]
pub struct ChannelInfo {
    pub name: String,
    pub channel_type: ChannelType,
    pub permissions: ChannelPermissions,
    pub owner_pid: u64,
    pub owner_gid: u64,
    pub ref_count: u32,
    pub client_count: u32,
    pub messages_sent: u64,
    pub messages_recv: u64,
}

/// Handle to a named channel
pub struct NamedChannelHandle {
    id: u64,
    name: String,
    entry: Arc<ChannelEntry>,
    can_read: bool,
    can_write: bool,
}

impl NamedChannelHandle {
    /// Send data through the channel
    pub fn send(&self, data: &[u8]) -> Result<(), IpcError> {
        if !self.can_write {
            return Err(IpcError::PermissionDenied);
        }
        
        if !self.entry.active.load(Ordering::Acquire) {
            return Err(IpcError::ChannelClosed);
        }
        
        match self.entry.ring.try_send_inline(data) {
            Ok(()) => {
                self.entry.stats.messages_sent.fetch_add(1, Ordering::Relaxed);
                self.entry.stats.bytes_sent.fetch_add(data.len() as u64, Ordering::Relaxed);
                Ok(())
            }
            Err(_) => Err(IpcError::WouldBlock),
        }
    }
    
    /// Send with blocking
    pub fn send_blocking(&self, data: &[u8]) -> Result<(), IpcError> {
        if !self.can_write {
            return Err(IpcError::PermissionDenied);
        }
        
        if !self.entry.active.load(Ordering::Acquire) {
            return Err(IpcError::ChannelClosed);
        }
        
        self.entry.ring.send_blocking(data);
        self.entry.stats.messages_sent.fetch_add(1, Ordering::Relaxed);
        self.entry.stats.bytes_sent.fetch_add(data.len() as u64, Ordering::Relaxed);
        Ok(())
    }
    
    /// Receive data from the channel
    pub fn recv(&self) -> Result<Vec<u8>, IpcError> {
        if !self.can_read {
            return Err(IpcError::PermissionDenied);
        }
        
        if !self.entry.active.load(Ordering::Acquire) {
            return Err(IpcError::ChannelClosed);
        }
        
        let mut buffer = vec![0u8; 4096];
        match self.entry.ring.try_recv(&mut buffer) {
            Ok(size) => {
                buffer.truncate(size);
                self.entry.stats.messages_recv.fetch_add(1, Ordering::Relaxed);
                self.entry.stats.bytes_recv.fetch_add(size as u64, Ordering::Relaxed);
                Ok(buffer)
            }
            Err(_) => Err(IpcError::WouldBlock),
        }
    }
    
    /// Receive with blocking
    pub fn recv_blocking(&self) -> Result<Vec<u8>, IpcError> {
        if !self.can_read {
            return Err(IpcError::PermissionDenied);
        }
        
        if !self.entry.active.load(Ordering::Acquire) {
            return Err(IpcError::ChannelClosed);
        }
        
        let mut buffer = vec![0u8; 4096];
        match self.entry.ring.recv_blocking(&mut buffer) {
            Ok(size) => {
                buffer.truncate(size);
                self.entry.stats.messages_recv.fetch_add(1, Ordering::Relaxed);
                self.entry.stats.bytes_recv.fetch_add(size as u64, Ordering::Relaxed);
                Ok(buffer)
            }
            Err(_) => Err(IpcError::ChannelClosed),
        }
    }
    
    /// Get channel name
    pub fn name(&self) -> &str {
        &self.name
    }
    
    /// Get channel type
    pub fn channel_type(&self) -> ChannelType {
        self.entry.channel_type
    }
    
    /// Check if channel is still active
    pub fn is_active(&self) -> bool {
        self.entry.active.load(Ordering::Acquire)
    }
}

impl Drop for NamedChannelHandle {
    fn drop(&mut self) {
        self.entry.client_count.fetch_sub(1, Ordering::Relaxed);
        
        if self.entry.release() && !self.entry.flags.is_persistent() {
            // Last reference and not persistent, mark inactive
            self.entry.active.store(false, Ordering::Release);
        }
    }
}

// =============================================================================
// GLOBAL NAMESPACE
// =============================================================================

/// Global channel namespace
static GLOBAL_NAMESPACE: ChannelNamespace = ChannelNamespace::new();

/// Create a named channel
pub fn create_channel(
    name: &str,
    channel_type: ChannelType,
    permissions: ChannelPermissions,
    flags: ChannelFlags,
) -> Result<NamedChannelHandle, IpcError> {
    // TODO: Get real PID/GID from current process
    let pid = 0;
    let gid = 0;
    
    GLOBAL_NAMESPACE.create(name, channel_type, permissions, flags, pid, gid)
}

/// Open a named channel
pub fn open_channel(name: &str, read: bool, write: bool) -> Result<NamedChannelHandle, IpcError> {
    // TODO: Get real PID/GID from current process
    let pid = 0;
    let gid = 0;
    
    GLOBAL_NAMESPACE.open(name, read, write, pid, gid)
}

/// Unlink a named channel
pub fn unlink_channel(name: &str) -> Result<(), IpcError> {
    // TODO: Get real PID/GID from current process
    let pid = 0;
    
    GLOBAL_NAMESPACE.unlink(name, pid)
}

/// List channels
pub fn list_channels(prefix: &str) -> Vec<String> {
    GLOBAL_NAMESPACE.list(prefix)
}

/// Get channel info
pub fn stat_channel(name: &str) -> Option<ChannelInfo> {
    GLOBAL_NAMESPACE.stat(name)
}

// =============================================================================
// CONVENIENCE MACROS
// =============================================================================

/// Create a pipe-style channel pair
pub fn pipe() -> Result<(NamedChannelHandle, NamedChannelHandle), IpcError> {
    static PIPE_COUNTER: AtomicU64 = AtomicU64::new(0);
    
    let id = PIPE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = alloc::format!("/pipe/anonymous/{}", id);
    
    let writer = create_channel(
        &name,
        ChannelType::Pipe,
        ChannelPermissions::private(),
        ChannelFlags::new(0),
    )?;
    
    let reader = open_channel(&name, true, false)?;
    
    Ok((writer, reader))
}

/// Create a FIFO (named pipe)
pub fn mkfifo(name: &str, mode: u32) -> Result<(), IpcError> {
    let _handle = create_channel(
        name,
        ChannelType::Fifo,
        ChannelPermissions::from_octal(mode),
        ChannelFlags::new(ChannelFlags::PERSISTENT),
    )?;
    
    // Handle is dropped but channel persists due to PERSISTENT flag
    Ok(())
}
