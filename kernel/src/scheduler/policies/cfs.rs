// kernel/src/scheduler/policies/cfs.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// CFS — Completely Fair Scheduler (Exo-OS Scheduler · Couche 1)
// ═══════════════════════════════════════════════════════════════════════════════
//
// CFS implante un scheduling proportionnel : chaque thread reçoit une part
// du CPU proportionnelle à son poids (basé sur nice/priority).
//
// Invariant principal : le thread avec le plus petit `vruntime` est toujours
// choisi en premier. Le vruntime est pondéré pour compenser les différences
// de priorité (threads plus prioritaires avancent plus lentement).
//
// Ce module fournit la logique de décision CFS ; la run queue (min-heap trié
// par vruntime) est dans core/runqueue.rs.
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, Ordering};
use crate::scheduler::core::task::{ThreadControlBlock, TaskState, SchedPolicy};
use crate::scheduler::core::runqueue::{PerCpuRunQueue, CFS_TARGET_LATENCY_MS, CFS_MIN_GRANULARITY_US};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes CFS
// ─────────────────────────────────────────────────────────────────────────────

/// Quantum minimal garanti pour chaque thread même en cas de file longue (ns).
pub const CFS_MIN_SLICE_NS: u64 = CFS_MIN_GRANULARITY_US * 1000;
/// Période de latence cible (ns) — time dans laquelle tout thread doit tourner.
pub const CFS_TARGET_PERIOD_NS: u64 = CFS_TARGET_LATENCY_MS * 1_000_000;
/// Seuil de wakeup preemption : si le thread sortant a avancé son vruntime de
/// plus que ce seuil par rapport au min_vruntime, on préempte immédiatement.
pub const CFS_WAKEUP_PREEMPT_NS: u64 = 1_000_000; // 1ms

/// Compteur global de préemptions CFS (instrumentation).
pub static CFS_PREEMPTIONS: AtomicU64 = AtomicU64::new(0);
/// Compteur global de wakeup-preemptions (instrumentation).
/// BUG-FIX I : correction de la typo (PREMPT → PREEMPT).
pub static CFS_WAKEUP_PREEMPT_COUNT: AtomicU64 = AtomicU64::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// Logique CFS
// ─────────────────────────────────────────────────────────────────────────────

/// Calcule le quantum alloué à `tcb` selon le nombre de threads dans la run queue.
///
/// Formule Linux CFS :
///   slice = max(target_period × weight / total_weight, min_granularity)
///
/// Avec `nr_tasks` threads de même poids :
///   slice = target_period / nr_tasks  (≥ min_granularity)
pub fn timeslice_for(tcb: &ThreadControlBlock, nr_tasks: usize, total_weight: u64) -> u64 {
    if nr_tasks == 0 { return CFS_TARGET_PERIOD_NS; }
    let weight = tcb.priority.cfs_weight() as u64;
    let raw_slice = if total_weight == 0 {
        CFS_TARGET_PERIOD_NS / nr_tasks as u64
    } else {
        CFS_TARGET_PERIOD_NS.saturating_mul(weight) / total_weight
    };
    raw_slice.max(CFS_MIN_SLICE_NS)
}

/// Vérifie si le thread courant doit être préempté au profit d'un thread plus
/// léger (à vruntime plus petit) venant de se réveiller.
///
/// RÈGLE : appelé dans `wakeup_thread()` après réinsertion dans la queue CFS.
/// Retourne `true` si une préemption est souhaitable.
pub fn should_preempt_on_wakeup(
    running:  &ThreadControlBlock,
    woken:    &ThreadControlBlock,
    min_vruntime: u64,
) -> bool {
    // Ne préempter que si le thread actuel n'est pas RT.
    if running.priority.is_realtime() { return false; }

    let running_vr = running.vruntime.load(Ordering::Relaxed);
    let woken_vr   = woken.vruntime.load(Ordering::Relaxed);

    // Préempter si le thread réveillé a un vruntime significativement plus petit.
    if woken_vr + CFS_WAKEUP_PREEMPT_NS < running_vr {
        CFS_WAKEUP_PREEMPT_COUNT.fetch_add(1, Ordering::Relaxed);
        return true;
    }
    false
}

/// Normalise le vruntime d'un thread nouvellement enfilé pour éviter qu'il
/// monopolise le CPU en revenant toujours avec un vruntime très bas.
///
/// Règle CFS : placer le nouveau thread à `max(son_vr, min_vruntime - timeslice/2)`.
pub fn normalize_vruntime_on_enqueue(tcb: &ThreadControlBlock, min_vruntime: u64, slice_ns: u64) {
    let current_vr = tcb.vruntime.load(Ordering::Relaxed);
    let floor = min_vruntime.saturating_sub(slice_ns / 2);
    if current_vr < floor {
        // SAFETY: on est l'unique producteur — la run queue a la main sur le TCB.
        tcb.vruntime.store(floor, Ordering::Relaxed);
    }
}

/// Appelé par le timer tick pour vérifier si le thread courant a épuisé son quantum.
///
/// Retourne `true` si le thread doit être préempté.
pub fn tick_check_preempt(
    tcb:           &ThreadControlBlock,
    elapsed_ns:    u64,
    slice_ns:      u64,
    nr_tasks:      usize,
) -> bool {
    if tcb.policy != SchedPolicy::Normal && tcb.policy != SchedPolicy::Batch {
        return false;
    }
    if nr_tasks <= 1 {
        // Seul thread → aucune préemption nécessaire.
        return false;
    }
    if elapsed_ns >= slice_ns {
        CFS_PREEMPTIONS.fetch_add(1, Ordering::Relaxed);
        return true;
    }
    false
}
