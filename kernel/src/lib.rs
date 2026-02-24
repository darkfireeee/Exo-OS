//! # Exo-OS Kernel — Re-exports publics
//!
//! `lib.rs` expose l'API publique du noyau aux crates externes et aux tests.
//! Le point d'entrée réel (`_start` → `kernel_main`) est dans `main.rs`.
//!
//! ## Architecture des couches (docs/refonte)
//! ```
//! Couche 0   : memory/    — aucune dépendance kernel
//! Couche 1   : scheduler/ — dépend de memory/
//! Couche 1.5 : process/   — dépend de memory/ + scheduler/
//! Couche 2a  : ipc/       — dépend de memory/ + scheduler/ + security/
//! Couche 2b  : security/  — dépend de memory/
//! Couche 3   : fs/        — dépend de tout sauf ipc/ direct
//! Transverse : arch/      — peut appeler n'importe quelle couche
//! ```
//!
//! ## Règles absolues
//! - `unsafe` → `// SAFETY:` obligatoire (regle_bonus.md)
//! - scheduler/core/ + ISR → NO-ALLOC (regle_bonus.md)
//! - Lock ordering : IPC < Scheduler < Memory < FS (regle_bonus.md)
//! - signal/ ∈ process/ uniquement (DOC1 RÈGLE SIGNAL-01)
//! - capability/ ∈ security/ uniquement (DOC1 RÈGLE CAP-01)
//! - futex ∈ memory/utils/futex_table.rs (DOC3 RÈGLE SCHED-03)

#![no_std]
#![allow(binary_asm_labels)]
#![allow(unexpected_cfgs)]
#![allow(static_mut_refs)]
#![feature(abi_x86_interrupt)]
#![feature(alloc_error_handler)]
#![allow(dead_code)]
#![allow(unused_variables)]
#![allow(unused_imports)]

// ── Crates externes (no_std) ──────────────────────────────────────────────────

extern crate alloc;

// ── Modules kernel ────────────────────────────────────────────────────────────

/// Couche transverse : code spécifique à l'architecture
pub mod arch;

/// Couche 0 : gestion mémoire physique, virtuelle, heap, DMA
pub mod memory;

/// Couche 1 : ordonnanceur, context switch, politiques d'ordo
pub mod scheduler;

/// Couche 1.5 : processus, threads, signaux, cycle de vie
pub mod process;

/// Couche 2a : IPC zero-copy, channels, shared memory
pub mod ipc;

/// Couche 2b : capabilities, isolation, intégrité
pub mod security;

/// Couche 3 : système de fichiers virtuel + ext4plus
pub mod fs;

/// Interface syscall → dispatch vers les couches supérieures
pub mod syscall;

// ── Re-exports publics ─────────────────────────────────────────────────────────
// Seuls les symboles nécessaires aux crates externes (tests, outils) sont exportés.
// Le binaire kernel_main utilise ces modules directement via `exo_os_kernel::`.

#[cfg(target_arch = "x86_64")]
pub use arch::ArchInfo;
pub use arch::x86_64::{
    // Point d'entrée d'initialisation architecture
    boot::early_init::arch_boot_init,
    // Primitives bas niveau exposées
    halt_cpu,
    memory_barrier,
    // Informations d'architecture
    arch_info,
    // Constantes
    PAGE_SIZE,
    KERNEL_BASE,
};

#[cfg(target_arch = "x86_64")]
pub use arch::x86_64::cpu::{
    features::{CpuFeatures, CPU_FEATURES},
    tsc::read_tsc,
};

/// Séquence d'initialisation des couches (appelée depuis `kernel_main` dans main.rs).
///
/// Suit l'ordre DOC2 + DOC3 :
/// 1. arch          (déjà fait avant cet appel)
/// 2. memory/       — EmergencyPool EN PREMIER (RÈGLE EMERGENCY-01)
/// 3. scheduler/    — après memory
/// 4. process/      — après scheduler
/// 5. ipc/          — après process + security
/// 6. security/     — après memory
/// 7. fs/           — en dernier
///
/// # Safety
/// Doit être appelé une seule fois, depuis le BSP, après `arch_boot_init`.
pub unsafe fn kernel_init() {
    // Phase 2 : memory/ — EmergencyPool + buddy + slab
    // memory::physical::frame::emergency_pool::init();   // PREMIER ABSOLU (RÈGLE EMERGENCY-01)
    // memory::physical::allocator::buddy::init();
    // memory::heap::allocator::global::init_heap();

    // Phase 3 : scheduler/
    // scheduler::core::preempt::init();
    // scheduler::core::runqueue::init_percpu();
    // scheduler::fpu::save_restore::detect_xsave_size();
    // scheduler::fpu::lazy::init();
    // scheduler::timer::tick::init(1000);   // HZ = 1000 (RÈGLE SCHED DOC3)
    // scheduler::timer::hrtimer::init();
    // scheduler::sync::wait_queue::init();  // Vérifie que EmergencyPool est init
    // scheduler::energy::c_states::init();

    // Phase 4 : process/
    // process::state::wakeup::register_with_dma();  // DmaWakeupHandler (DOC4 RÈGLE PROC-02)

    // Phase 5 : security/
    // security::capability::init();

    // Phase 6 : ipc/
    // ipc::endpoint::registry::init();

    // Phase 7 : fs/
    // fs::core::vfs::init();
    // process::lifecycle::exec::register_elf_loader(...);  // (DOC4 RÈGLE PROC-01)
}

#[panic_handler]
fn kernel_panic(_info: &core::panic::PanicInfo) -> ! {
    // SAFETY: CLI est toujours sûr depuis Ring 0.
    unsafe { core::arch::asm!("cli", options(nostack, nomem)); }
    loop { unsafe { core::arch::asm!("hlt", options(nostack, nomem)); } }
}

#[alloc_error_handler]
fn alloc_error_handler(_layout: core::alloc::Layout) -> ! {
    unsafe { core::arch::asm!("cli", options(nostack, nomem)); }
    loop { unsafe { core::arch::asm!("hlt", options(nostack, nomem)); } }
}

// ── Point d'entrée principal du kernel (BSP) ─────────────────────────────────
//
// Note : kernel_main est défini dans main.rs (le binaire).
// La lib expose uniquement kernel_init(), arch_boot_init() et halt_cpu().
