//! # Kernel Interface - Capability Cache and Native API Bridge
//!
//! Provides the interface between POSIX-X layer and Exo-OS kernel.
//! Implements capability caching for fast path lookups.
//!
//! ## Capability Cache
//!
//! LRU cache for path → capability mappings:
//! - Hit rate target: 90%+
//! - Hit: 50 cycles
//! - Miss: 2000 cycles (lookup + create capability)
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    POSIX-X Request                          │
//! └───────────────────────────┬─────────────────────────────────┘
//!                             │
//!                             ▼
//!              ┌─────────────────────────────┐
//!              │     Capability Cache        │
//!              │   (LRU, 1024 entries)       │
//!              └──────────┬──────────────────┘
//!                         │
//!          ┌──────────────┼──────────────┐
//!          │ HIT          │              │ MISS
//!          ▼ (50 cy)      │              ▼ (2000 cy)
//!   ┌────────────┐        │       ┌────────────┐
//!   │  Return    │        │       │  Kernel    │
//!   │ Capability │        │       │  Resolve   │
//!   └────────────┘        │       └─────┬──────┘
//!                         │             │
//!                         │             ▼
//!                         │      ┌────────────┐
//!                         │      │ Insert to  │
//!                         │      │   Cache    │
//!                         │      └────────────┘
//!                         │
//!                         ▼
//!              ┌─────────────────────────────┐
//!              │       Fusion Ring IPC       │
//!              │      (347 cycles avg)       │
//!              └─────────────────────────────┘
//! ```

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::PosixXError;

/// Maximum entries in capability cache
pub const CACHE_SIZE: usize = 1024;

/// Capability cache hit latency target (cycles)
pub const CACHE_HIT_CYCLES: u32 = 50;

/// Capability cache miss latency (cycles)
pub const CACHE_MISS_CYCLES: u32 = 2000;

/// Fusion Ring inline message threshold (bytes)
pub const FUSION_INLINE_THRESHOLD: usize = 56;

/// Fusion Ring inline send latency (cycles)
pub const FUSION_INLINE_CYCLES: u32 = 347;

/// Fusion Ring zero-copy send latency (cycles)
pub const FUSION_ZEROCOPY_CYCLES: u32 = 800;

/// Capability handle (opaque to userspace)
///
/// Represents a kernel capability that grants specific access rights
/// to a resource. Capabilities are the foundation of Exo-OS security.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct CapabilityHandle(u64);

impl CapabilityHandle {
    /// Create a new capability handle from raw value
    ///
    /// # Safety
    /// The caller must ensure the raw value represents a valid capability.
    #[inline]
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// Get raw capability value
    #[inline]
    pub const fn as_raw(&self) -> u64 {
        self.0
    }

    /// Check if capability is valid (non-zero)
    #[inline]
    pub const fn is_valid(&self) -> bool {
        self.0 != 0
    }

    /// Invalid/null capability
    pub const INVALID: Self = Self(0);
}

impl Default for CapabilityHandle {
    fn default() -> Self {
        Self::INVALID
    }
}

/// Capability type classification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum CapabilityType {
    /// File system object (file, directory)
    File = 0,
    /// Network socket
    Socket = 1,
    /// Process/thread handle
    Process = 2,
    /// Memory region
    Memory = 3,
    /// IPC channel (Fusion Ring)
    Ipc = 4,
    /// Device handle
    Device = 5,
    /// Timer handle
    Timer = 6,
    /// Signal handle
    Signal = 7,
}

/// Capability rights flags
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilityRights(u32);

impl CapabilityRights {
    /// Read access
    pub const READ: Self = Self(1 << 0);
    /// Write access
    pub const WRITE: Self = Self(1 << 1);
    /// Execute access
    pub const EXECUTE: Self = Self(1 << 2);
    /// Seek access
    pub const SEEK: Self = Self(1 << 3);
    /// Map into memory
    pub const MMAP: Self = Self(1 << 4);
    /// Create child resources
    pub const CREATE: Self = Self(1 << 5);
    /// Delete/unlink
    pub const DELETE: Self = Self(1 << 6);
    /// Change attributes
    pub const ATTR: Self = Self(1 << 7);
    /// Delegate capability
    pub const DELEGATE: Self = Self(1 << 8);

    /// No rights
    pub const NONE: Self = Self(0);

    /// All rights
    pub const ALL: Self = Self(0x1FF);

    /// Check if contains specific rights
    #[inline]
    pub const fn contains(&self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    /// Combine rights
    #[inline]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Intersect rights
    #[inline]
    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
}

/// Capability cache entry with LRU tracking
#[derive(Debug, Clone)]
struct CacheEntry {
    /// Capability handle
    handle: CapabilityHandle,
    /// Capability type
    cap_type: CapabilityType,
    /// Granted rights
    rights: CapabilityRights,
    /// Last access timestamp (for LRU eviction)
    last_access: u64,
    /// Access count (for statistics)
    access_count: u64,
}

/// LRU Capability Cache
///
/// High-performance cache for path → capability mappings.
/// Uses a BTreeMap for O(log n) lookups with LRU eviction.
pub struct CapabilityCache {
    /// Path to capability mapping
    entries: BTreeMap<String, CacheEntry>,
    /// Maximum cache size
    max_size: usize,
    /// Monotonic timestamp counter
    timestamp: AtomicU64,
    /// Cache statistics
    stats: CacheStats,
}

/// Cache statistics
#[derive(Debug, Default)]
pub struct CacheStats {
    /// Total lookups
    pub lookups: AtomicU64,
    /// Cache hits
    pub hits: AtomicU64,
    /// Cache misses
    pub misses: AtomicU64,
    /// Evictions
    pub evictions: AtomicU64,
    /// Invalidations
    pub invalidations: AtomicU64,
}

impl CacheStats {
    /// Create new stats
    pub const fn new() -> Self {
        Self {
            lookups: AtomicU64::new(0),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
            invalidations: AtomicU64::new(0),
        }
    }

    /// Get hit ratio as percentage
    pub fn hit_ratio(&self) -> f32 {
        let hits = self.hits.load(Ordering::Relaxed);
        let lookups = self.lookups.load(Ordering::Relaxed);
        if lookups == 0 {
            return 0.0;
        }
        (hits as f32 / lookups as f32) * 100.0
    }
}

impl CapabilityCache {
    /// Create new capability cache
    pub fn new(max_size: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            max_size,
            timestamp: AtomicU64::new(0),
            stats: CacheStats::new(),
        }
    }

    /// Get capability for path (cached lookup)
    ///
    /// # Performance
    /// - Hit: ~50 cycles
    /// - Miss: Returns None (caller should resolve)
    #[inline]
    pub fn get(&mut self, path: &str) -> Option<(CapabilityHandle, CapabilityRights)> {
        self.stats.lookups.fetch_add(1, Ordering::Relaxed);

        if let Some(entry) = self.entries.get_mut(path) {
            // Update LRU timestamp
            entry.last_access = self.next_timestamp();
            entry.access_count += 1;
            self.stats.hits.fetch_add(1, Ordering::Relaxed);
            Some((entry.handle, entry.rights))
        } else {
            self.stats.misses.fetch_add(1, Ordering::Relaxed);
            None
        }
    }

    /// Insert capability into cache
    ///
    /// Evicts LRU entry if cache is full.
    pub fn insert(
        &mut self,
        path: String,
        handle: CapabilityHandle,
        cap_type: CapabilityType,
        rights: CapabilityRights,
    ) {
        // Evict if at capacity
        if self.entries.len() >= self.max_size {
            self.evict_lru();
        }

        let entry = CacheEntry {
            handle,
            cap_type,
            rights,
            last_access: self.next_timestamp(),
            access_count: 1,
        };

        self.entries.insert(path, entry);
    }

    /// Evict least recently used entry
    fn evict_lru(&mut self) {
        // Find LRU entry (minimum last_access)
        let lru_key = self
            .entries
            .iter()
            .min_by_key(|(_, v)| v.last_access)
            .map(|(k, _)| k.clone());

        if let Some(key) = lru_key {
            self.entries.remove(&key);
            self.stats.evictions.fetch_add(1, Ordering::Relaxed);
            log::trace!("Cache evicted: {}", key);
        }
    }

    /// Get next monotonic timestamp
    #[inline]
    fn next_timestamp(&self) -> u64 {
        self.timestamp.fetch_add(1, Ordering::Relaxed)
    }

    /// Get cache statistics
    pub fn stats(&self) -> &CacheStats {
        &self.stats
    }

    /// Get current cache size
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if cache is empty
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Invalidate cache entry for path
    pub fn invalidate(&mut self, path: &str) {
        if self.entries.remove(path).is_some() {
            self.stats.invalidations.fetch_add(1, Ordering::Relaxed);
            log::trace!("Cache invalidated: {}", path);
        }
    }

    /// Invalidate all entries matching prefix
    pub fn invalidate_prefix(&mut self, prefix: &str) {
        let keys_to_remove: Vec<String> = self
            .entries
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect();

        for key in keys_to_remove {
            self.entries.remove(&key);
            self.stats.invalidations.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Clear all cache entries
    pub fn clear(&mut self) {
        let count = self.entries.len() as u64;
        self.entries.clear();
        self.stats.invalidations.fetch_add(count, Ordering::Relaxed);
        log::debug!("Cache cleared: {} entries", count);
    }
}

/// Global capability cache
static mut CAPABILITY_CACHE: Option<CapabilityCache> = None;

/// Initialize capability cache
///
/// Must be called before using any cache functions.
pub fn init_capability_cache() -> Result<(), PosixXError> {
    // Safety: Called only during single-threaded initialization
    unsafe {
        if CAPABILITY_CACHE.is_some() {
            return Err(PosixXError::InternalError(
                "Capability cache already initialized".into(),
            ));
        }
        CAPABILITY_CACHE = Some(CapabilityCache::new(CACHE_SIZE));
    }
    log::info!(
        "Capability cache initialized: {} entries max, {} cycle hit target",
        CACHE_SIZE,
        CACHE_HIT_CYCLES
    );
    Ok(())
}

/// Get capability from cache or resolve from kernel
///
/// # Performance
/// - Cache hit: ~50 cycles
/// - Cache miss: ~2000 cycles
pub fn get_capability(path: &str) -> Result<CapabilityHandle, PosixXError> {
    let cache = unsafe {
        CAPABILITY_CACHE
            .as_mut()
            .ok_or_else(|| PosixXError::InternalError("Cache not initialized".into()))?
    };

    // Try cache first (50 cycles target)
    if let Some((handle, _rights)) = cache.get(path) {
        crate::stats().record_cache_hit();
        return Ok(handle);
    }

    // Cache miss - resolve from kernel (2000 cycles)
    crate::stats().record_cache_miss();
    let (handle, cap_type, rights) = resolve_path_to_capability(path)?;
    cache.insert(path.into(), handle, cap_type, rights);
    Ok(handle)
}

/// Resolve path to capability via kernel syscall
///
/// This is the slow path (~2000 cycles) that queries the kernel
/// for a capability to access the specified path.
fn resolve_path_to_capability(
    path: &str,
) -> Result<(CapabilityHandle, CapabilityType, CapabilityRights), PosixXError> {
    log::trace!("Resolving capability for path: {}", path);

    // TODO: Implement actual kernel syscall via exo_std
    // For now, create a deterministic handle based on path

    // Determine capability type from path
    let cap_type = if path.starts_with("/dev/") {
        CapabilityType::Device
    } else if path.starts_with("/proc/") || path.starts_with("/sys/") {
        CapabilityType::Process
    } else if path.starts_with("/tmp/") || path.starts_with("/run/") {
        CapabilityType::Ipc
    } else {
        CapabilityType::File
    };

    // Default rights for files
    let rights = CapabilityRights::READ.union(CapabilityRights::WRITE);

    // Generate handle from path hash (placeholder)
    let mut hash: u64 = 0xcbf29ce484222325; // FNV-1a offset basis
    for byte in path.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3); // FNV-1a prime
    }

    let handle = CapabilityHandle::from_raw(hash);
    Ok((handle, cap_type, rights))
}

/// Invalidate capability cache entry
pub fn invalidate_capability(path: &str) {
    if let Some(cache) = unsafe { CAPABILITY_CACHE.as_mut() } {
        cache.invalidate(path);
    }
}

/// Get cache statistics
pub fn cache_stats() -> Option<&'static CacheStats> {
    unsafe { CAPABILITY_CACHE.as_ref().map(|c| c.stats()) }
}

/// Kernel interface for IPC operations using Fusion Rings
///
/// Provides high-performance IPC with:
/// - Inline messages (≤56 bytes): 347 cycles
/// - Zero-copy messages (>56 bytes): 800 cycles
pub struct KernelInterface;

impl KernelInterface {
    /// Send message via Fusion Ring (347 cycles inline)
    ///
    /// Automatically selects inline or zero-copy based on size.
    ///
    /// # Arguments
    /// * `ring_id` - Fusion Ring identifier
    /// * `data` - Data to send
    ///
    /// # Returns
    /// Number of bytes sent, or error
    #[inline]
    pub fn fusion_ring_send(ring_id: u64, data: &[u8]) -> Result<usize, PosixXError> {
        if data.len() <= FUSION_INLINE_THRESHOLD {
            // Inline path: 347 cycles
            Self::fusion_ring_send_inline(ring_id, data)
        } else {
            // Zero-copy path: 800 cycles
            Self::fusion_ring_send_zerocopy(ring_id, data)
        }
    }

    /// Send inline message via Fusion Ring
    ///
    /// # Performance
    /// Target: 347 cycles
    #[inline]
    fn fusion_ring_send_inline(ring_id: u64, data: &[u8]) -> Result<usize, PosixXError> {
        log::trace!(
            "Fusion ring inline send: {} bytes to ring {}",
            data.len(),
            ring_id
        );
        // TODO: Implement via kernel syscall
        Ok(data.len())
    }

    /// Send zero-copy message via Fusion Ring
    ///
    /// # Performance
    /// Target: 800 cycles
    fn fusion_ring_send_zerocopy(ring_id: u64, data: &[u8]) -> Result<usize, PosixXError> {
        log::trace!(
            "Fusion ring zero-copy send: {} bytes to ring {}",
            data.len(),
            ring_id
        );
        // TODO: Implement via kernel syscall with shared memory
        Ok(data.len())
    }

    /// Receive message via Fusion Ring
    ///
    /// # Arguments
    /// * `ring_id` - Fusion Ring identifier
    /// * `buffer` - Buffer to receive data
    ///
    /// # Returns
    /// Number of bytes received, or error
    #[inline]
    pub fn fusion_ring_recv(ring_id: u64, buffer: &mut [u8]) -> Result<usize, PosixXError> {
        log::trace!(
            "Fusion ring recv from ring {}, buffer size {}",
            ring_id,
            buffer.len()
        );
        // TODO: Implement via kernel syscall
        Ok(0)
    }

    /// Create new Fusion Ring
    ///
    /// # Arguments
    /// * `capacity` - Number of slots in the ring (power of 2)
    ///
    /// # Returns
    /// Ring identifier
    pub fn fusion_ring_create(capacity: usize) -> Result<u64, PosixXError> {
        if !capacity.is_power_of_two() {
            return Err(PosixXError::InvalidArgument(
                "Capacity must be power of 2".into(),
            ));
        }
        log::debug!("Creating fusion ring with capacity {}", capacity);
        // TODO: Implement via kernel syscall
        Ok(0)
    }

    /// Connect to existing Fusion Ring
    ///
    /// # Arguments
    /// * `ring_id` - Fusion Ring identifier
    pub fn fusion_ring_connect(ring_id: u64) -> Result<(), PosixXError> {
        log::trace!("Connecting to fusion ring {}", ring_id);
        // TODO: Implement via kernel syscall
        Ok(())
    }

    /// Disconnect from Fusion Ring
    ///
    /// # Arguments
    /// * `ring_id` - Fusion Ring identifier
    pub fn fusion_ring_disconnect(ring_id: u64) -> Result<(), PosixXError> {
        log::trace!("Disconnecting from fusion ring {}", ring_id);
        // TODO: Implement via kernel syscall
        Ok(())
    }

    /// Get current process ID
    ///
    /// # Performance
    /// Target: 48 cycles (vDSO-like fast path)
    #[inline]
    pub fn getpid() -> i32 {
        // TODO: Read from thread-local storage (vDSO-style)
        1
    }

    /// Get current user ID
    ///
    /// # Performance
    /// Target: 48 cycles (vDSO-like fast path)
    #[inline]
    pub fn getuid() -> u32 {
        // TODO: Read from thread-local storage (vDSO-style)
        1000
    }

    /// Get current time
    ///
    /// # Performance
    /// Target: 100 cycles (vDSO-like fast path)
    #[inline]
    pub fn clock_gettime(clock_id: i32) -> Result<(i64, i64), PosixXError> {
        let _ = clock_id;
        // TODO: Read from vDSO page
        Ok((0, 0))
    }
}
