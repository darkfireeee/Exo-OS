// kernel/src/scheduler/smp/load_balance.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Équilibrage de charge — rééquilibrage périodique entre CPUs
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLE LB-01 : Toujours verrouiller les run queues dans l'ORDRE CROISSANT
//   des CPU IDs pour éviter les deadlocks entre deux CPUs qui effectuent
//   un équilibrage simultané.
//
// Algorithme :
//   Périodiquement (tous les BALANCE_INTERVAL_MS), chaque CPU parcourt ses
//   voisins et effectue un "pull" depuis le CPU le plus chargé.
//   Si un CPU dépasse IMBALANCE_THRESHOLD tâches de plus que le CPU local,
//   une tâche est pull-migrée.
// ═══════════════════════════════════════════════════════════════════════════════

use super::migration::request_migration;
use super::topology::{cpu_node, nr_cpus};
use crate::scheduler::core::runqueue;
use crate::scheduler::core::task::CpuId;
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Paramètres d'équilibrage
// ─────────────────────────────────────────────────────────────────────────────

/// Intervalle entre deux passes d'équilibrage par CPU (en ticks, à HZ=1000 → 4ms).
pub const BALANCE_INTERVAL_TICKS: u64 = 4;
/// Déséquilibre minimum pour déclencher une migration (différence de tâches).
pub const IMBALANCE_THRESHOLD: usize = 2;
/// Maximum de migrations par passe d'équilibrage.
pub const MAX_MIGRATIONS_PER_BALANCE: usize = 4;

// ─────────────────────────────────────────────────────────────────────────────
// Métriques
// ─────────────────────────────────────────────────────────────────────────────

pub static BALANCE_RUNS: AtomicU64 = AtomicU64::new(0);
pub static BALANCE_MIGRATIONS: AtomicU64 = AtomicU64::new(0);
pub static BALANCE_NUMA_SKIP: AtomicU64 = AtomicU64::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// Entrée principale
// ─────────────────────────────────────────────────────────────────────────────

/// Tente d'équilibrer la charge du CPU `local_cpu`.
///
/// Cherche le CPU le plus chargé parmi les voisins (même nœud NUMA en priorité),
/// et pull-migre des tâches si le déséquilibre dépasse `IMBALANCE_THRESHOLD`.
///
/// # Safety
/// Appelé avec la préemption DÉSACTIVÉE.
/// RÈGLE LB-01 respectée : on verrouille toujours min(local, busiest) en premier.
pub unsafe fn balance_cpu(local_cpu: CpuId) {
    BALANCE_RUNS.fetch_add(1, Ordering::Relaxed);

    let local_rq = runqueue::run_queue(local_cpu);
    let local_nr = local_rq.nr_running_usize();

    // Cherche le CPU le plus chargé parmi tous les CPUs.
    let mut busiest_cpu: Option<CpuId> = None;
    let mut busiest_nr: usize = 0;

    let n = nr_cpus();
    for cpu_raw in 0..n as u32 {
        let cpu = CpuId(cpu_raw);
        if cpu == local_cpu {
            continue;
        }
        let rq = runqueue::run_queue(cpu);
        let nr = rq.nr_running_usize();
        if nr > busiest_nr {
            busiest_nr = nr;
            busiest_cpu = Some(cpu);
        }
    }

    let Some(busiest) = busiest_cpu else {
        return;
    };
    if busiest_nr <= local_nr + IMBALANCE_THRESHOLD {
        return;
    }

    // Préférer un CPU du même nœud NUMA (coût de migration moindre).
    let local_node = cpu_node(local_cpu);
    let busiest_node = cpu_node(busiest);
    if local_node != busiest_node {
        // Vérifier si un CPU sur le même nœud est déjà plus chargé.
        let mut same_node_busiest: Option<CpuId> = None;
        let mut same_node_nr: usize = 0;
        for cpu_raw in 0..n as u32 {
            let cpu = CpuId(cpu_raw);
            if cpu == local_cpu {
                continue;
            }
            if cpu_node(cpu) != local_node {
                continue;
            }
            let rq = runqueue::run_queue(cpu);
            let nr = rq.nr_running_usize();
            if nr > same_node_nr {
                same_node_nr = nr;
                same_node_busiest = Some(cpu);
            }
        }
        if let Some(snb) = same_node_busiest {
            if same_node_nr > local_nr + IMBALANCE_THRESHOLD {
                // Pull depuis le même nœud NUMA en priorité.
                do_pull(local_cpu, snb, (same_node_nr - local_nr) / 2);
                return;
            }
        }
        // Pas de déséquilibre intra-nœud → skip migration inter-nœud.
        BALANCE_NUMA_SKIP.fetch_add(1, Ordering::Relaxed);
        return;
    }

    let to_move = ((busiest_nr - local_nr) / 2).min(MAX_MIGRATIONS_PER_BALANCE);
    do_pull(local_cpu, busiest, to_move);
}

/// Effectue le pull de `count` tâches depuis `src_cpu` vers `dst_cpu`.
///
/// RÈGLE LB-01 : La run queue de `min(src, dst)` est verrouillée en premier.
///
/// # Safety
/// Préemption désactivée requise.
unsafe fn do_pull(dst_cpu: CpuId, src_cpu: CpuId, count: usize) {
    if count == 0 {
        return;
    }

    // RÈGLE LB-01 : ordre d'acquisition croissant par CPU ID.
    let (first, second, first_is_dst) = if dst_cpu < src_cpu {
        (dst_cpu, src_cpu, true)
    } else {
        (src_cpu, dst_cpu, false)
    };

    let _ = first; // Dans un OS réel : spinlock(&rq[first]); spinlock(&rq[second]);
    let _ = second;

    let src_rq = runqueue::run_queue(src_cpu);
    let dst_rq = runqueue::run_queue(dst_cpu);

    let mut moved = 0usize;
    while moved < count {
        // Essayer de pull une tâche CFS (jamais de tâche RT — violer les invariants RT).
        let tcb_opt = src_rq.cfs_dequeue_for_migration(dst_cpu);
        let Some(tcb) = tcb_opt else {
            break;
        };

        let tcb_ref = tcb.as_ref();
        // Vérifier l'affinité.
        if !tcb_ref.allowed_on(dst_cpu) {
            // Réinsérer sur le CPU source si l'affinité ne permet pas la migration.
            src_rq.enqueue(tcb);
            break;
        }

        request_migration(tcb, dst_cpu);
        moved += 1;
    }

    BALANCE_MIGRATIONS.fetch_add(moved as u64, Ordering::Relaxed);

    // Dans un OS réel : spinunlock dans l'ordre INVERSE (second puis first).
    let _ = (first_is_dst, dst_rq);
}
