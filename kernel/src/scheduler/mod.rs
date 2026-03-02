// kernel/src/scheduler/mod.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Module Scheduler — point d'entrée et séquence d'initialisation
// ═══════════════════════════════════════════════════════════════════════════════
//
// Couche 1 : dépend UNIQUEMENT de memory/ (Couche 0).
// Aucun import de process/, ipc/, fs/, security/.
//
// Séquence d'init (DOC3) :
//   1. preempt::init()
//   2. runqueue::init_percpu()
//   3. fpu::save_restore::init()   (détecte taille XSAVE)
//   4. fpu::lazy::init()           (CR0.TS=1)
//   5. timer::clock::init()        (calibrage TSC)
//   6. timer::tick::init()         (HZ=1000)
//   7. timer::hrtimer::init()
//   8. timer::deadline_timer::init()
//   9. sync::wait_queue::init()    (verif EmergencyPool)
//  10. energy::c_states::init()
//  11. smp::topology_init()
// ═══════════════════════════════════════════════════════════════════════════════

pub mod core;
pub mod energy;
pub mod fpu;
pub mod policies;
pub mod smp;
pub mod stats;
pub mod sync;
pub mod timer;

// Re-exports principaux.
pub use self::core::task::{ThreadControlBlock, ThreadId, ProcessId, CpuId, TaskState, SchedPolicy};
pub use self::core::preempt::{PreemptGuard, IrqGuard};
pub use self::core::runqueue::{run_queue, init_percpu as rq_init_percpu};
pub use self::core::pick_next::pick_next_task;
pub use self::core::switch::{context_switch, schedule_yield, schedule_block, wake_enqueue,
                             block_current_thread, current_thread_raw};
pub use self::timer::clock::monotonic_ns;
pub use self::timer::tick::scheduler_tick;
pub use self::policies::ai_guided::AI_HINTS_ENABLED;

// ─────────────────────────────────────────────────────────────────────────────
// Séquence d'initialisation globale
// ─────────────────────────────────────────────────────────────────────────────

/// Paramètres d'initialisation du scheduler.
pub struct SchedInitParams {
    /// Nombre de CPUs logiques.
    pub nr_cpus: usize,
    /// Nombre de nœuds NUMA.
    pub nr_nodes: usize,
    /// Fréquence TSC en Hz (0 = utiliser la valeur par défaut 3GHz).
    pub tsc_hz: u64,
}

impl Default for SchedInitParams {
    fn default() -> Self {
        Self { nr_cpus: 1, nr_nodes: 1, tsc_hz: 3_000_000_000 }
    }
}

/// Initialise le sous-système scheduler.
///
/// Doit être appelé par le BSP APRÈS que `memory::init()` a été exécuté
/// (l'EmergencyPool doit être disponible avant `sync::wait_queue::init()`).
///
/// # Safety
/// Appelé une seule fois, depuis le BSP, avant l'activation des APs.
pub unsafe fn init(params: &SchedInitParams) {
    // BUG-FIX H : clamp nr_cpus à MAX_CPUS pour éviter des accès out-of-bounds
    // sur PER_CPU_RQ (64 entrées) si l'appelant passe nr_cpus > 64.
    let nr_cpus  = params.nr_cpus.clamp(1, crate::scheduler::core::preempt::MAX_CPUS);
    let nr_nodes = params.nr_nodes.max(1);
    let tsc_hz   = if params.tsc_hz == 0 { 3_000_000_000 } else { params.tsc_hz };

    // Étape 1 — Compteurs de préemption.
    self::core::preempt::init();

    // Étape 2 — Run queues par CPU.
    self::core::runqueue::init_percpu(nr_cpus);

    // Étape 3 — Détection XSAVE.
    self::fpu::save_restore::init();

    // Étape 4 — Lazy FPU (CR0.TS=1 sur le BSP).
    self::fpu::lazy::init();

    // Étape 5 — Horloge TSC.
    self::timer::clock::init(tsc_hz);

    // Étape 6 — Tick handler.
    self::timer::tick::init(nr_cpus);

    // Étape 7 — HRTimers.
    self::timer::hrtimer::init(nr_cpus);

    // Étape 8 — Deadline timers.
    self::timer::deadline_timer::init(nr_cpus);

    // Étape 9 — Wait queues (vérifie que l'EmergencyPool est prêt).
    self::sync::wait_queue::init();

    // Étape 10 — C-states.
    self::energy::c_states::init(nr_cpus);

    // Étape 11 — Topologie SMP.
    self::smp::topology_init(nr_cpus, nr_nodes);
}

/// Initialise le scheduler sur un AP (Application Processor).
///
/// Appelé par chaque AP après son démarrage.
///
/// # Safety
/// Appelé depuis le contexte de boot de chaque AP.
pub unsafe fn init_ap(cpu_id: u32) {
    // CR0.TS=1 sur cet AP (FPU lazy).
    self::fpu::lazy::init();

    // La run queue de ce CPU est déjà initialisée par init_percpu().
    // Rien d'autre à faire.
    let _ = cpu_id;
}
