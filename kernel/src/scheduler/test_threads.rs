//! Test threads for scheduler demonstration
//! 
//! Three simple threads that run in loops. Timer interrupts do preemption.

/// Write directly to serial port (avoids allocator deadlocks)
#[inline(never)]
fn serial_out(s: &str) {
    for b in s.bytes() {
        unsafe { core::arch::asm!("out dx, al", in("dx") 0x3F8u16, in("al") b, options(nomem, nostack)); }
    }
}

/// Ensure interrupts are enabled (critical for preemption after context switch)
#[inline(always)]
fn enable_interrupts() {
    unsafe { core::arch::asm!("sti", options(nomem, nostack, preserves_flags)); }
}

/// Test thread A - runs in infinite loop, timer will preempt it
pub fn thread_a() -> ! {
    enable_interrupts();  // Re-enable interrupts after context switch
    serial_out("[A] Started\n");
    
    let mut counter = 0u64;
    loop {
        counter = counter.wrapping_add(1);
        
        // Print [A] every 500000 iterations
        if counter % 500000 == 0 {
            serial_out("[A]");
        }
    }
}

/// Test thread B - runs in infinite loop, timer will preempt it
pub fn thread_b() -> ! {
    enable_interrupts();  // Re-enable interrupts after context switch
    serial_out("[B] Started\n");
    
    let mut counter = 0u64;
    loop {
        counter = counter.wrapping_add(1);
        
        // Print [B] every 500000 iterations
        if counter % 500000 == 0 {
            serial_out("[B]");
        }
    }
}

/// Test thread C - runs in infinite loop, timer will preempt it
pub fn thread_c() -> ! {
    enable_interrupts();  // Re-enable interrupts after context switch
    serial_out("[C] Started\n");
    
    let mut counter = 0u64;
    loop {
        counter = counter.wrapping_add(1);
        
        // Print [C] every 500000 iterations
        if counter % 500000 == 0 {
            serial_out("[C]");
        }
    }
}
