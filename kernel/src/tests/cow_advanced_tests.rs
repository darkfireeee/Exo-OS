//! Tests CoW Avancés - Validation en Conditions Réelles
//!
//! Tests pour valider walk_pages(), fork_cow() et sys_fork()
//! avec de vraies pages et mesures de performance.

use crate::memory::{PhysicalAddress, VirtualAddress, PAGE_SIZE};
use crate::memory::user_space::UserPageFlags;
use crate::memory::cow_manager;
use alloc::vec::Vec;

/// Test walk_pages() avec page tables réelles
/// 
/// Ce test vérifie que walk_pages() peut scanner correctement
/// une hiérarchie de page tables et retourner toutes les pages mappées.
pub fn test_walk_pages_current() {
    crate::logger::early_print("\n");
    crate::logger::early_print("╔═══════════════════════════════════════════════════╗\n");
    crate::logger::early_print("║  TEST WALK_PAGES: Scanner Page Tables Actuelles  ║\n");
    crate::logger::early_print("╚═══════════════════════════════════════════════════╝\n");
    
    crate::logger::early_print("[INFO] Walking current page tables...\n");
    
    // Lire CR3 pour obtenir PML4 actuel
    let cr3: u64;
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack));
    }
    
    let pml4_phys = PhysicalAddress::new((cr3 & 0x000F_FFFF_FFFF_F000) as usize);
    let s = alloc::format!("[INFO] Current PML4 at phys: {:#x}\n", pml4_phys.value());
    crate::logger::early_print(&s);
    
    // Note: walk_pages() nécessite UserAddressSpace
    // Pour l'instant, ce test documente l'approche future
    
    crate::logger::early_print("[SKIP] walk_pages() nécessite UserAddressSpace créé\n");
    crate::logger::early_print("[INFO] Approche future: Créer Process userspace via exec()\n");
}

/// Test sys_fork() avec Process minimal
///
/// Crée un Process minimal et essaie de fork
pub fn test_sys_fork_minimal() {
    crate::logger::early_print("\n");
    crate::logger::early_print("╔═══════════════════════════════════════════════════╗\n");
    crate::logger::early_print("║  TEST SYS_FORK: Fork depuis Kernel Thread       ║\n");
    crate::logger::early_print("╚═══════════════════════════════════════════════════╝\n");
    
    crate::logger::early_print("[INFO] Testing sys_fork() from kernel context...\n");
    
    // Appeler sys_fork() depuis kernel thread (devrait créer child vide)
    match crate::syscall::handlers::process::sys_fork() {
        Ok(child_pid) => {
            let s = alloc::format!("[SUCCESS] Fork returned child PID: {}\n", child_pid);
            crate::logger::early_print(&s);
            crate::logger::early_print("[INFO] Child has empty address space (parent is kernel thread)\n");
        }
        Err(e) => {
            let s = alloc::format!("[ERROR] Fork failed: {:?}\n", e);
            crate::logger::early_print(&s);
        }
    }
    
    crate::logger::early_print("[NOTE] Pour tester fork() avec pages réelles:\n");
    crate::logger::early_print("       1. Charger ELF userspace via loader\n");
    crate::logger::early_print("       2. Process aura UserAddressSpace mappé\n");
    crate::logger::early_print("       3. Fork capturera pages via walk_pages()\n");
}

/// Test refcount CoW avec fork simulé
///
/// Simule un fork en marquant les mêmes pages 2 fois
/// pour vérifier l'incrémentation du refcount
pub fn test_cow_refcount() {
    crate::logger::early_print("\n");
    crate::logger::early_print("╔═══════════════════════════════════════════════════╗\n");
    crate::logger::early_print("║  TEST REFCOUNT: Partage Pages Parent/Child      ║\n");
    crate::logger::early_print("╚═══════════════════════════════════════════════════╝\n");
    
    // État initial - avant tout mark_cow
    let stats_init = cow_manager::get_stats();
    let s = alloc::format!("[DEBUG-INIT] Stats AVANT test: total_pages={}, total_refs={}\n",
                          stats_init.total_pages, stats_init.total_refs);
    crate::logger::early_print(&s);
    
    // Créer frame synthétique UNIQUE non utilisé par autres tests
    // Utiliser 0x500000 au lieu de 0x100000 pour éviter collision
    let test_frame = PhysicalAddress::new(0x500000);
    let s = alloc::format!("[DEBUG] Using UNIQUE test frame: {:#x}\n", test_frame.value());
    crate::logger::early_print(&s);
    
    crate::logger::early_print("[TEST] Simulating parent marking page as CoW...\n");
    let refcount1 = cow_manager::mark_cow(test_frame);
    let s = alloc::format!("[PARENT] Refcount after parent: {}\n", refcount1);
    crate::logger::early_print(&s);
    
    let stats_after1 = cow_manager::get_stats();
    let s = alloc::format!("[DEBUG-AFTER1] Stats: total_pages={}, total_refs={}\n",
                          stats_after1.total_pages, stats_after1.total_refs);
    crate::logger::early_print(&s);
    
    crate::logger::early_print("[TEST] Simulating child mapping same page...\n");
    let refcount2 = cow_manager::mark_cow(test_frame);
    let s = alloc::format!("[CHILD] Refcount after child: {}\n", refcount2);
    crate::logger::early_print(&s);
    
    let stats_after2 = cow_manager::get_stats();
    let s = alloc::format!("[DEBUG-AFTER2] Stats: total_pages={}, total_refs={}\n",
                          stats_after2.total_pages, stats_after2.total_refs);
    crate::logger::early_print(&s);
    
    if refcount2 == 2 {
        crate::logger::early_print("[PASS] ✅ Refcount correctly incremented to 2\n");
    } else {
        let s = alloc::format!("[FAIL] ❌ Expected refcount=2, got {}\n", refcount2);
        crate::logger::early_print(&s);
        crate::logger::early_print("[DEBUG] Possible causes:\n");
        crate::logger::early_print("        - Frame was already tracked before test\n");
        crate::logger::early_print("        - Previous tests left residual refcount\n");
        crate::logger::early_print("        - Bug in mark_cow() increment logic\n");
    }
    
    crate::logger::early_print("[INFO] Test complete (refcount will persist)\n");
}

/// Test complet: Mesure latence fork()
///
/// Mesure le temps pris par sys_fork() avec RDTSC
pub fn test_fork_latency() {
    crate::logger::early_print("\n");
    crate::logger::early_print("╔═══════════════════════════════════════════════════╗\n");
    crate::logger::early_print("║  TEST LATENCY: Mesure Performance fork()        ║\n");
    crate::logger::early_print("╚═══════════════════════════════════════════════════╝\n");
    
    // Mesurer cycles avec RDTSC
    let start: u64;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            "shl rdx, 32",
            "or rax, rdx",
            out("rax") start,
            out("rdx") _,
            options(nomem, nostack)
        );
    }
    
    crate::logger::early_print("[TEST] Calling sys_fork()...\n");
    let result = crate::syscall::handlers::process::sys_fork();
    
    let end: u64;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            "shl rdx, 32",
            "or rax, rdx",
            out("rax") end,
            out("rdx") _,
            options(nomem, nostack)
        );
    }
    
    let cycles = end - start;
    
    match result {
        Ok(child_pid) => {
            let s = alloc::format!("[SUCCESS] Fork completed in {} cycles\n", cycles);
            crate::logger::early_print(&s);
            let s = alloc::format!("           Child PID: {}\n", child_pid);
            crate::logger::early_print(&s);
            
            // Objectif: < 100K cycles pour fork sans pages
            if cycles < 100_000 {
                crate::logger::early_print("[PASS] ✅ Latency acceptable (< 100K cycles)\n");
            } else if cycles < 1_000_000 {
                crate::logger::early_print("[WARN] ⚠️  Latency élevée mais acceptable (< 1M cycles)\n");
            } else {
                crate::logger::early_print("[FAIL] ❌ Latency excessive (> 1M cycles)\n");
            }
        }
        Err(e) => {
            let s = alloc::format!("[ERROR] Fork failed: {:?}\n", e);
            crate::logger::early_print(&s);
        }
    }
}

/// Lance tous les tests avancés
pub fn run_all_advanced_tests() {
    crate::logger::early_print("\n");
    crate::logger::early_print("════════════════════════════════════════════════════\n");
    crate::logger::early_print("   TESTS CoW AVANCÉS - Validation Complète\n");
    crate::logger::early_print("════════════════════════════════════════════════════\n");
    
    test_walk_pages_current();
    test_sys_fork_minimal();
    test_cow_refcount();
    test_fork_latency();
    
    crate::logger::early_print("\n");
    crate::logger::early_print("════════════════════════════════════════════════════\n");
    crate::logger::early_print("   TESTS AVANCÉS TERMINÉS\n");
    crate::logger::early_print("════════════════════════════════════════════════════\n");
}
