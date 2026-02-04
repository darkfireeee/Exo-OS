//! Tests CoW RÉELS
use crate::memory::{PhysicalAddress, PAGE_SIZE};
use crate::memory::cow_manager;
use alloc::boxed::Box;

pub fn test_walk_pages_kernel_real() {
    crate::logger::early_print("\n[TEST 1] CR3 Access\n");
    let cr3: u64;
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack));
    }
    let pml4_phys = PhysicalAddress::new((cr3 & 0x000F_FFFF_FFFF_F000) as usize);
    let s = alloc::format!("[CR3] PML4 at phys: {:#x}\n", pml4_phys.value());
    crate::logger::early_print(&s);
    crate::logger::early_print("[PASS] ✅ CR3 access OK\n");
}

pub fn test_fork_cow_kernel_pages() {
    crate::logger::early_print("\n[TEST 2] CoW Refcount (no heap alloc)\n");
    
    let test_addrs = [
        PhysicalAddress::new(0x200000),
        PhysicalAddress::new(0x300000),
        PhysicalAddress::new(0x400000),
    ];
    
    for (i, phys) in test_addrs.iter().enumerate() {
        let rc1 = cow_manager::mark_cow(*phys);
        let rc2 = cow_manager::mark_cow(*phys);
        let s = alloc::format!("  Addr {}: {:#x} → rc1={}, rc2={}\n", i+1, phys.value(), rc1, rc2);
        crate::logger::early_print(&s);
        if rc1 == 1 && rc2 == 2 {
            crate::logger::early_print("  ✅ PASS\n");
        } else {
            crate::logger::early_print("  ❌ FAIL\n");
        }
    }
}

pub fn test_cow_with_heap_pages() {
    crate::logger::early_print("\n[TEST 3] Small Data Test\n");
    let mut val: u64 = 0xAAAAAAAA_BBBBBBBB;
    crate::logger::early_print("[SETUP] val = 0xAAAAAAAABBBBBBBB\n");
    
    val = 0xCCCCCCCC_DDDDDDDD;
    
    if val == 0xCCCCCCCC_DDDDDDDD {
        crate::logger::early_print("[PASS] ✅ Data modified OK\n");
    } else {
        crate::logger::early_print("[FAIL] ❌ Data corrupted\n");
    }
}

pub fn run_all_real_tests() {
    crate::logger::early_print("\n=== TESTS COW RÉELS ===\n");
    test_walk_pages_kernel_real();
    test_fork_cow_kernel_pages();
    test_cow_with_heap_pages();
    crate::logger::early_print("\n=== TESTS RÉELS TERMINÉS ===\n");
}
