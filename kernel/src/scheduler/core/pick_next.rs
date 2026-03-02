// kernel/src/scheduler/core/pick_next.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// PICK_NEXT_TASK — Sélection O(1) du prochain thread (Exo-OS Scheduler · Couche 1)
// ═══════════════════════════════════════════════════════════════════════════════
//
// OBJECTIF PERFORMANCE (DOC3) :
//   • pick_next_task() cible 100-150 cycles en hot path
//   • L'ordre de priorité est absolument respecté : RT > CFS > Idle
//   • Les hints IA (ai_guided.rs) ne peuvent que SUGGÉRER, jamais ignorer RT
//
// RÈGLES :
//   • NO-ALLOC — aucune allocation dans cette fonction
//   • Appelé avec préemption désactivée (vérification debug)
//   • Instrumentation complète (compteurs atomiques)
//   • Compatible SMP : chaque CPU appelle sur SA propre run queue
// ═══════════════════════════════════════════════════════════════════════════════

use core::ptr::NonNull;
use core::sync::atomic::{AtomicU64, Ordering};
use super::preempt::assert_preempt_disabled;
use super::runqueue::{PerCpuRunQueue, run_queue, MAX_TASKS_PER_CPU};
use super::task::{ThreadControlBlock, CpuId, TaskState};
use crate::scheduler::policies::ai_guided;

// ─────────────────────────────────────────────────────────────────────────────
// Compteurs globaux d'instrumentation
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre total d'appels pick_next_task() depuis le boot.
pub static PICK_NEXT_TOTAL: AtomicU64 = AtomicU64::new(0);
/// Nombre de fois que le thread courant a été reconduit sans switch.
pub static PICK_SAME_CURRENT: AtomicU64 = AtomicU64::new(0);
/// Nombre de switches RT → RT.
pub static PICK_RT_RT: AtomicU64 = AtomicU64::new(0);
/// Nombre de fois qu'un thread inéligible a été ignoré.
pub static PICK_SKIP_INELIGIBLE: AtomicU64 = AtomicU64::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// Résultat de pick_next
// ─────────────────────────────────────────────────────────────────────────────

/// Décision de scheduling pour ce tick.
pub enum PickResult {
    /// Aucun changement — le thread courant continue.
    KeepRunning,
    /// Switcher vers ce nouveau thread.
    Switch(NonNull<ThreadControlBlock>),
    /// Aller en idle (aucun thread à exécuter).
    GoIdle,
}

// ─────────────────────────────────────────────────────────────────────────────
// pick_next_task() — fonction centrale
// ─────────────────────────────────────────────────────────────────────────────

/// Sélectionne le prochain thread à exécuter sur le CPU courant.
///
/// # Préconditions
/// - Préemption désactivée sur le CPU appelant.
/// - La run queue est celle du CPU courant.
///
/// # Algorithme
/// 1. Si file RT non vide → déqueuer le thread RT le plus prioritaire.
/// 2. Sinon → déqueuer la tête CFS (vruntime minimal).
/// 3. Sinon → retourner thread idle.
///
/// Le hint IA est consulté UNIQUEMENT pour le tiebreak CFS (aucun impact RT).
///
/// # Garantie de performance
/// En l'absence de contention, cette fonction s'exécute en 100-150 cycles.
#[inline(always)]
pub unsafe fn pick_next_task(
    rq:      &mut PerCpuRunQueue,
    current: Option<NonNull<ThreadControlBlock>>,
) -> PickResult {
    assert_preempt_disabled();

    PICK_NEXT_TOTAL.fetch_add(1, Ordering::Relaxed);

    // Cas rapide : si le thread courant est RT et toujours le plus prioritaire,
    // on ne fait pas de switch (optimisation hot path).
    if let Some(cur) = current {
        let cur_ref = cur.as_ref();
        if cur_ref.policy == crate::scheduler::core::task::SchedPolicy::Fifo
            || cur_ref.policy == crate::scheduler::core::task::SchedPolicy::RoundRobin
        {
            // Vérifier si un thread RT de priorité supérieure attend.
            if let Some(best_prio) = rq.rt_bitmap_highest_prio() {
                if best_prio < cur_ref.priority.0 {
                    // Préempter le thread RT courant au profit d'un RT plus prioritaire.
                    PICK_RT_RT.fetch_add(1, Ordering::Relaxed);
                    // Re-enqueuer le courant (sauf FIFO qui garde la priorité).
                    if cur_ref.policy == crate::scheduler::core::task::SchedPolicy::RoundRobin {
                        rq.enqueue(cur);
                    }
                    let next = rq.dequeue_highest_rt().expect("RT queue non vide après bitmap check");
                    return PickResult::Switch(next);
                }
            }
            // Pas de RT plus prioritaire → le courant continue.
            PICK_SAME_CURRENT.fetch_add(1, Ordering::Relaxed);
            return PickResult::KeepRunning;
        }
    }

    // Sélection standard.
    let candidate = rq.pick_next();

    match candidate {
        None => PickResult::GoIdle,
        Some(next) => {
            // Vérification : le thread doit être en état Runnable.
            let state = next.as_ref().state();
            if state != TaskState::Runnable && state != TaskState::Running {
                // Thread non éligible (zombie, stopped, etc.) — on le réinsère pas.
                PICK_SKIP_INELIGIBLE.fetch_add(1, Ordering::Relaxed);
                // Continuer avec idle.
                return PickResult::GoIdle;
            }

            // Appliquer le hint IA pour potentiellement préférer un autre thread CFS.
            // RÈGLE IA-KERNEL-02 : si hint = None → fallback déterministe.
            // IA ne concerne que CFS — jamais RT.
            let final_next = if next.as_ref().policy == crate::scheduler::core::task::SchedPolicy::Normal
                || next.as_ref().policy == crate::scheduler::core::task::SchedPolicy::Batch
            {
                let preferred = ai_guided::maybe_prefer(rq, next);
                // BUG-FIX A : si l'IA a choisi un thread alternatif (alt != candidate) :
                //   – `next` (candidate) a été extrait par rq.pick_next() → le remettre.
                //   – `preferred` (alt) est encore dans la queue CFS via cfs_peek_second
                //     et doit être retiré avant d'être exécuté.
                // Sans cette correction : `next` est perdu et `preferred` est
                // double-schedulé (trong la queue ET en train de tourner).
                if preferred != next {
                    rq.enqueue(next);      // remettre le candidat original (+nr_running)
                    rq.remove(preferred);  // retirer l'élu IA de la queue (-nr_running)
                }
                preferred
            } else {
                next
            };

            if current == Some(final_next) {
                PICK_SAME_CURRENT.fetch_add(1, Ordering::Relaxed);
                PickResult::KeepRunning
            } else {
                PickResult::Switch(final_next)
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Mise à jour du vruntime après une tranche d'exécution
// ─────────────────────────────────────────────────────────────────────────────

/// Comptabilise le temps CPU du thread `tcb` pour `delta_ns` nanosecondes.
/// À appeler dans le tick handler, avec préemption désactivée.
#[inline(always)]
pub unsafe fn account_time(tcb: &ThreadControlBlock, delta_ns: u64) {
    let weight = tcb.priority.cfs_weight();
    match tcb.policy {
        crate::scheduler::core::task::SchedPolicy::Normal
        | crate::scheduler::core::task::SchedPolicy::Batch => {
            tcb.advance_vruntime(delta_ns, weight);
        }
        // RT : pas de vruntime — préemption basée sur priorité fixe.
        _ => {}
    }
}