// kernel/src/scheduler/smp/load_balance.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Load balancing — periodic rebalancing between CPUs
// ═══════════════════════════════════════════════════════════════════════════════
//
// RULE LB-01: Always lock run queues in INCREASING ORDER
//   of CPU IDs to avoid deadlocks between two CPUs performing
//   simultaneous load balancing.
//
// Algorithm:
//   Periodically (every BALANCE_INTERVAL_MS), each CPU scans its
//   neighbors and pulls from the busiest CPU.
//   If a CPU has IMBALANCE_THRESHOLD more tasks than the local CPU,
//   a task is pull-migrated.
// ═══════════════════════════════════════════════════════════════════════════════

use super::migration::request_migration;
use super::topology::{cpu_node, nr_cpus};
use crate::scheduler::core::runqueue;
use crate::scheduler::core::task::CpuId;
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Load balancing parameters
// ─────────────────────────────────────────────────────────────────────────────

/// Interval between two balancing passes per CPU (in ticks, at HZ=1000 → 4ms).
pub const BALANCE_INTERVAL_TICKS: u64 = 4;
/// Minimum imbalance to trigger migration (difference in tasks).
pub const IMBALANCE_THRESHOLD: usize = 2;
/// Maximum migrations per balancing pass.
pub const MAX_MIGRATIONS_PER_BALANCE: usize = 4;

// ─────────────────────────────────────────────────────────────────────────────
// Metrics
// ─────────────────────────────────────────────────────────────────────────────

pub static BALANCE_RUNS: AtomicU64 = AtomicU64::new(0);
pub static BALANCE_MIGRATIONS: AtomicU64 = AtomicU64::new(0);
pub static BALANCE_NUMA_SKIP: AtomicU64 = AtomicU64::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// Main entry point
// ─────────────────────────────────────────────────────────────────────────────

/// Attempts to balance load on CPU `local_cpu`.
///
/// Searches for the busiest CPU among neighbors (same NUMA node prioritized),
/// and pull-migrates tasks if imbalance exceeds `IMBALANCE_THRESHOLD`.
///
/// # Safety
/// Called with preemption DISABLED.
/// RULE LB-01 respected: always lock min(local, busiest) first.
pub unsafe fn balance_cpu(local_cpu: CpuId) {
    BALANCE_RUNS.fetch_add(1, Ordering::Relaxed);

    let local_nr = runqueue::run_queue(local_cpu).nr_running_usize();

    // Search for busiest CPU among all CPUs.
    let mut busiest_cpu: Option<CpuId> = None;
    let mut busiest_nr: usize = 0;

    let n = nr_cpus();
    for cpu_raw in 0..n as u32 {
        let cpu = CpuId(cpu_raw);
        if cpu == local_cpu {
            continue;
        }
        let nr = runqueue::run_queue(cpu).nr_running_usize();
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

    // Prefer a CPU on the same NUMA node (lower migration cost).
    let local_node = cpu_node(local_cpu);
    let busiest_node = cpu_node(busiest);
    if local_node != busiest_node {
        // Check if a CPU on the same node is already busier.
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
            let nr = runqueue::run_queue(cpu).nr_running_usize();
            if nr > same_node_nr {
                same_node_nr = nr;
                same_node_busiest = Some(cpu);
            }
        }
        if let Some(snb) = same_node_busiest {
            if same_node_nr > local_nr + IMBALANCE_THRESHOLD {
                // Pull from same NUMA node first.
                do_pull(local_cpu, snb, (same_node_nr - local_nr) / 2);
                return;
            }
        }
        // No intra-node imbalance → skip inter-node migration.
        BALANCE_NUMA_SKIP.fetch_add(1, Ordering::Relaxed);
        return;
    }

    let to_move = ((busiest_nr - local_nr) / 2).min(MAX_MIGRATIONS_PER_BALANCE);
    do_pull(local_cpu, busiest, to_move);
}

/// Performs pulling of `count` tasks from `src_cpu` to `dst_cpu`.
///
/// RULE LB-01: Run queue of `min(src, dst)` is locked first.
///
/// # Safety
/// Requires preemption disabled.
unsafe fn do_pull(dst_cpu: CpuId, src_cpu: CpuId, count: usize) {
    let capped = count.min(MAX_MIGRATIONS_PER_BALANCE);
    if capped == 0 || dst_cpu == src_cpu {
        return;
    }

    let mut pulled: [Option<core::ptr::NonNull<crate::scheduler::core::task::ThreadControlBlock>>;
        MAX_MIGRATIONS_PER_BALANCE] = [None; MAX_MIGRATIONS_PER_BALANCE];
    let mut moved = 0usize;

    let _ = runqueue::with_two_run_queues(src_cpu, dst_cpu, |src_rq, _dst_rq| {
        while moved < capped {
            // Try to pull a CFS task (never an RT or currently-running task).
            let Some(tcb) = src_rq.cfs_dequeue_for_migration(dst_cpu) else {
                break;
            };

            // Double-check affinity under source lock; restore immediately if disallowed.
            if !tcb.as_ref().allowed_on(dst_cpu) {
                src_rq.enqueue(tcb);
                break;
            }

            pulled[moved] = Some(tcb);
            moved += 1;
        }
    });

    for slot in pulled.into_iter().take(moved) {
        if let Some(tcb) = slot {
            request_migration(tcb, dst_cpu);
        }
    }

    BALANCE_MIGRATIONS.fetch_add(moved as u64, Ordering::Relaxed);
}
