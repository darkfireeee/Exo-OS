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

use super::preempt::assert_preempt_disabled;
use super::runqueue::{PerCpuRunQueue, MAX_TASKS_PER_CPU};
use super::task::{TaskState, ThreadControlBlock};
use core::ptr::NonNull;
use core::sync::atomic::{AtomicU64, Ordering};

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

const MAX_PICK_SKIP_SCAN: usize = MAX_TASKS_PER_CPU + 256 + 32;

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
    rq: &mut PerCpuRunQueue,
    current: Option<NonNull<ThreadControlBlock>>,
) -> PickResult {
    assert_preempt_disabled();

    PICK_NEXT_TOTAL.fetch_add(1, Ordering::Relaxed);

    // Cas rapide : si le thread courant est RT et toujours le plus prioritaire,
    // on ne fait pas de switch (optimisation hot path).
    if let Some(cur) = current {
        let cur_ref = cur.as_ref();
        if cur_ref.state() == TaskState::Running
            && (cur_ref.policy == crate::scheduler::core::task::SchedPolicy::Fifo
                || cur_ref.policy == crate::scheduler::core::task::SchedPolicy::RoundRobin)
        {
            // Vérifier si un thread RT de priorité supérieure attend.
            if let Some(best_prio) = rq.rt_bitmap_highest_prio() {
                if best_prio < cur_ref.priority.0 {
                    // Préempter le thread RT courant au profit d'un RT plus prioritaire.
                    PICK_RT_RT.fetch_add(1, Ordering::Relaxed);
                    // Re-enqueuer le courant: meme FIFO doit rester pret apres
                    // preemption par une priorite RT superieure.
                    cur_ref.set_state(TaskState::Runnable);
                    rq.enqueue(cur);
                    if let Some(next) = rq.dequeue_highest_rt() {
                        return PickResult::Switch(next);
                    }
                    return PickResult::KeepRunning;
                }
            }
            // Pas de RT plus prioritaire → le courant continue.
            PICK_SAME_CURRENT.fetch_add(1, Ordering::Relaxed);
            return PickResult::KeepRunning;
        }
    }

    // Sélection standard. Une runqueue peut contenir transitoirement une entree
    // stale apres un reveil/preemption concurrent; on la depile et on continue
    // au lieu de basculer vers idle alors qu'un parent runnable attend derriere.
    for _ in 0..MAX_PICK_SKIP_SCAN {
        match rq.pick_next() {
            None => return PickResult::GoIdle,
            Some(next) => {
                // Vérification : seul un thread Runnable peut être sélectionné.
                // Exception: l'idle courant est publié comme Running et sert de
                // dernier recours quand aucune file active n'a de travail.
                let next_ref = next.as_ref();
                let state = next_ref.state();
                let idle_running = state == TaskState::Running && next_ref.is_idle();
                if state != TaskState::Runnable && !idle_running {
                    // Thread non éligible (zombie, stopped, etc.) — on le réinsère pas.
                    PICK_SKIP_INELIGIBLE.fetch_add(1, Ordering::Relaxed);
                    continue;
                }

                // Sélection déterministe CFS pure — aucun ajustement heuristique.
                let final_next = next;

                if current == Some(final_next) {
                    PICK_SAME_CURRENT.fetch_add(1, Ordering::Relaxed);
                    return PickResult::KeepRunning;
                } else {
                    return PickResult::Switch(final_next);
                }
            }
        }
    }

    PickResult::GoIdle
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
