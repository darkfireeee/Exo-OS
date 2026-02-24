// kernel/src/process/mod.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// MODULE PROCESS — Couche 1.5 : Gestion des processus et threads
// ═══════════════════════════════════════════════════════════════════════════════
//
// POSITION ARCHITECTURALE (docs/refonte/DOC4) :
//   Couche 1.5 — dépend de memory/ (Couche 0) + scheduler/ (Couche 1)
//   Est appelé par : ipc/, fs/ (via traits), arch/syscall
//   INTERDIT : use crate::fs, use crate::ipc (sauf via trait abstrait)
//   signal/ : ICI uniquement (RÈGLE SIGNAL-01 DOC1)
//
// SOUS-MODULES :
//   core/       — pid.rs, pcb.rs, tcb.rs (extensions process), registry.rs
//   lifecycle/  — create, fork, exec, exit, wait, reap
//   thread/     — création, join, detach, TLS, pthread_compat
//   signal/     — delivery, handler, mask, queue, default  ← déplacé de scheduler/
//   state/      — machine d'états, wakeup (DmaWakeupHandler impl)
//   group/      — session, pgrp, job_control
//   namespace/  — pid_ns, mount_ns, net_ns, uts_ns, user_ns
//   resource/   — rlimit, usage, cgroup
//
// SÉQUENCE D'INIT (step 20 global) :
//   1. core::pid::init()
//   2. core::registry::init()
//   3. lifecycle::reap::init_reaper()
//   4. state::wakeup::register_with_dma()
//   5. resource::cgroup::init()
//
// RÈGLES ABSOLUES :
//   • PROC-01 : exec() via trait ElfLoader abstrait
//   • PROC-02 : DmaWakeupHandler impl dans state/wakeup.rs
//   • PROC-03 : signal/ géré ici entièrement
//   • PROC-04 : signal_pending ÉCRIT par process/signal/, LU par scheduler
//   • PROC-05 : TCB ProcessThread ≤ 256 bytes additionnel
//   • PROC-06 : Livraison signal : au retour userspace UNIQUEMENT
//   • PROC-07 : zombie reaper = kthread dédié
//   • PROC-08 : fork() flush TLB parent AVANT retour
//   • PROC-09 : namespace/ = isolation complète PID/net/mount
//   • PROC-10 : dma_completion_result dans TCB scheduler (AtomicU8)
//   • unsafe → // SAFETY: obligatoire (regle_bonus.md)
// ═══════════════════════════════════════════════════════════════════════════════

pub mod core;
pub mod lifecycle;
pub mod thread;
pub mod signal;
pub mod state;
pub mod group;
pub mod namespace;
pub mod resource;

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports principaux
// ─────────────────────────────────────────────────────────────────────────────

pub use self::core::pid::{Pid, Tid, PidAllocator, PID_ALLOCATOR, TID_ALLOCATOR};
pub use self::core::pcb::{ProcessControlBlock, ProcessState, ProcessFlags};
pub use self::core::tcb::{ProcessThread, ThreadAddress};
pub use self::core::registry::{PROCESS_REGISTRY, ProcessRegistry};
pub use self::lifecycle::exec::{ElfLoader, register_elf_loader, ExecError};
pub use self::signal::delivery::handle_pending_signals;
pub use self::state::wakeup::{register_with_dma, PROCESS_WAKEUP_HANDLER};

// ─────────────────────────────────────────────────────────────────────────────
// Paramètres d'initialisation
// ─────────────────────────────────────────────────────────────────────────────

/// Paramètres pour `process::init()`.
pub struct ProcessInitParams {
    /// Nombre max de processus simultanés.
    pub max_pids:      usize,
    /// Nombre max de threads simultanés.
    pub max_tids:      usize,
    /// Stack kernel par thread (bytes, multiple de PAGE_SIZE).
    pub kstack_size:   usize,
}

impl Default for ProcessInitParams {
    fn default() -> Self {
        Self {
            max_pids:    32768,
            max_tids:    131072,
            kstack_size: 16384, // 4 × 4K pages
        }
    }
}

/// Initialise le sous-système process.
///
/// Appelé depuis `kernel_main` APRÈS `scheduler::init()`.
///
/// # Safety
/// Appelé une seule fois depuis le BSP avant activation des APs.
pub unsafe fn init(params: &ProcessInitParams) {
    self::core::pid::init(params.max_pids, params.max_tids);
    self::core::registry::init(params.max_pids);
    self::lifecycle::reap::init_reaper();
    self::state::wakeup::register_with_dma();
    self::resource::cgroup::init();
}
