//! Tests for TLB flush operations
//! 
//! Investigating why TLB flush causes system hang during page splitting

use crate::arch::x86_64::memory::tlb;

/// Test 1: Basic TLB flush with flush_page
pub fn test_tlb_flush_single_page() {
    log::info!("[TLB_TEST] Test 1: Single page flush...");
    
    // Test flushing a known mapped address (kernel code)
    let test_addr = 0xFFFF_8000_0010_0000; // Kernel address
    
    log::info!("[TLB_TEST] About to flush address {:#x}", test_addr);
    tlb::flush_page(test_addr);
    log::info!("[TLB_TEST] ✅ Single page flush completed");
}

/// Test 2: TLB flush_all
pub fn test_tlb_flush_all() {
    log::info!("[TLB_TEST] Test 2: Full TLB flush...");
    
    log::info!("[TLB_TEST] About to call flush_all()");
    tlb::flush_all();
    log::info!("[TLB_TEST] ✅ Full TLB flush completed");
}

/// Test 3: Multiple sequential flushes
pub fn test_tlb_multiple_flushes() {
    log::info!("[TLB_TEST] Test 3: Multiple sequential flushes...");
    
    for i in 0..5 {
        let addr = 0xFFFF_8000_0010_0000 + (i * 0x1000);
        log::info!("[TLB_TEST] Flushing {:#x}...", addr);
        tlb::flush_page(addr);
    }
    
    log::info!("[TLB_TEST] ✅ Multiple flushes completed");
}

/// Test 4: Flush in different contexts
pub fn test_tlb_context_variations() {
    log::info!("[TLB_TEST] Test 4: Context variations...");
    
    // Test with interrupts disabled
    log::info!("[TLB_TEST] Test 4a: With interrupts disabled");
    let flags = unsafe { 
        let flags: u64;
        core::arch::asm!("pushfq; pop {}", out(reg) flags);
        core::arch::asm!("cli");
        flags
    };
    
    tlb::flush_page(0xFFFF_8000_0010_0000);
    log::info!("[TLB_TEST] ✅ Flush with CLI succeeded");
    
    // Restore interrupts
    unsafe {
        if flags & 0x200 != 0 {
            core::arch::asm!("sti");
        }
    }
    
    log::info!("[TLB_TEST] ✅ All context tests passed");
}

/// Run all TLB tests
pub fn run_all_tlb_tests() {
    log::info!("\n╔══════════════════════════════════════════════════════════╗");
    log::info!("║         TLB FLUSH INVESTIGATION TESTS                   ║");
    log::info!("╚══════════════════════════════════════════════════════════╝\n");
    
    test_tlb_flush_single_page();
    test_tlb_multiple_flushes();
    test_tlb_context_variations();
    test_tlb_flush_all(); // Test this last as it may hang
    
    log::info!("\n[TLB_TEST] ✅ All TLB tests completed successfully");
}
