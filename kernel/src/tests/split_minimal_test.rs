//! Test minimal pour valider le page splitting
//!
//! Ce test essaie juste de mapper une adresse dans une huge page
//! pour forcer un split et valider que ça fonctionne.

use crate::memory::{PhysicalAddress, VirtualAddress};
use crate::memory::virtual_mem::page_table::{PageTableFlags, PageTableWalker};
use crate::arch;
use crate::logger;

pub fn test_split_minimal() {
    logger::early_print("[SPLIT_MIN] Starting minimal split test...\n");
    
    // Créer un walker
    logger::early_print("[SPLIT_MIN] Reading CR3...\n");
    let root_address = unsafe { 
        let cr3: usize;
        core::arch::asm!("mov {}, cr3", out(reg) cr3);
        PhysicalAddress::new(cr3 & !0xFFF)
    };
    
    logger::early_print("[SPLIT_MIN] Creating PageTableWalker...\n");
    let mut walker = PageTableWalker::new(root_address);
    logger::early_print("[SPLIT_MIN] Walker created successfully\n");
    
    // Essayer de mapper une adresse dans une huge page connue
    // 0x40000000 est typiquement une huge page dans notre setup
    logger::early_print("[SPLIT_MIN] Creating addresses...\n");
    let virt_addr = VirtualAddress::new(0x40000000);
    let phys_addr = PhysicalAddress::new(0x1000000); // Random physical address for test
    logger::early_print("[SPLIT_MIN] Addresses created\n");
    
    logger::early_print("[SPLIT_MIN] Creating flags...\n");
    let flags = PageTableFlags::new().present().writable().user();
    logger::early_print("[SPLIT_MIN] Flags created\n");
    
    logger::early_print("[SPLIT_MIN] About to call map()...\n");
    
    match walker.map(virt_addr, phys_addr, flags) {
        Ok(()) => {
            logger::early_print("[SPLIT_MIN] ✅ Map succeeded!\n");
        }
        Err(_) => {
            logger::early_print("[SPLIT_MIN] ❌ Map failed\n");
        }
    }
    
    logger::early_print("[SPLIT_MIN] After map() call\n");
    
    logger::early_print("[SPLIT_MIN] ✅ Test complete\n");
}
