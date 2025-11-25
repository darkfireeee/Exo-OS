//! Idle Thread Implementation
//! 
//! Provides idle threads that run when no other threads are ready.
//! Idle threads use HLT instruction to save power and reduce CPU usage.
//!
//! # Design
//! - One idle thread per CPU (future: SMP support)
//! - Lowest priority (never picked if real work exists)
//! - Uses HLT to reduce power consumption
//! - Wakes up on interrupts automatically

use super::thread::{Thread, ThreadId};
use core::sync::atomic::{AtomicU64, Ordering};
use alloc::vec::Vec;
use spin::Mutex;

/// Global idle thread registry
static IDLE_THREADS: Mutex<Vec<ThreadId>> = Mutex::new(Vec::new());

/// Current idle thread (per-CPU, for now just one)
static CURRENT_IDLE_TID: AtomicU64 = AtomicU64::new(0);

/// Idle thread entry point
/// 
/// This function never returns and continuously halts the CPU
/// until an interrupt occurs.
///
/// # Public for scheduler
/// This needs to be public so scheduler can use it
pub fn idle_thread_entry() -> ! {
    crate::logger::debug("Idle thread started");
    
    loop {
        // Enable interrupts before HLT
        // This ensures we wake up on timer/keyboard/other interrupts
        unsafe {
            core::arch::asm!(
                "sti",      // Enable interrupts
                "hlt",      // Halt until interrupt
                options(nomem, nostack)
            );
        }
        
        // After interrupt, yield back to scheduler
        // The interrupt handler will have changed scheduler state
    }
}

/// Create idle thread for current CPU
pub fn create_idle_thread() -> Thread {
    Thread::new_kernel(
        0, // TID will be assigned by scheduler
        "idle",
        idle_thread_entry,
        4096, // 4KB stack is enough for idle
    )
}

/// Initialize idle thread subsystem
pub fn init() {
    // Create one idle thread (TODO: one per CPU in SMP)
    crate::logger::info("Initializing idle thread...");
    
    // The idle thread will be created by scheduler when needed
    // We just initialize the global state here
    
    crate::logger::info("✓ Idle thread system initialized");
}

/// Register an idle thread
pub fn register_idle_thread(tid: ThreadId) {
    let mut idle_threads = IDLE_THREADS.lock();
    idle_threads.push(tid);
    
    // Set as current idle if first one
    if idle_threads.len() == 1 {
        CURRENT_IDLE_TID.store(tid, Ordering::Release);
    }
    
    crate::logger::debug(&alloc::format!("Registered idle thread {}", tid));
}

/// Check if a thread ID is an idle thread
pub fn is_idle_thread(tid: ThreadId) -> bool {
    let idle_threads = IDLE_THREADS.lock();
    idle_threads.contains(&tid)
}

/// Check if current thread is idle thread
pub fn is_current_idle() -> bool {
    let current_idle = CURRENT_IDLE_TID.load(Ordering::Acquire);
    if current_idle == 0 {
        return false;
    }
    
    // TODO: Get actual current TID from scheduler
    // For now, assume not idle
    false
}

/// Get idle thread for current CPU
pub fn get_idle_tid() -> Option<ThreadId> {
    let tid = CURRENT_IDLE_TID.load(Ordering::Acquire);
    if tid == 0 {
        None
    } else {
        Some(tid)
    }
}

/// Idle loop (called when no threads are ready)
/// 
/// This is a fallback if somehow we don't have an idle thread.
/// Should rarely/never be called in normal operation.
pub fn idle_loop() -> ! {
    crate::logger::warn("Entered fallback idle loop!");
    
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
/// 
/// Uses HLT instruction to reduce CPU power consumption
/// until next interrupt.
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
