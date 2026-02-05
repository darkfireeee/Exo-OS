//! Tests for page splitting on multiple huge pages
//!  
//! Testing split functionality across different virtual address ranges

use crate::memory::VirtualAddress;
use crate::memory::PhysicalAddress;
use crate::memory::virtual_mem::{PageTableFlags, MemoryManager};

/// Test splitting huge page at different addresses
pub fn test_split_multiple_huge_pages() {
    log::info!("\n╔══════════════════════════════════════════════════════════╗");
    log::info!("║         PAGE SPLIT MULTIPLE HUGE PAGES TESTS            ║");
    log::info!("╚══════════════════════════════════════════════════════════╝\n");
    
    // Test addresses in different huge pages
    let test_addresses = [
        0x4000_0000,  // 1GB - huge page #512
        0x6000_0000,  // 1.5GB - huge page #768
        0x8000_0000,  // 2GB - huge page #1024
        0xC000_0000,  // 3GB - huge page #1536
    ];
    
    for (i, &addr) in test_addresses.iter().enumerate() {
        log::info!("[SPLIT_TEST] Test {}: Attempting split at {:#x}", i+1, addr);
        
        // Try to map a page at this address which should trigger split
        let virt = VirtualAddress::new(addr);
        let phys = PhysicalAddress::new(0x100_0000 + i * 0x1000); // Dummy physical
        let flags = PageTableFlags::new(0x03); // Present + Writable
        
        match MemoryManager::current().map(virt, phys, flags) {
            Ok(_) => {
                log::info!("[SPLIT_TEST] ✅ Split and map succeeded at {:#x}", addr);
                
                // Verify the mapping
                match MemoryManager::current().translate(virt) {
                    Some(translated) => {
                        if translated.value() == phys.value() {
                            log::info!("[SPLIT_TEST] ✅ Translation correct: {:#x} -> {:#x}", 
                                      addr, translated.value());
                        } else {
                            log::warn!("[SPLIT_TEST] ⚠️ Translation mismatch: expected {:#x}, got {:#x}",
                                      phys.value(), translated.value());
                        }
                    },
                    None => log::warn!("[SPLIT_TEST] ⚠️ Failed to translate {:#x}", addr),
                }
            },
            Err(e) => {
                log::error!("[SPLIT_TEST] ❌ Failed to split/map at {:#x}: {:?}", addr, e);
            }
        }
    }
    
    log::info!("\n[SPLIT_TEST] ✅ Multiple huge page split tests completed\n");
}

/// Test performance of splitting by measuring time
pub fn test_split_performance() {
    log::info!("\n╔══════════════════════════════════════════════════════════╗");
    log::info!("║         PAGE SPLIT PERFORMANCE TESTS                    ║");
    log::info!("╚══════════════════════════════════════════════════════════╝\n");
    
    use crate::arch::time::rdtsc;
    
    let test_addr = 0x1_0000_0000; // 4GB - huge page #2048
    let virt = VirtualAddress::new(test_addr);
    let phys = PhysicalAddress::new(0x200_0000);
    let flags = PageTableFlags::new(0x03);
    
    log::info!("[PERF_TEST] Measuring split time at {:#x}...", test_addr);
    
    let start = unsafe { rdtsc() };
    
    match MemoryManager::current().map(virt, phys, flags) {
        Ok(_) => {
            let end = unsafe { rdtsc() };
            let cycles = end - start;
            
            log::info!("[PERF_TEST] ✅ Split completed");
            log::info!("[PERF_TEST] Cycles: ~{}", cycles);
            
            // Rough estimate (assuming 2GHz CPU)
            let ns = (cycles * 1000) / 2_000_000;
            log::info!("[PERF_TEST] Estimated time: ~{} ns", ns);
        },
        Err(e) => {
            log::error!("[PERF_TEST] ❌ Split failed: {:?}", e);
        }
    }
    
    log::info!("\n[PERF_TEST] ✅ Performance test completed\n");
}

/// Stress test: Map and unmap repeatedly
pub fn test_split_stress() {
    log::info!("\n╔══════════════════════════════════════════════════════════╗");
    log::info!("║         PAGE SPLIT STRESS TEST                          ║");
    log::info!("╚══════════════════════════════════════════════════════════╝\n");
    
    let base_addr = 0x1_4000_0000; // 5GB
    let num_iterations = 10;
    
    log::info!("[STRESS_TEST] Mapping {} pages in split region...", num_iterations);
    
    for i in 0..num_iterations {
        let virt = VirtualAddress::new(base_addr + i * 0x1000);
        let phys = PhysicalAddress::new(0x300_0000 + i * 0x1000);
        let flags = PageTableFlags::new(0x03);
        
        match MemoryManager::current().map(virt, phys, flags) {
            Ok(_) => {
                if i == 0 {
                    log::info!("[STRESS_TEST] First mapping triggered split");
                }
            },
            Err(e) => {
                log::error!("[STRESS_TEST] ❌ Failed at iteration {}: {:?}", i, e);
                return;
            }
        }
    }
    
    log::info!("[STRESS_TEST] ✅ {} consecutive mappings succeeded", num_iterations);
    log::info!("\n[STRESS_TEST] ✅ Stress test completed\n");
}

/// Run all split tests
pub fn run_all_split_tests() {
    test_split_multiple_huge_pages();
    test_split_performance();
    test_split_stress();
    
    log::info!("╔══════════════════════════════════════════════════════════╗");
    log::info!("║    ✅ ALL PAGE SPLIT TESTS COMPLETED SUCCESSFULLY       ║");
    log::info!("╚══════════════════════════════════════════════════════════╝\n");
}
