//! Idle Thread Implementation - SMP Support
//! 
//! Provides one idle thread per CPU that runs when no other threads are ready.
//! Idle threads use HLT instruction to save power and reduce CPU usage.
//!
//! # Design
//! - One idle thread per CPU for SMP systems
//! - Lowest priority (never picked if real work exists)
//! - Uses HLT/MWAIT to reduce power consumption
//! - Wakes up on interrupts automatically
//! - Per-CPU idle statistics for power management

use super::thread::{Thread, ThreadId};
use crate::arch::x86_64::cpu::smp;
use core::sync::atomic::{AtomicU64, AtomicU32, Ordering};
use alloc::vec::Vec;
use spin::Mutex;

/// Maximum supported CPUs
const MAX_CPUS: usize = 256;

/// Per-CPU idle thread data
struct PerCpuIdle {
    /// Thread ID of this CPU's idle thread
    tid: AtomicU64,
    /// Number of times we've entered idle
    idle_count: AtomicU64,
    /// Total cycles spent in idle (approximation)
    idle_cycles: AtomicU64,
    /// Is this CPU currently idle?
    is_idle: AtomicU32,
}

impl PerCpuIdle {
    const fn new() -> Self {
        Self {
            tid: AtomicU64::new(0),
            idle_count: AtomicU64::new(0),
            idle_cycles: AtomicU64::new(0),
            is_idle: AtomicU32::new(0),
        }
    }
}

/// Per-CPU idle data
static PER_CPU_IDLE: [PerCpuIdle; MAX_CPUS] = {
    const INIT: PerCpuIdle = PerCpuIdle::new();
    [INIT; MAX_CPUS]
};

/// Global idle thread registry (for backward compatibility)
static IDLE_THREADS: Mutex<Vec<ThreadId>> = Mutex::new(Vec::new());

/// Idle thread entry point
/// 
/// This function never returns and continuously halts the CPU
/// until an interrupt occurs.
pub fn idle_thread_entry() -> ! {
    let cpu_id = smp::get_apic_id() as usize;
    
    // Mark this CPU as having an active idle thread
    if cpu_id < MAX_CPUS {
        log::debug!("Idle thread started on CPU {}", cpu_id);
    }
    
    loop {
        // Mark as entering idle
        if cpu_id < MAX_CPUS {
            PER_CPU_IDLE[cpu_id].is_idle.store(1, Ordering::Release);
            PER_CPU_IDLE[cpu_id].idle_count.fetch_add(1, Ordering::Relaxed);
        }
        
        // Enable interrupts and halt
        // This ensures we wake up on timer/keyboard/other interrupts
        unsafe {
            core::arch::asm!(
                "sti",      // Enable interrupts
                "hlt",      // Halt until interrupt
                options(nomem, nostack)
            );
        }
        
        // Mark as exiting idle
        if cpu_id < MAX_CPUS {
            PER_CPU_IDLE[cpu_id].is_idle.store(0, Ordering::Release);
        }
        
        // After interrupt, loop back
        // The interrupt handler may have made threads runnable
    }
}

/// Create idle thread for a specific CPU
pub fn create_idle_thread_for_cpu(cpu_id: u32) -> Thread {
    Thread::new_kernel(
        0, // TID will be assigned by scheduler
        &alloc::format!("idle-{}", cpu_id),
        idle_thread_entry,
        4096, // 4KB stack is enough for idle
    )
}

/// Create idle thread for current CPU
pub fn create_idle_thread() -> Thread {
    create_idle_thread_for_cpu(smp::get_apic_id())
}

/// Initialize idle thread subsystem
pub fn init() {
    log::info!("Initializing per-CPU idle threads...");
    
    // The actual idle threads will be created by the scheduler
    // when boot_aps() completes and each CPU enters the scheduler
    
    log::info!("✓ Idle thread system initialized");
}

/// Initialize idle threads for all CPUs
pub fn init_all_cpus() {
    let cpu_count = smp::cpu_count();
    log::info!("Creating {} idle threads for SMP", cpu_count);
    
    // BSP idle thread is already created during boot
    // AP idle threads are created when each AP boots and enters scheduler
}

/// Register an idle thread for a specific CPU
pub fn register_idle_thread_for_cpu(cpu_id: u32, tid: ThreadId) {
    if (cpu_id as usize) < MAX_CPUS {
        PER_CPU_IDLE[cpu_id as usize].tid.store(tid, Ordering::Release);
    }
    
    // Also add to global list for compatibility
    let mut idle_threads = IDLE_THREADS.lock();
    if !idle_threads.contains(&tid) {
        idle_threads.push(tid);
    }
    
    log::debug!("Registered idle thread {} for CPU {}", tid, cpu_id);
}

/// Register an idle thread (backward compatible)
pub fn register_idle_thread(tid: ThreadId) {
    let cpu_id = smp::get_apic_id();
    register_idle_thread_for_cpu(cpu_id, tid);
}

/// Check if a thread ID is an idle thread
pub fn is_idle_thread(tid: ThreadId) -> bool {
    let idle_threads = IDLE_THREADS.lock();
    idle_threads.contains(&tid)
}

/// Check if specified CPU is currently idle
pub fn is_cpu_idle(cpu_id: u32) -> bool {
    if (cpu_id as usize) < MAX_CPUS {
        PER_CPU_IDLE[cpu_id as usize].is_idle.load(Ordering::Acquire) != 0
    } else {
        false
    }
}

/// Check if current CPU is idle
pub fn is_current_idle() -> bool {
    is_cpu_idle(smp::get_apic_id())
}

/// Get idle thread for current CPU
pub fn get_idle_tid() -> Option<ThreadId> {
    get_idle_tid_for_cpu(smp::get_apic_id())
}

/// Get idle thread for specified CPU
pub fn get_idle_tid_for_cpu(cpu_id: u32) -> Option<ThreadId> {
    if (cpu_id as usize) < MAX_CPUS {
        let tid = PER_CPU_IDLE[cpu_id as usize].tid.load(Ordering::Acquire);
        if tid != 0 {
            return Some(tid);
        }
    }
    None
}

/// Get idle statistics for a CPU
pub fn get_idle_stats(cpu_id: u32) -> (u64, u64) {
    if (cpu_id as usize) < MAX_CPUS {
        let count = PER_CPU_IDLE[cpu_id as usize].idle_count.load(Ordering::Relaxed);
        let cycles = PER_CPU_IDLE[cpu_id as usize].idle_cycles.load(Ordering::Relaxed);
        (count, cycles)
    } else {
        (0, 0)
    }
}

/// Idle loop (fallback if somehow we don't have an idle thread)
pub fn idle_loop() -> ! {
    log::warn!("Entered fallback idle loop on CPU {}!", smp::get_apic_id());
    
    loop {
        // Power-saving halt with interrupts enabled
        unsafe {
            core::arch::asm!(
                "sti",      // Enable interrupts
                "hlt",      // Halt until interrupt
                options(nomem, nostack)
            );
        }
    }
}

/// Enter low-power idle state
#[inline]
pub fn halt() {
    unsafe {
        core::arch::asm!(
            "sti",      // Ensure interrupts enabled
            "hlt",      // Halt
            options(nomem, nostack)
        );
    }
}

/// Wake up a specific CPU from idle (send IPI)
pub fn wake_cpu(cpu_id: u32) {
    if is_cpu_idle(cpu_id) {
        // Send a scheduler IPI to wake the CPU
        smp::send_ipi(cpu_id, 0x20); // Vector 0x20 = scheduler IPI
    }
}

/// Wake up all idle CPUs
pub fn wake_all_idle() {
    for cpu_id in 0..smp::cpu_count() {
        if is_cpu_idle(cpu_id) {
            smp::send_ipi(cpu_id, 0x20);
        }
    }
}
