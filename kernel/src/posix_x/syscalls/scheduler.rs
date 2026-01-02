//! Scheduler-related POSIX Syscalls
//!
//! Phase 2d: CPU affinity, priority, and scheduling policy syscalls

use crate::scheduler::SCHEDULER;
use crate::scheduler::thread::ThreadId;
use crate::error::{Error, Result};
use alloc::vec::Vec;

// Temporary helper until scheduler exposes current_thread_id()
fn current_thread_id() -> usize {
    // For now return stub - will be replaced with real implementation
    // when scheduler module exports this function
    0  // Thread ID 0
}

/// CPU set type - bitset for CPU affinity (128 CPUs max)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CpuSet {
    bits: [u64; 2], // 2 * 64 = 128 CPUs
}

impl CpuSet {
    /// Create empty CPU set
    pub const fn new() -> Self {
        Self { bits: [0; 2] }
    }

    /// Set CPU in mask
    pub fn set(&mut self, cpu: usize) {
        if cpu < 128 {
            let idx = cpu / 64;
            let bit = cpu % 64;
            self.bits[idx] |= 1u64 << bit;
        }
    }

    /// Clear CPU from mask
    pub fn clear(&mut self, cpu: usize) {
        if cpu < 128 {
            let idx = cpu / 64;
            let bit = cpu % 64;
            self.bits[idx] &= !(1u64 << bit);
        }
    }

    /// Check if CPU is set
    pub fn is_set(&self, cpu: usize) -> bool {
        if cpu < 128 {
            let idx = cpu / 64;
            let bit = cpu % 64;
            (self.bits[idx] & (1u64 << bit)) != 0
        } else {
            false
        }
    }

    /// Get first set CPU
    pub fn first(&self) -> Option<usize> {
        for (idx, &word) in self.bits.iter().enumerate() {
            if word != 0 {
                let bit = word.trailing_zeros() as usize;
                return Some(idx * 64 + bit);
            }
        }
        None
    }

    /// Count number of CPUs set
    pub fn count(&self) -> usize {
        self.bits.iter().map(|w| w.count_ones() as usize).sum()
    }
}

/// sched_setaffinity - Set CPU affinity mask for a thread
///
/// # Arguments
/// * `pid` - Thread ID (0 = current thread)
/// * `cpusetsize` - Size of CPU set in bytes
/// * `mask` - Pointer to CPU affinity mask
///
/// # Returns
/// 0 on success, -1 on error (errno set)
pub fn sys_sched_setaffinity(pid: u64, cpusetsize: usize, mask: *const CpuSet) -> Result<usize> {
    // Validate parameters
    if cpusetsize < core::mem::size_of::<CpuSet>() {
        return Err(Error::InvalidArgument);
    }

    if mask.is_null() {
        return Err(Error::InvalidAddress);
    }

    // Read CPU mask from userspace
    let cpu_mask = unsafe { &*mask };

    // Get target thread ID (0 = current)
    let target_tid = if pid == 0 {
        current_thread_id()
    } else {
        pid as usize
    };

    // Validate at least one CPU is set
    if cpu_mask.count() == 0 {
        return Err(Error::InvalidArgument);
    }

    // Get number of online CPUs
    let num_cpus = crate::arch::x86_64::smp::get_cpu_count();

    // Find first valid CPU in mask
    let mut affinity_cpu = None;
    for cpu in 0..num_cpus {
        if cpu_mask.is_set(cpu) {
            affinity_cpu = Some(cpu);
            break;
        }
    }

    let affinity_cpu = affinity_cpu.ok_or(Error::InvalidArgument)?;

    // Set affinity in scheduler (ThreadId is u64, so use target_tid directly)
    SCHEDULER.set_thread_affinity(target_tid as u64, Some(affinity_cpu))
        .map_err(|_| Error::NotFound)?;
    
    Ok(0)
}

/// sched_getaffinity - Get CPU affinity mask for a thread
///
/// # Arguments
/// * `pid` - Thread ID (0 = current thread)
/// * `cpusetsize` - Size of CPU set buffer in bytes
/// * `mask` - Pointer to buffer for CPU affinity mask
///
/// # Returns
/// Size of CPU set on success, -1 on error (errno set)
pub fn sys_sched_getaffinity(pid: u64, cpusetsize: usize, mask: *mut CpuSet) -> Result<usize> {
    // Validate parameters
    if cpusetsize < core::mem::size_of::<CpuSet>() {
        return Err(Error::InvalidArgument);
    }

    if mask.is_null() {
        return Err(Error::InvalidAddress);
    }

    // Get target thread ID (0 = current)
    let target_tid = if pid == 0 {
        current_thread_id()
    } else {
        pid as usize
    };

    // Get affinity from scheduler (ThreadId is u64, so use target_tid directly)
    let affinity = SCHEDULER.get_thread_affinity(target_tid as u64)
        .map_err(|_| Error::NotFound)?;

    // Build CPU mask
    let mut cpu_mask = CpuSet::new();
    
    if let Some(cpu) = affinity {
        // Thread pinned to specific CPU
        cpu_mask.set(cpu);
    } else {
        // Thread can run on any CPU (all CPUs available)
        let num_cpus = crate::arch::x86_64::smp::get_cpu_count().min(128);
        for cpu in 0..num_cpus {
            cpu_mask.set(cpu);
        }
    }

    // Write to userspace
    unsafe {
        *mask = cpu_mask;
    }

    Ok(core::mem::size_of::<CpuSet>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cpu_set_operations() {
        let mut mask = CpuSet::new();
        
        // Initially empty
        assert_eq!(mask.count(), 0);
        assert!(!mask.is_set(0));
        
        // Set CPU 0
        mask.set(0);
        assert!(mask.is_set(0));
        assert_eq!(mask.count(), 1);
        assert_eq!(mask.first(), Some(0));
        
        // Set CPU 127
        mask.set(127);
        assert!(mask.is_set(127));
        assert_eq!(mask.count(), 2);
        
        // Clear CPU 0
        mask.clear(0);
        assert!(!mask.is_set(0));
        assert_eq!(mask.count(), 1);
        assert_eq!(mask.first(), Some(127));
    }

    #[test]
    fn test_cpu_set_multiple() {
        let mut mask = CpuSet::new();
        
        // Set all CPUs in first word
        for cpu in 0..64 {
            mask.set(cpu);
        }
        assert_eq!(mask.count(), 64);
        
        // Set all CPUs in second word
        for cpu in 64..128 {
            mask.set(cpu);
        }
        assert_eq!(mask.count(), 128);
    }
}
