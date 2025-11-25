//! Affinity - CPU affinity management
//!
//! Controls which CPUs a thread can run on

use core::sync::atomic::{AtomicU64, Ordering};

/// CPU affinity mask (64 CPUs max)
#[derive(Debug, Clone, Copy)]
pub struct CpuMask(u64);

impl CpuMask {
    /// Create empty mask
    pub const fn empty() -> Self {
        Self(0)
    }
    
    /// Create mask allowing all CPUs
    pub const fn all() -> Self {
        Self(u64::MAX)
    }
    
    /// Create mask for single CPU
    pub const fn single(cpu: usize) -> Self {
        Self(1 << (cpu & 63))
    }
    
    /// Set CPU bit
    pub fn set(&mut self, cpu: usize) {
        self.0 |= 1 << (cpu & 63);
    }
    
    /// Clear CPU bit
    pub fn clear(&mut self, cpu: usize) {
        self.0 &= !(1 << (cpu & 63));
    }
    
    /// Check if CPU is set
    pub fn is_set(&self, cpu: usize) -> bool {
        (self.0 & (1 << (cpu & 63))) != 0
    }
    
    /// Count set CPUs
    pub fn count(&self) -> u32 {
        self.0.count_ones()
    }
    
    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }
    
    /// Get first set CPU
    pub fn first(&self) -> Option<usize> {
        if self.0 == 0 {
            None
        } else {
            Some(self.0.trailing_zeros() as usize)
        }
    }
    
    /// Intersect with another mask
    pub fn intersect(&self, other: &Self) -> Self {
        Self(self.0 & other.0)
    }
    
    /// Union with another mask
    pub fn union(&self, other: &Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// Thread affinity
pub struct ThreadAffinity {
    /// Allowed CPUs
    allowed: CpuMask,
    
    /// Preferred CPU (for cache locality)
    preferred: Option<usize>,
    
    /// Last CPU (for migration tracking)
    last_cpu: AtomicU64,
}

impl ThreadAffinity {
    pub fn new() -> Self {
        Self {
            allowed: CpuMask::all(),
            preferred: None,
            last_cpu: AtomicU64::new(0),
        }
    }
    
    /// Check if can run on CPU
    pub fn can_run_on(&self, cpu: usize) -> bool {
        self.allowed.is_set(cpu)
    }
    
    /// Set allowed CPUs
    pub fn set_mask(&mut self, mask: CpuMask) {
        self.allowed = mask;
    }
    
    /// Get allowed mask
    pub fn mask(&self) -> CpuMask {
        self.allowed
    }
    
    /// Set preferred CPU
    pub fn set_preferred(&mut self, cpu: usize) {
        self.preferred = Some(cpu);
    }
    
    /// Get preferred CPU
    pub fn preferred(&self) -> Option<usize> {
        self.preferred
    }
    
    /// Update last CPU
    pub fn set_last_cpu(&self, cpu: usize) {
        self.last_cpu.store(cpu as u64, Ordering::Release);
    }
    
    /// Get last CPU
    pub fn last_cpu(&self) -> usize {
        self.last_cpu.load(Ordering::Acquire) as usize
    }
}
