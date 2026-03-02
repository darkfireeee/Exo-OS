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

/// Couche 3 : système de fichiers virtuel + exofs
/// pub mod fs;

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
    // ── Phase 2a : EmergencyPool — PREMIER ABSOLU (RÈGLE EMERGENCY-01) ─────────
    // Pool statique de 64 WaitNodes pré-alloués en .bss, aucune dépendance heap.
    // Doit être init avant scheduler::sync::wait_queue (appelé par scheduler::init).
    crate::memory::physical::frame::emergency_pool::init();

    // ── Phase 2b : Allocateur heap (SLUB + large) ────────────────────────────
    // Requiert le buddy allocator physique initialisé avec les données Multiboot2.
    // Activé par arch_boot_init via memory::physical::allocator::init_phase1..4().
    // Laisser commenté jusqu'à ce que arch_boot_init propage les plages mémoire.
    // memory::heap::allocator::hybrid::init();  // active HYBRID_ENABLED → Box/Vec OK

    // ── Phase 3 : Scheduler ─────────────────────────────────────────────────
    // scheduler::init() orchestre les 11 étapes : preempt, runqueues, FPU, TSC,
    // tick (HZ=1000), hrtimer, deadline_timer, wait_queue, c_states, smp topology.
    // SAFETY: EmergencyPool initialisé avant cet appel (étape 9 wait_queue en dépend).
    crate::scheduler::init(&crate::scheduler::SchedInitParams::default());

    // ── Phase 3b : Thread idle du BSP (CPU 0) ────────────────────────────────
    // Requis pour que pick_next_task() ait un fallback lorsqu'aucun thread n'est prêt.
    // TCB stocké dans la section .bss (static mut) — aucune allocation heap.
    // SAFETY: appelé une seule fois depuis le BSP, après scheduler::init()
    //         qui a déjà exécuté init_percpu() pour le CPU 0.
    {
        use core::mem::MaybeUninit;
        use core::ptr::NonNull;

        static mut IDLE_TCB: MaybeUninit<crate::scheduler::ThreadControlBlock> =
            MaybeUninit::uninit();

        // Crée un TCB idle : pid=0, SchedPolicy::Normal, Priority::IDLE (140).
        // cr3 = 0 → réutilise la table de pages courante (mapping identité kernel).
        // kernel_rsp = sommet du stack BSP (défini dans .boot_stack par global_asm!).
        // SAFETY: IDLE_TCB est dans .bss (durée de vie 'static).
        //         write() est sûr car exécuté exactement une fois (mono-CPU au boot).
        let idle = IDLE_TCB.write(
            crate::scheduler::ThreadControlBlock::new(
                crate::scheduler::ThreadId(0),
                crate::scheduler::ProcessId(0),
                crate::scheduler::SchedPolicy::Normal,
                crate::scheduler::core::task::Priority::IDLE,
                0u64, // cr3 : réutilise le CR3 courant (pas de switch de PGD)
                crate::arch::x86_64::boot::early_init::boot_stack_top(),
            )
        );

        // SAFETY: write() retourne &mut T non nul ; durée de vie = 'static.
        let idle_ptr = NonNull::new_unchecked(idle as *mut _);
        crate::scheduler::run_queue(crate::scheduler::CpuId(0))
            .set_idle_thread(idle_ptr);
    }

    // ── Phase 4 : Process (reaper kthread) ──────────────────────────────────
    // Nécessite l'allocateur heap (Box<ProcessThread> dans create_kthread).
    // Activer après memory::heap::allocator::hybrid::init().
    // process::lifecycle::reap::init_reaper();

    // ── Phase 5 : Security ──────────────────────────────────────────────────
    // init_capability_subsystem() = pur atomique, aucune alloc. Sûr maintenant.
    crate::security::capability::init_capability_subsystem();
    // access_control::checker::init() et integrity_check::runtime_check::init_runtime_integrity()
    // référencent des symboles lieur (_text_start, _text_end, _rodata_*) définis dans linker.ld.
    // Ces symboles ne sont disponibles que dans le binaire final baremetal.
    // Activer uniquement lors du build final avec le linker script complet.
    // crate::security::access_control::checker::init();
    // crate::security::integrity_check::runtime_check::init_runtime_integrity();

    // ── Phase 6 : IPC ────────────────────────────────────────────────────────
    // Nécessite heap (structures de channels/registry). Activer après heap init.
    // ipc::ring::spsc::init_spsc_rings();

    // ── Phase 7 : FS ─────────────────────────────────────────────────────────
    // fs/ non activé dans lib.rs — en attente d'intégration.
    // fs::core::vfs::init();
}

#[panic_handler]
fn kernel_panic(info: &core::panic::PanicInfo) -> ! {
    // BUG-5 FIX: l'ancien handler effectuait un halt silencieux sans aucun diagnostic.
    // Tout unwrap()/expect() stoppait le système sans message → débogage impossible.
    //
    // Port 0xE9 = QEMU ISA debug device : sortie directe sans initialisation requise.
    // Disponible dès le reset, même avant l'init des drivers série.
    #[inline(always)]
    unsafe fn debug_byte(b: u8) {
        core::arch::asm!("out 0xE9, al", in("al") b, options(nomem, nostack));
    }
    #[inline(always)]
    unsafe fn debug_str(s: &[u8]) {
        for &b in s { debug_byte(b); }
    }
    #[inline(always)]
    unsafe fn debug_u32(mut n: u32) {
        let mut buf = [0u8; 10];
        let mut len = 0usize;
        if n == 0 { debug_byte(b'0'); return; }
        while n > 0 { buf[len] = b'0' + (n % 10) as u8; len += 1; n /= 10; }
        for i in (0..len).rev() { debug_byte(buf[i]); }
    }
    // SAFETY: CLI depuis Ring 0 — toujours sûr. debug_byte écrit sur le port 0xE9.
    unsafe {
        core::arch::asm!("cli", options(nostack, nomem));
        debug_str(b"\n\x1B[1;31m*** KERNEL PANIC ***\x1B[0m ");
        if let Some(loc) = info.location() {
            debug_str(loc.file().as_bytes());
            debug_byte(b':');
            debug_u32(loc.line());
            debug_byte(b':');
            debug_u32(loc.column());
        } else {
            debug_str(b"<location inconnue>");
        }
        debug_byte(b'\n');
    }
    loop { unsafe { core::arch::asm!("hlt", options(nostack, nomem)); } }
}

#[alloc_error_handler]
fn alloc_error_handler(layout: core::alloc::Layout) -> ! {
    // BUG-5 FIX: même correction — affiche taille/alignement sur port 0xE9 QEMU.
    #[inline(always)]
    unsafe fn debug_byte(b: u8) {
        core::arch::asm!("out 0xE9, al", in("al") b, options(nomem, nostack));
    }
    #[inline(always)]
    unsafe fn debug_usize(mut n: usize) {
        let mut buf = [0u8; 20];
        let mut len = 0usize;
        if n == 0 { debug_byte(b'0'); return; }
        while n > 0 { buf[len] = b'0' + (n % 10) as u8; len += 1; n /= 10; }
        for i in (0..len).rev() { debug_byte(buf[i]); }
    }
    unsafe {
        core::arch::asm!("cli", options(nostack, nomem));
        for &b in b"\n*** ALLOC ERROR size=" { debug_byte(b); }
        debug_usize(layout.size());
        for &b in b" align=" { debug_byte(b); }
        debug_usize(layout.align());
        debug_byte(b'\n');
    }
    loop { unsafe { core::arch::asm!("hlt", options(nostack, nomem)); } }
}

// ── Point d'entrée principal du kernel (BSP) ─────────────────────────────────
//
// Note : kernel_main est défini dans main.rs (le binaire).
// La lib expose uniquement kernel_init(), arch_boot_init() et halt_cpu().
