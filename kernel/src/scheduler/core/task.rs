// kernel/src/scheduler/core/task.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// TASK / TCB — Thread Control Block (Exo-OS Scheduler · Couche 1)
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLES D'ARCHITECTURE (docs/refonte/DOC1 + DOC3 + DOC4) :
//   • signal_pending = AtomicBool ÉCRIT par process/signal/, LU par scheduler (hot path)
//   • signal_mask    = AtomicU64  — bitmask 64 signaux standard (POSIX)
//   • TCB = 128 bytes EXACT (2 cache lines) — vérifié statiquement
//   • ThreadAiState inline (8 bytes) — zéro allocation heap (Règle IA-KERNEL-01)
//   • NO_ALLOC — aucun Vec/Box/Arc dans ce fichier (Zone NO-ALLOC)
//   • dma_completion_result : AtomicU8 (requis par process/state/wakeup.rs)
//   • UNSAFE : tout bloc unsafe documenté par // SAFETY:
//
// LAYOUT CACHE (128 bytes = 2 × 64 bytes) — optimisé pour pick_next_task() :
//   CL1 [0..64]   : champs lus par le scheduler à chaque tick
//   CL2 [64..128] : champs utilisés lors du context switch (cold)
//
// INSTRUMENTATION : stats séparées dans TaskStats pour ne pas polluer le TCB.
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use core::mem::size_of;

// ─────────────────────────────────────────────────────────────────────────────
// Types fondamentaux
// ─────────────────────────────────────────────────────────────────────────────

/// Identifiant de thread — unique dans tout le système.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[repr(transparent)]
pub struct ThreadId(pub u32);

/// Identifiant de processus.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[repr(transparent)]
pub struct ProcessId(pub u32);

/// Identifiant CPU logique.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[repr(transparent)]
pub struct CpuId(pub u32);

/// Priorité d'ordonnancement (0 = plus haute priorité, 139 = plus basse).
/// Niveaux : 0–99 = RT, 100–139 = CFS/BATCH, 140 = IDLE.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[repr(transparent)]
pub struct Priority(pub u8);

impl Priority {
    pub const RT_MAX: Self = Self(0);
    pub const RT_MIN: Self = Self(99);
    pub const NORMAL_MAX: Self = Self(100);
    pub const NORMAL_DEFAULT: Self = Self(120);
    pub const NORMAL_MIN: Self = Self(139);
    pub const IDLE: Self = Self(140);

    /// `nice` value Linux-compatible (-20..+19) → priority 100..139.
    #[inline(always)]
    pub fn from_nice(nice: i8) -> Self {
        let n = nice.clamp(-20, 19);
        Self((120i16 + n as i16) as u8)
    }

    /// Retourne vrai si ce thread est temps-réel.
    #[inline(always)]
    pub fn is_realtime(self) -> bool {
        self.0 <= 99
    }
}

/// Politique d'ordonnancement.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum SchedPolicy {
    /// Complètement fair (normal + batch).
    Normal     = 0,
    Batch      = 1,
    /// Temps réel à priorité fixe.
    Fifo       = 2,
    RoundRobin = 3,
    /// EDF (Earliest Deadline First).
    Deadline   = 4,
    /// Thread idle uniquement.
    Idle       = 5,
}

/// Poids CFS standard Linux — table prio_to_weight[40] (nice -20..+19).
const PRIO_TO_WEIGHT: [u32; 40] = [
    88761, 71755, 56483, 46273, 36291,
    29154, 23254, 18705, 14949, 11916,
     9548,  7620,  6100,  4904,  3906,
     3121,  2501,  1991,  1586,  1277,
     1024,   820,   655,   526,   423,
      335,   272,   215,   172,   137,
      110,    87,    70,    56,    45,
       36,    29,    23,    18,    15,
];

impl Priority {
    /// Retourne le poids CFS de cette priorité (Linux-compatible).
    #[inline(always)]
    pub fn cfs_weight(self) -> u32 {
        if self.0 < 100 {
            return 1024; // RT : poids NICE_0_LOAD fixe pour ghost accounting
        }
        let nice_idx = (self.0 as i32 - 120 + 20).clamp(0, 39) as usize;
        PRIO_TO_WEIGHT[nice_idx]
    }
}

/// État du thread dans la machine d'états.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum TaskState {
    /// Prêt à tourner.
    Runnable   = 0,
    /// En cours d'exécution sur un CPU.
    Running    = 1,
    /// Bloqué en attente interruptible (signal réveille).
    Sleeping   = 2,
    /// Bloqué en attente non interruptible (reclaim mémoire).
    Uninterruptible = 3,
    /// Arrêté (SIGSTOP).
    Stopped    = 4,
    /// Terminé, en attente de reap.
    Zombie     = 5,
    /// Mort, ressources libérées.
    Dead       = 6,
}

// ─────────────────────────────────────────────────────────────────────────────
// DeadlineParams — paramètres SCHED_DEADLINE (EDF)
// ─────────────────────────────────────────────────────────────────────────────

/// Paramètres EDF — runtime / deadline / period en nanosecondes.
#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct DeadlineParams {
    /// Budget d'exécution par période (ns).
    pub runtime_ns:  u64,
    /// Délai absolu depuis le début de la période (ns).
    pub deadline_ns: u64,
    /// Période de récurrence (ns).
    pub period_ns:   u64,
}

impl Default for DeadlineParams {
    #[inline]
    fn default() -> Self {
        Self { runtime_ns: 0, deadline_ns: 0, period_ns: 0 }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ThreadControlBlock — structure centrale du scheduler
// INVARIANT : size_of::<ThreadControlBlock>() <= 128 bytes
// ─────────────────────────────────────────────────────────────────────────────

/// Thread Control Block — 128 bytes exactement (2 cache lines), aligné 64 bytes.
///
/// Layout:
///  Cache line 1 [0..64]   — HOT PATH pick_next_task() (100-150 cycles cible)
///    tid(4)+pid(4)+cpu(4)+_a(4)+affinity(8)+policy(1)+prio(1)+state(1)+_ps(1)+
///    flags(4)+vruntime(8)+deadline_abs(8)+signal_pending(1)+dma_result(1)+_p1(14)
///    = 4+4+4+4+8+1+1+1+1+4+8+8+1+1+14 = 64 ✓
///
///  Cache line 2 [64..128]  — CONTEXT SWITCH / COLD
///    kernel_rsp(8)+cr3(8)+fpu_ptr(8)+signal_mask(8)+_pad2(8)+deadline_params(24)
///    = 8+8+8+8+8+24 = 64 ✓
#[repr(C, align(64))]
pub struct ThreadControlBlock {
    // ═══════════════════════════════════════════════════════
    // Cache line 1 — HOT PATH (bytes 0..64)
    // ═══════════════════════════════════════════════════════

    /// Identifiant unique du thread.                      [+0]  4B
    pub tid:                   ThreadId,
    /// Processus parent.                                  [+4]  4B
    pub pid:                   ProcessId,
    /// CPU courant (AtomicU32).                           [+8]  4B
    pub cpu:                   AtomicU32,
    /// Alignement pour cpu_affinity (AtomicU64 exige 8B). [+12] 4B
    _align_affinity:           u32,
    /// Bitmask affinité CPU (max 64 CPUs logiques).        [+16] 8B
    pub cpu_affinity:          AtomicU64,
    /// Politique d'ordonnancement.                        [+24] 1B
    pub policy:                SchedPolicy,
    /// Priorité (0–140).                                  [+25] 1B
    pub priority:              Priority,
    /// État courant (valeur de TaskState).                [+26] 1B
    pub state:                 AtomicU8,
    /// Padding état → flags (align 4B).                   [+27] 1B
    _pad_state:                u8,
    /// Drapeaux (task_flags::*).                          [+28] 4B
    pub flags:                 AtomicU32,
    /// Virtual runtime CFS (ns, monotone croissant).      [+32] 8B
    pub vruntime:              AtomicU64,
    /// Deadline absolue EDF (ns depuis boot).             [+40] 8B
    pub deadline_abs:          AtomicU64,
    /// Signal en attente — ÉCRIT par process/signal/ UNIQUEMENT. [+48] 1B
    pub signal_pending:        AtomicBool,
    /// Résultat DMA — stocké par ProcessWakeupHandler.    [+49] 1B
    pub dma_completion_result: AtomicU8,
    /// Padding fin CL1.                                   [+50] 14B → CL1=64
    _pad1:                     [u8; 14],

    // ═══════════════════════════════════════════════════════
    // Cache line 2 — CONTEXT SWITCH / COLD (bytes 64..128)
    // ═══════════════════════════════════════════════════════

    /// RSP kernel au moment du context switch.            [+64] 8B
    pub kernel_rsp:            u64,
    /// CR3 de l'espace d'adressage (KPTI).                [+72] 8B
    pub cr3:                   u64,
    /// Pointeur FpuState (512B aligné 64). NULL → jamais FPU. [+80] 8B
    pub fpu_state_ptr:         *mut u8,
    /// Bitmask signaux bloqués (sigprocmask).             [+88] 8B
    pub signal_mask:           AtomicU64,
    /// Réservé (remà profil thread) — 8 octets.           [+96] 8B
    _pad2:                     [u8; 8],
    /// Paramètres EDF (runtime/deadline/period, ns).      [+104] 24B → CL2=64
    pub deadline_params:       DeadlineParams,
}

// Vérifications statiques du layout TCB.
const _: () = assert!(
    size_of::<ThreadControlBlock>() == 128,
    "TCB doit faire exactement 128 bytes (2 cache lines)"
);
const _: () = assert!(
    core::mem::align_of::<ThreadControlBlock>() == 64,
    "TCB doit être aligné sur 64 bytes"
);

/// Flag constants pour `ThreadControlBlock::flags`.
pub mod task_flags {
    /// Thread kernel (ne passe jamais en userspace).
    pub const KTHREAD:          u32 = 1 << 0;
    /// FPU chargée dans les registres physiques.
    pub const FPU_LOADED:       u32 = 1 << 1;
    /// Thread en cours de terminaison (exit()).
    pub const EXITING:          u32 = 1 << 2;
    /// Wakeup spurieux toléré (attentes conditionnelles).
    pub const WAKEUP_SPURIOUS:  u32 = 1 << 3;
    /// Préemption forcée demandée (TIF_NEED_RESCHED équivalent).
    pub const NEED_RESCHED:     u32 = 1 << 4;
    /// Thread en reclaim mémoire (évite deadlock EmergencyPool).
    pub const IN_RECLAIM:       u32 = 1 << 5;
    /// Thread migré inter-CPU (compteur statistique).
    pub const MIGRATED:         u32 = 1 << 6;
    /// Thread sous ptrace/debug.
    pub const PTRACE:           u32 = 1 << 7;
    /// Thread bloqué dans une wait_queue.
    pub const IN_WAIT_QUEUE:    u32 = 1 << 8;
    /// Thread idle (aucun travail disponible).
    pub const IS_IDLE:          u32 = 1 << 9;
}

impl ThreadControlBlock {
    /// Crée un nouveau TCB. `kernel_stack_top` : adresse du sommet du stack kernel.
    pub fn new(
        tid:              ThreadId,
        pid:              ProcessId,
        policy:           SchedPolicy,
        prio:             Priority,
        cr3:              u64,
        kernel_stack_top: u64,
    ) -> Self {
        debug_assert!(
            !((policy == SchedPolicy::Fifo || policy == SchedPolicy::RoundRobin)
                && !prio.is_realtime()),
            "Politique RT exige priorité RT (0..=99)"
        );
        Self {
            tid,
            pid,
            cpu:                   AtomicU32::new(0),
            _align_affinity:       0,
            cpu_affinity:          AtomicU64::new(!0u64),
            policy,
            priority:              prio,
            state:                 AtomicU8::new(TaskState::Runnable as u8),
            _pad_state:            0,
            flags:                 AtomicU32::new(0),
            vruntime:              AtomicU64::new(0),
            deadline_abs:          AtomicU64::new(0),
            signal_pending:        AtomicBool::new(false),
            dma_completion_result: AtomicU8::new(0),
            _pad1:                 [0u8; 14],
            kernel_rsp:            kernel_stack_top,
            cr3,
            fpu_state_ptr:         core::ptr::null_mut(),
            signal_mask:           AtomicU64::new(0),
            _pad2:                 [0u8; 8],
            deadline_params:       DeadlineParams::default(),
        }
    }

    /// Thread kernel dédié (pid=0, policy Normal, prio default).
    pub fn new_kthread(tid: ThreadId, cr3: u64, kernel_stack_top: u64) -> Self {
        let t = Self::new(
            tid, ProcessId(0), SchedPolicy::Normal,
            Priority::NORMAL_DEFAULT, cr3, kernel_stack_top,
        );
        t.flags.store(task_flags::KTHREAD, Ordering::Relaxed);
        t
    }

    // ─── État ───────────────────────────────────────────────────────────────

    /// Lit l'état courant du thread.
    #[inline(always)]
    pub fn state(&self) -> TaskState {
        // SAFETY: AtomicU8 contient toujours une valeur TaskState valide (invariant: seul set_state écrit).
        unsafe { core::mem::transmute(self.state.load(Ordering::Acquire)) }
    }

    /// Définit l'état du thread (Release pour visibilité cross-CPU).
    #[inline(always)]
    pub fn set_state(&self, s: TaskState) {
        self.state.store(s as u8, Ordering::Release);
    }

    /// Transition CAS — retourne `true` si la transition a réussi.
    #[inline(always)]
    pub fn try_transition(&self, from: TaskState, to: TaskState) -> bool {
        self.state
            .compare_exchange(from as u8, to as u8, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
    }

    // ─── Flags ──────────────────────────────────────────────────────────────

    /// Demande une préemption forcée (thread-safe depuis tout CPU).
    #[inline(always)]
    pub fn request_preemption(&self) {
        self.flags.fetch_or(task_flags::NEED_RESCHED, Ordering::Release);
    }

    /// Lit et efface `NEED_RESCHED` atomiquement.
    #[inline(always)]
    pub fn take_need_resched(&self) -> bool {
        self.flags.fetch_and(!task_flags::NEED_RESCHED, Ordering::AcqRel)
            & task_flags::NEED_RESCHED != 0
    }

    /// Vrai si la préemption est demandée (lecture sans reset).
    #[inline(always)]
    pub fn need_resched(&self) -> bool {
        self.flags.load(Ordering::Relaxed) & task_flags::NEED_RESCHED != 0
    }

    /// Vrai si la FPU est chargée dans les registres physiques.
    #[inline(always)]
    pub fn fpu_loaded(&self) -> bool {
        self.flags.load(Ordering::Relaxed) & task_flags::FPU_LOADED != 0
    }

    /// Marque la FPU comme chargée ou non.
    #[inline(always)]
    pub fn set_fpu_loaded(&self, loaded: bool) {
        if loaded {
            self.flags.fetch_or(task_flags::FPU_LOADED, Ordering::Relaxed);
        } else {
            self.flags.fetch_and(!task_flags::FPU_LOADED, Ordering::Relaxed);
        }
    }

    #[inline(always)] pub fn is_kthread(&self) -> bool {
        self.flags.load(Ordering::Relaxed) & task_flags::KTHREAD != 0
    }
    #[inline(always)] pub fn is_idle(&self) -> bool {
        self.flags.load(Ordering::Relaxed) & task_flags::IS_IDLE != 0
    }
    #[inline(always)] pub fn is_exiting(&self) -> bool {
        self.flags.load(Ordering::Relaxed) & task_flags::EXITING != 0
    }

    // ─── Scheduling ─────────────────────────────────────────────────────────

    /// Avance le vruntime CFS (delta_ns, weight = Priority::cfs_weight()).
    #[inline(always)]
    pub fn advance_vruntime(&self, delta_ns: u64, weight: u32) {
        const NICE_0_LOAD: u64 = 1024;
        let weighted = if weight == 0 { delta_ns }
            else { delta_ns.saturating_mul(NICE_0_LOAD) / weight as u64 };
        self.vruntime.fetch_add(weighted, Ordering::Relaxed);
    }

    #[inline(always)]
    pub fn current_cpu(&self) -> CpuId { CpuId(self.cpu.load(Ordering::Relaxed)) }

    #[inline(always)]
    pub fn assign_cpu(&self, cpu: CpuId) { self.cpu.store(cpu.0, Ordering::Relaxed); }

    /// Vrai si ce thread peut tourner sur le CPU donné (bitmask d'affinité).
    #[inline(always)]
    pub fn allowed_on(&self, cpu: CpuId) -> bool {
        if cpu.0 >= 64 { return false; }
        self.cpu_affinity.load(Ordering::Relaxed) & (1u64 << cpu.0) != 0
    }

    // ─── Signaux ────────────────────────────────────────────────────────────

    /// Positionne le flag signal_pending (appelé UNIQUEMENT depuis process/signal/).
    #[inline(always)]
    pub fn set_signal_pending(&self) {
        self.signal_pending.store(true, Ordering::Release);
    }

    /// Lit le flag (hot path scheduler, Relaxed).
    #[inline(always)]
    pub fn has_signal_pending(&self) -> bool {
        self.signal_pending.load(Ordering::Relaxed)
    }

    /// Efface le flag après traitement dans process/signal/delivery.rs.
    #[inline(always)]
    pub fn clear_signal_pending(&self) {
        self.signal_pending.store(false, Ordering::Release);
    }
}

// SAFETY: ThreadControlBlock partagé entre CPUs via raw pointers.
// Les champs mutables sont atomiques ; kernel_rsp et fpu_state_ptr ne sont
// accédés que du CPU propriétaire à la fois (garantie des invariants scheduler).
unsafe impl Send for ThreadControlBlock {}
unsafe impl Sync for ThreadControlBlock {}

// ─────────────────────────────────────────────────────────────────────────────
// Statistiques par thread — séparées du TCB pour ne pas dépasser 128 bytes.
// Allouées séparément, pointeur éventuel dans une ext. struct.
// ─────────────────────────────────────────────────────────────────────────────

/// Compteurs de performance par thread — séparés du TCB (TCB = 128B fixe).
#[repr(C, align(64))]
pub struct TaskStats {
    /// Context switches volontaires (sleep, yield, mutex wait…).
    pub voluntary_switches:   AtomicU64,
    /// Context switches involontaires (préemptions timer + IPI).
    pub involuntary_switches: AtomicU64,
    /// Temps total en Running (ns).
    pub run_time_ns:          AtomicU64,
    /// Temps total bloqué (sleep + uninterruptible, ns).
    pub blocked_time_ns:      AtomicU64,
    /// Migrations inter-CPU.
    pub migrations:           AtomicU64,
    /// Page faults utilisateur.
    pub page_faults:          AtomicU64,
    /// Timestamp dernier démarrage sur CPU (ns monotone).
    pub last_start_ns:        AtomicU64,
    /// Priority inversions détectées par le kernel mutex.
    pub priority_inversions:  AtomicU64,
}

impl TaskStats {
    pub const fn new() -> Self {
        Self {
            voluntary_switches:   AtomicU64::new(0),
            involuntary_switches: AtomicU64::new(0),
            run_time_ns:          AtomicU64::new(0),
            blocked_time_ns:      AtomicU64::new(0),
            migrations:           AtomicU64::new(0),
            page_faults:          AtomicU64::new(0),
            last_start_ns:        AtomicU64::new(0),
            priority_inversions:  AtomicU64::new(0),
        }
    }

    #[inline(always)]
    pub fn record_run_start(&self, now_ns: u64) {
        self.last_start_ns.store(now_ns, Ordering::Relaxed);
    }

    #[inline(always)]
    pub fn record_run_end(&self, now_ns: u64) {
        let start = self.last_start_ns.load(Ordering::Relaxed);
        if now_ns > start {
            self.run_time_ns.fetch_add(now_ns - start, Ordering::Relaxed);
        }
    }

    #[inline(always)]
    pub fn record_switch(&self, voluntary: bool) {
        if voluntary { self.voluntary_switches.fetch_add(1, Ordering::Relaxed); }
        else { self.involuntary_switches.fetch_add(1, Ordering::Relaxed); }
    }
}
