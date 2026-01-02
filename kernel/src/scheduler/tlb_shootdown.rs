//! TLB Shootdown - Multi-CPU TLB Synchronization
//!
//! Phase 2d: When one CPU modifies page tables, other CPUs' TLBs must be flushed
//!
//! TLB Shootdown protocol:
//! 1. Initiating CPU sends IPI_TLB_FLUSH to target CPUs
//! 2. Target CPUs flush their TLBs and acknowledge
//! 3. Initiating CPU waits for all ACKs before proceeding

use crate::arch::x86_64::interrupts::ipi::{send_ipi, IPI_TLB_FLUSH_VECTOR};
use crate::scheduler::smp_init::current_cpu_id;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

/// Maximum CPUs supported for TLB shootdown
const MAX_CPUS: usize = 8;

/// TLB flush request
#[derive(Debug, Clone, Copy)]
pub struct TlbFlushRequest {
    /// Virtual address to flush (0 = flush all)
    pub addr: u64,
    
    /// Process CR3 (page table root)
    pub cr3: u64,
    
    /// Global flush (across all processes)
    pub global: bool,
    
    /// Request ID (for tracking)
    pub request_id: u64,
}

/// Per-CPU TLB flush state
pub struct CpuTlbState {
    /// CPU ID
    cpu_id: usize,
    
    /// Pending TLB flush request
    pending: Mutex<Option<TlbFlushRequest>>,
    
    /// Number of TLB flushes performed
    flush_count: AtomicUsize,
    
    /// Acknowledgement flag (set when flush completed)
    ack: AtomicBool,
}

impl CpuTlbState {
    pub const fn new(cpu_id: usize) -> Self {
        Self {
            cpu_id,
            pending: Mutex::new(None),
            flush_count: AtomicUsize::new(0),
            ack: AtomicBool::new(false),
        }
    }
    
    /// Set pending TLB flush request
    pub fn set_pending(&self, request: TlbFlushRequest) {
        *self.pending.lock() = Some(request);
        self.ack.store(false, Ordering::Release);
    }
    
    /// Process pending TLB flush (called from IPI handler)
    pub fn process_flush(&self) {
        let request = {
            let mut pending = self.pending.lock();
            pending.take()
        };
        
        if let Some(req) = request {
            crate::logger::debug(&alloc::format!(
                "[TLB] CPU {} flushing TLB (addr: {:#x}, cr3: {:#x})",
                self.cpu_id,
                req.addr,
                req.cr3
            ));
            
            unsafe {
                if req.global {
                    // Flush all TLB entries
                    flush_tlb_all();
                } else if req.addr == 0 {
                    // Flush TLB for specific CR3 (all addresses)
                    flush_tlb_cr3(req.cr3);
                } else {
                    // Flush specific address
                    flush_tlb_addr(req.addr);
                }
            }
            
            self.flush_count.fetch_add(1, Ordering::Relaxed);
            self.ack.store(true, Ordering::Release);
            
            crate::logger::debug(&alloc::format!(
                "[TLB] CPU {} flush complete (total: {})",
                self.cpu_id,
                self.flush_count.load(Ordering::Relaxed)
            ));
        }
    }
    
    /// Check if flush acknowledged
    pub fn is_acked(&self) -> bool {
        self.ack.load(Ordering::Acquire)
    }
    
    /// Clear acknowledgement
    pub fn clear_ack(&self) {
        self.ack.store(false, Ordering::Release);
    }
    
    /// Get flush count
    pub fn flush_count(&self) -> usize {
        self.flush_count.load(Ordering::Relaxed)
    }
}

/// Global TLB state for all CPUs
pub struct TlbShootdown {
    /// Per-CPU TLB state
    cpus: [CpuTlbState; MAX_CPUS],
    
    /// Next request ID
    next_request_id: AtomicU64,
    
    /// Total shootdowns performed
    total_shootdowns: AtomicUsize,
}

impl TlbShootdown {
    pub const fn new() -> Self {
        Self {
            cpus: [
                CpuTlbState::new(0),
                CpuTlbState::new(1),
                CpuTlbState::new(2),
                CpuTlbState::new(3),
                CpuTlbState::new(4),
                CpuTlbState::new(5),
                CpuTlbState::new(6),
                CpuTlbState::new(7),
            ],
            next_request_id: AtomicU64::new(1),
            total_shootdowns: AtomicUsize::new(0),
        }
    }
    
    /// Flush TLB on specific CPUs
    ///
    /// # Arguments
    /// * `cpus` - List of CPU IDs to flush
    /// * `addr` - Virtual address (0 = flush all)
    /// * `cr3` - Page table root (0 = current)
    /// * `global` - Global flush
    pub fn flush_cpus(&self, cpus: &[usize], addr: u64, cr3: u64, global: bool) {
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        
        let request = TlbFlushRequest {
            addr,
            cr3,
            global,
            request_id,
        };
        
        crate::logger::debug(&alloc::format!(
            "[TLB] Shootdown #{} initiated for {} CPUs",
            request_id,
            cpus.len()
        ));
        
        // Send flush requests to all target CPUs
        for &cpu_id in cpus {
            if cpu_id >= MAX_CPUS {
                continue;
            }
            
            // Set pending request
            self.cpus[cpu_id].set_pending(request);
            
            // Send IPI
            unsafe {
                let apic_id = get_apic_id_for_cpu(cpu_id);
                send_ipi(apic_id, IPI_TLB_FLUSH_VECTOR);
            }
        }
        
        // Wait for all ACKs (with timeout)
        const MAX_WAIT_CYCLES: usize = 10_000_000; // ~10ms at 1GHz
        let mut wait_cycles = 0;
        
        loop {
            let mut all_acked = true;
            
            for &cpu_id in cpus {
                if cpu_id >= MAX_CPUS {
                    continue;
                }
                
                if !self.cpus[cpu_id].is_acked() {
                    all_acked = false;
                    break;
                }
            }
            
            if all_acked {
                break;
            }
            
            wait_cycles += 1;
            if wait_cycles >= MAX_WAIT_CYCLES {
                crate::logger::warn(&alloc::format!(
                    "[TLB] Shootdown #{} timeout waiting for ACKs",
                    request_id
                ));
                break;
            }
            
            // Busy wait with pause
            core::hint::spin_loop();
        }
        
        // Clear ACKs
        for &cpu_id in cpus {
            if cpu_id < MAX_CPUS {
                self.cpus[cpu_id].clear_ack();
            }
        }
        
        self.total_shootdowns.fetch_add(1, Ordering::Relaxed);
        
        crate::logger::debug(&alloc::format!(
            "[TLB] Shootdown #{} complete (waited {} cycles)",
            request_id,
            wait_cycles
        ));
    }
    
    /// Flush TLB on all CPUs except current
    pub fn flush_all_but_self(&self, addr: u64, cr3: u64, global: bool) {
        let current_cpu = current_cpu_id();
        let num_cpus = crate::arch::x86_64::smp::get_cpu_count();
        
        let mut target_cpus = alloc::vec::Vec::new();
        for cpu in 0..num_cpus {
            if cpu != current_cpu {
                target_cpus.push(cpu);
            }
        }
        
        if !target_cpus.is_empty() {
            self.flush_cpus(&target_cpus, addr, cr3, global);
        }
    }
    
    /// Process TLB flush for current CPU (called from IPI handler)
    pub fn process_current_cpu(&self) {
        let cpu_id = current_cpu_id();
        if cpu_id < MAX_CPUS {
            self.cpus[cpu_id].process_flush();
        }
    }
    
    /// Get statistics
    pub fn stats(&self) -> TlbStats {
        let num_cpus = crate::arch::x86_64::smp::get_cpu_count();
        let mut total_flushes = 0;
        
        for cpu in 0..num_cpus.min(MAX_CPUS) {
            total_flushes += self.cpus[cpu].flush_count();
        }
        
        TlbStats {
            total_shootdowns: self.total_shootdowns.load(Ordering::Relaxed),
            total_flushes,
        }
    }
}

/// TLB statistics
#[derive(Debug, Clone, Copy)]
pub struct TlbStats {
    pub total_shootdowns: usize,
    pub total_flushes: usize,
}

/// Global TLB shootdown coordinator
pub static TLB_SHOOTDOWN: TlbShootdown = TlbShootdown::new();

/// Flush TLB entry for a specific address on all CPUs
pub fn tlb_flush_addr_all_cpus(addr: u64) {
    // Flush on current CPU first
    unsafe {
        flush_tlb_addr(addr);
    }
    
    // Shootdown on other CPUs
    TLB_SHOOTDOWN.flush_all_but_self(addr, 0, false);
}

/// Flush all TLB entries on all CPUs
pub fn tlb_flush_all_cpus() {
    // Flush on current CPU first
    unsafe {
        flush_tlb_all();
    }
    
    // Shootdown on other CPUs
    TLB_SHOOTDOWN.flush_all_but_self(0, 0, true);
}

/// Flush TLB for a specific CR3 on all CPUs
pub fn tlb_flush_cr3_all_cpus(cr3: u64) {
    // Flush on current CPU first
    unsafe {
        flush_tlb_cr3(cr3);
    }
    
    // Shootdown on other CPUs
    TLB_SHOOTDOWN.flush_all_but_self(0, cr3, false);
}

/// Low-level TLB flush functions

/// Flush TLB entry for specific address
#[inline]
unsafe fn flush_tlb_addr(addr: u64) {
    core::arch::asm!(
        "invlpg [{}]",
        in(reg) addr,
        options(nostack, preserves_flags)
    );
}

/// Flush all TLB entries
#[inline]
unsafe fn flush_tlb_all() {
    // Reload CR3 to flush TLB
    let cr3: u64;
    core::arch::asm!(
        "mov {0}, cr3",
        "mov cr3, {0}",
        out(reg) cr3,
        options(nostack, preserves_flags)
    );
}

/// Flush TLB for specific CR3
#[inline]
unsafe fn flush_tlb_cr3(cr3: u64) {
    if cr3 == 0 {
        flush_tlb_all();
    } else {
        // If CR3 matches current, reload it
        let current_cr3: u64;
        core::arch::asm!(
            "mov {}, cr3",
            out(reg) current_cr3,
            options(nostack, preserves_flags, nomem)
        );
        
        if current_cr3 == cr3 {
            core::arch::asm!(
                "mov cr3, {}",
                in(reg) cr3,
                options(nostack, preserves_flags)
            );
        }
    }
}

/// Get APIC ID for CPU
fn get_apic_id_for_cpu(cpu_id: usize) -> u32 {
    use crate::arch::x86_64::smp::SMP_SYSTEM;
    
    if let Some(cpu_info) = SMP_SYSTEM.cpu(cpu_id) {
        cpu_info.apic_id.load(Ordering::Acquire) as u32
    } else {
        cpu_id as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_tlb_state() {
        let state = CpuTlbState::new(0);
        
        assert_eq!(state.flush_count(), 0);
        assert!(!state.is_acked());
        
        let request = TlbFlushRequest {
            addr: 0x1000,
            cr3: 0,
            global: false,
            request_id: 1,
        };
        
        state.set_pending(request);
        assert!(!state.is_acked());
    }
}
