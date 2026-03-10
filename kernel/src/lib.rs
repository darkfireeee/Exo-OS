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
#![feature(allocator_api)]
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
/// 5. security/     — après memory (RÈGLE SEC-BOOT-GAP : avant IPC)
/// 6. ipc/          — après process + security
/// 7. fs/           — en dernier
///
/// # Safety
/// Doit être appelé une seule fois, depuis le BSP, après `arch_boot_init`.
pub unsafe fn kernel_init() {
    #[inline(always)] unsafe fn kdb(b: u8) {
        core::arch::asm!("out 0xE9, al", in("al") b, options(nomem, nostack));
    }
    // ── Phase 2a : EmergencyPool — PREMIER ABSOLU (RÈGLE EMERGENCY-01) ─────────
    crate::memory::physical::frame::emergency_pool::init();
    kdb(b'2'); // Phase 2a done

    // ── Phase 2b : Allocateur heap (SLUB + large) ────────────────────────────
    crate::memory::heap::allocator::hybrid::init();
    kdb(b'3'); // Phase 2b done

    // ── Phase 2c : Time subsystem (HPET + calibration TSC + ktime seqlock) ────────
    // Remplace les 3 appels directs par time_init() qui orchestre :
    //   init_hpet_post_memory() → calibrate_tsc() → pll_init → init_ktime() → clock::init()
    // FIX TIME-02/03 : calibration par fenêtre temporelle réelle (loop = ticks HPET).
    // FIX TIME-01    : ktime protegé par seqlock ISR-safe.
    crate::arch::x86_64::time::time_init();
    kdb(b'4'); // Phase 2c done

    // ── Phase 3 : Scheduler ───────────────────────────────────────────
    crate::scheduler::init(&crate::scheduler::SchedInitParams::default());
    kdb(b'5'); // Phase 3 done

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
    kdb(b'6'); // idle thread done

    // ── Phase 4 : Process ────────────────────────────────────────────────────
    // CORRECTIF : le crash GPF "f000ff53f000ff53" observé précédemment était causé
    // par le bug LAPIC LVT LINT0 (vecteur 0x8E non masqué, livré par le BIOS QEMU).
    // Corrigé par init_local_apic() qui masque tous les LVT entries (commit 40da75e).
    //
    // De plus, create_kthread() utilise désormais le bon frame stack pour
    // context_switch_asm (MXCSR+FCW + 6 regs + kthread_trampoline) au lieu
    // des 2 u64 incorrects qui corrompaient le stack au premier context switch.
    //
    // process::init() orchestre :
    //   1. pid::init()              — réserve PID 0 (idle) et PID 1 (init)
    //   2. registry::init()         — alloue la table PCB (32768 slots)
    //   3. lifecycle::reap::init_reaper() — enfile le kthread reaper
    //   4. state::wakeup::register_with_dma() — enregistre le handler DMA
    //   5. resource::cgroup::init() — initialise le cgroup racine
    //
    // NOTE: process::init() appelle cgroup::init() qui référence CGROUP_TABLE (~28KB .data).
    // Cette section est initialisée correctement mais augmente la taille du binaire.
    // Seuls les sous-systèmes critiques pour le démarrage sont activés ici :
    // GUARD: désactiver les interruptions pendant l'init du sous-système process
    // pour éviter qu'un timer interrupt ne déclenche un context switch avec des
    // structures de données partiellement initialisées.
    core::arch::asm!("cli", options(nomem, nostack));
    kdb(b'a'); // avant pid::init
    crate::process::core::pid::init(32768, 131072);
    kdb(b'b'); // avant registry::init
    // crate::process::core::registry::init(32768); // TEMPORAIREMENT DÉSACTIVÉ — debug
    kdb(b'c'); // avant init_reaper
    // Debug: tester l'allocation heap
    {
        use alloc::alloc::{alloc, dealloc, Layout};
        // Test 8 octets (SLUB classe 0)
        let layout8 = unsafe { Layout::from_size_align_unchecked(8, 8) };
        let p8 = unsafe { alloc(layout8) };
        if p8.is_null() { kdb(b'N'); } else { kdb(b'S'); unsafe { dealloc(p8, layout8); } }
        // Test 128 octets (SLUB classe 4)
        let layout128 = unsafe { Layout::from_size_align_unchecked(128, 64) };
        let p128 = unsafe { alloc(layout128) };
        if p128.is_null() { kdb(b'n'); } else { kdb(b's'); unsafe { dealloc(p128, layout128); } }
        // Indicateur: si les stats SLUB comptent des allocs
        let small = crate::memory::heap::allocator::hybrid::HEAP_STATS
            .small_allocs.load(core::sync::atomic::Ordering::Relaxed);
        if small > 0 { kdb(b'+'); } else { kdb(b'0'); }
    }
    crate::process::lifecycle::reap::init_reaper();
    kdb(b'd'); // avant register_with_dma
    crate::process::state::wakeup::register_with_dma();
    core::arch::asm!("sti", options(nomem, nostack));
    kdb(b'P'); // Phase 4 done (process init + reaper kthread)

    // ── Phase 5 : Security ──────────────────────────────────────────────────
    crate::security::capability::init_capability_subsystem();
    kdb(b'7'); // security done

    // Phase 5b : Crypto — RDRAND-based CSPRNG (requis avant futex seed + IPC auth).
    // TODO: crypto_init() — vérifier compatibilité RDRAND au boot
     crate::security::crypto::crypto_init();

    // ERR-05 fix: Init graine SipHash de la table futex (anti-DoS hash collision).
    {
        let mut seed = [0u8; 16];
        if crate::security::crypto::rng_fill(&mut seed).is_ok() {
            crate::memory::utils::futex_table::init_futex_seed(seed);
        }
    }
    kdb(b'8'); // futex seed done

    // ── Phase 6 : IPC ────────────────────────────────────────────────────────
    ipc::ring::spsc::init_spsc_rings();
    kdb(b'9'); // IPC done

    // ── Phase 7 : FS ─────────────────────────────────────────────────────────
    let _ = crate::fs::exofs::exofs_init(0u64);
    kdb(b'!'); // FS done
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
