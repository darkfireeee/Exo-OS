// kernel/src/scheduler/policies/realtime.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Politique Temps-Réel : SCHED_FIFO + SCHED_RR (Exo-OS · Couche 1)
// ═══════════════════════════════════════════════════════════════════════════════
//
// SCHED_FIFO : aucune préemption au sein d'une même priorité.
//              Un thread RT tourne jusqu'à ce qu'il se bloque ou cède.
//              Un thread RT de priorité plus haute peut préempter.
//
// SCHED_RR   : identique à FIFO mais chaque thread a un quantum de 10ms.
//              À l'expiration, le thread est réinséré à la fin de sa file
//              de priorité.
//
// Priorités RT : 1 (la plus basse) .. 99 (la plus haute) selon POSIX.
//   Interne : stocké dans Priority::rt_prio (0 = idle, 100–199 = RT 1–100)
//   La RunQueue RT utilise les niveaux 100–199 pour les files RT.
// ═══════════════════════════════════════════════════════════════════════════════

use crate::scheduler::core::runqueue::PerCpuRunQueue;
use crate::scheduler::core::task::{SchedPolicy, ThreadControlBlock};
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Quantum SCHED_RR (10ms en nanosecondes).
pub const RR_TIMESLICE_NS: u64 = 10_000_000;

/// Compteur de préemptions RT sur interruption de thread plus haute priorité.
pub static RT_PREEMPTIONS: AtomicU64 = AtomicU64::new(0);
/// Expirations de quantum RR.
pub static RR_QUANTUM_EXPIRATIONS: AtomicU64 = AtomicU64::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// Logique SCHED_FIFO
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie si un thread RT de priorité plus haute est en attente dans la run
/// queue, ce qui déclencherait une préemption du thread courant.
///
/// Retourne `true` si une préemption est nécessaire.
///
/// # BUG-FIX Q
/// L'ancien code utilisait `rt_highest_prio()` qui retourne 0 (= priorité
/// maximale RT) même quand la file RT est VIDE. Résultat : `highest_waiting (0)
/// < running_prio` était vrai pour tout thread non-RT (prio > 0), déclenchant
/// une préemption fantôme sans thread RT en attente.
/// Correction : `rt_bitmap_highest_prio()` retourne `None` si la file est
/// vide — la préemption n'est déclenchée que si un thread attend réellement.
pub fn fifo_should_preempt(rq: &PerCpuRunQueue, running: &ThreadControlBlock) -> bool {
    let running_prio = running.priority.0;
    // rt_bitmap_highest_prio() retourne None si aucun thread RT n'est prêt.
    match rq.rt_bitmap_highest_prio() {
        None => false,
        Some(highest_waiting) => {
            if highest_waiting < running_prio {
                RT_PREEMPTIONS.fetch_add(1, Ordering::Relaxed);
                true
            } else {
                false
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Logique SCHED_RR
// ─────────────────────────────────────────────────────────────────────────────

/// Appelé par le timer tick pour un thread SCHED_RR.
///
/// Retourne `true` si le quantum est expiré (le thread doit être réenfilé à la
/// fin de sa file de priorité).
pub fn rr_tick(tcb: &ThreadControlBlock, elapsed_since_schedule_ns: u64) -> bool {
    if tcb.policy != SchedPolicy::RoundRobin {
        return false;
    }
    if elapsed_since_schedule_ns >= RR_TIMESLICE_NS {
        RR_QUANTUM_EXPIRATIONS.fetch_add(1, Ordering::Relaxed);
        return true;
    }
    false
}

/// Quantum restant pour un thread SCHED_RR (en nanosecondes).
pub fn rr_remaining_slice(elapsed_since_schedule_ns: u64) -> u64 {
    RR_TIMESLICE_NS.saturating_sub(elapsed_since_schedule_ns)
}
