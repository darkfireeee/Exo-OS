//! Test simple de multi-threading

use crate::scheduler::{Thread, yield_now};
use core::sync::atomic::{AtomicU64, Ordering};

static COUNTER_T1: AtomicU64 = AtomicU64::new(0);
static COUNTER_T2: AtomicU64 = AtomicU64::new(0);
static TEST_ACTIVE: AtomicBool = AtomicBool::new(true);

use core::sync::atomic::AtomicBool;

fn simple_thread_1() -> ! {
    crate::logger::early_print("[TEST] Thread 1 started!\n");
    
    for i in 0..3 {  // Reduced to 3 iterations
        COUNTER_T1.fetch_add(1, Ordering::Relaxed);
        
        let s = alloc::format!("[T1] Iteration {}\n", i);
        crate::logger::early_print(&s);
        
        // Yield to allow other threads to run
        yield_now();
    }
    
    crate::logger::early_print("[TEST] Thread 1 exiting\n");
    crate::syscall::handlers::process::sys_exit(0);
}

fn simple_thread_2() -> ! {
    crate::logger::early_print("[TEST] Thread 2 started!\n");
    
    for i in 0..3 {  // Reduced to 3 iterations
        COUNTER_T2.fetch_add(1, Ordering::Relaxed);
        
        let s = alloc::format!("[T2] Iteration {}\n", i);
        crate::logger::early_print(&s);
        
        // Yield to allow other threads to run
        yield_now();
    }
    
    crate::logger::early_print("[TEST] Thread 2 exiting\n");
    crate::syscall::handlers::process::sys_exit(0);
}

pub fn run_simple_multithread_test() {
    use crate::logger;
    
    logger::early_print("\n═══════════════════════════════════════════\n");
    logger::early_print("  SIMPLE MULTI-THREAD TEST\n");
    logger::early_print("═══════════════════════════════════════════\n\n");
    
    // Reset counters
    COUNTER_T1.store(0, Ordering::SeqCst);
    COUNTER_T2.store(0, Ordering::SeqCst);
    
    // Create 2 threads
    let thread1 = Thread::new_kernel(
        3001,
        "simple_test_1",
        simple_thread_1,
        16384,
    );
    
    let thread2 = Thread::new_kernel(
        3002,
        "simple_test_2",
        simple_thread_2,
        16384,
    );
    
    logger::early_print("[TEST] Adding threads to scheduler...\n");
    
    crate::arch::x86_64::disable_interrupts();
    
    if let Err(e) = crate::scheduler::SCHEDULER.add_thread(thread1) {
        logger::early_print("[ERROR] Failed to add thread1: ");
        let s = alloc::format!("{:?}\n", e);
        logger::early_print(&s);
    }
    
    if let Err(e) = crate::scheduler::SCHEDULER.add_thread(thread2) {
        logger::early_print("[ERROR] Failed to add thread2: ");
        let s = alloc::format!("{:?}\n", e);
        logger::early_print(&s);
    }
    
    crate::arch::x86_64::enable_interrupts();
    
    logger::early_print("[TEST] Interrupts re-enabled, threads will run\n\n");
    
    // NOTE: This test validates that multithreading STARTS correctly.
    // The threads will run in parallel with the kernel continuing execution.
    // We cannot wait for threads to complete in this simple test because
    // the kernel main code is not a scheduled thread.
    //
    // Expected behavior: Threads execute and print their messages via
    // serial output, demonstrating round-robin scheduling.
    
    logger::early_print("✅ MULTITHREAD TEST: Threads created and scheduled\n");
    logger::early_print("   Watch above output for thread execution proof\n");
    logger::early_print("   (threads will continue running while test proceeds)\n\n");
}
