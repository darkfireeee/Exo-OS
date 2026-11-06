// kernel/src/scheduler/smp/migration.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Thread migration — moving a thread from one CPU to another
// ═══════════════════════════════════════════════════════════════════════════════
//
// Migrations are performed via IPI: source CPU sends an IPI to target CPU
// with the TCB to migrate. Target CPU integrates the thread into its run queue
// on the next tick.
//
// Migration channel: per-target-CPU lock-free circular array
// (up to MIGRATION_QUEUE_DEPTH pending migrations).
// ═══════════════════════════════════════════════════════════════════════════════

use super::topology::{nr_cpus, MAX_CPUS};
use crate::scheduler::core::task::{CpuId, ThreadControlBlock};
use core::ptr::NonNull;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Per-CPU migration channel
// ─────────────────────────────────────────────────────────────────────────────

const MIGRATION_QUEUE_DEPTH: usize = 16;

/// Circular migration queue for a target CPU.
struct MigrationQueue {
    head: AtomicU32,
    tail: AtomicU32,
    slots: [core::sync::atomic::AtomicPtr<ThreadControlBlock>; MIGRATION_QUEUE_DEPTH],
}

impl MigrationQueue {
    const fn new() -> Self {
        Self {
            head: AtomicU32::new(0),
            tail: AtomicU32::new(0),
            slots: [const { core::sync::atomic::AtomicPtr::new(core::ptr::null_mut()) }; MIGRATION_QUEUE_DEPTH],
        }
    }

    /// Enqueues `tcb` into queue. Returns `false` if full.
    fn push(&self, tcb: NonNull<ThreadControlBlock>) -> bool {
        let tail = self.tail.load(Ordering::Acquire);
        let next = (tail + 1) % MIGRATION_QUEUE_DEPTH as u32;
        if next == self.head.load(Ordering::Acquire) {
            return false; // Full.
        }
        self.slots[tail as usize].store(tcb.as_ptr(), Ordering::Release);
        self.tail.store(next, Ordering::Release);
        true
    }

    /// Dequeues next TCB. Returns `None` if empty.
    fn pop(&self) -> Option<NonNull<ThreadControlBlock>> {
        let head = self.head.load(Ordering::Acquire);
        if head == self.tail.load(Ordering::Acquire) {
            return None; // Empty.
        }
        let ptr = self.slots[head as usize].load(Ordering::Acquire);
        let next = (head + 1) % MIGRATION_QUEUE_DEPTH as u32;
        self.head.store(next, Ordering::Release);
        NonNull::new(ptr)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Global arrays (static, no_alloc)
// ─────────────────────────────────────────────────────────────────────────────

static MIGRATION_QUEUES: [MigrationQueue; MAX_CPUS] =
    [const { MigrationQueue::new() }; MAX_CPUS];

// ─────────────────────────────────────────────────────────────────────────────
// FFI to arch (IPI sending)
// ─────────────────────────────────────────────────────────────────────────────

extern "C" {
    fn arch_send_reschedule_ipi(target_cpu: u32);
}

// ─────────────────────────────────────────────────────────────────────────────
// Metrics
// ─────────────────────────────────────────────────────────────────────────────

pub static MIGRATIONS_SENT: AtomicU64 = AtomicU64::new(0);
pub static MIGRATIONS_RECEIVED: AtomicU64 = AtomicU64::new(0);
pub static MIGRATIONS_DROPPED: AtomicU64 = AtomicU64::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Requests migration of thread `tcb` to CPU `target`.
///
/// Thread is enqueued into target CPU migration queue, and a reschedule
/// IPI is sent.
///
/// # Safety
/// TCB must be dequeued from source run queue BEFORE this call.
pub unsafe fn request_migration(tcb: NonNull<ThreadControlBlock>, target: CpuId) {
    let target_idx = target.0 as usize;
    if target_idx >= nr_cpus() {
        return;
    }

    // MIGRATED flag: encoded in sched_state if necessary — no-op for stats alone.

    if !MIGRATION_QUEUES[target_idx].push(tcb) {
        // Queue full. The thread was ALREADY dequeued from source run queue
        // by cfs_dequeue_for_migration(). Without re-enqueuing, it disappears
        // permanently (original comment was mistaken).
        // BUG-FIX J: re-enqueue on home CPU to avoid losing the thread.
        MIGRATIONS_DROPPED.fetch_add(1, Ordering::Relaxed);
        let home_raw = tcb.as_ref().cpu_id.load(Ordering::Acquire) as usize;
        if home_raw < nr_cpus() {
            // SAFETY: home_raw < nr_cpus() ≤ MAX_CPUS, run queue initialized.
            let home_cpu = CpuId(home_raw as u32);
            crate::scheduler::core::runqueue::run_queue(home_cpu).enqueue(tcb);
        }
        return;
    }
    MIGRATIONS_SENT.fetch_add(1, Ordering::Relaxed);
    arch_send_reschedule_ipi(target.0);
}

/// Processes pending migrations for current CPU.
///
/// `drain_cb` is called for each received TCB.
///
/// # Safety
/// CPL 0, `cpu` must be local CPU, and `drain_cb` must be safe to invoke for each TCB.
pub unsafe fn drain_pending_migrations(
    cpu: CpuId,
    drain_cb: unsafe fn(tcb: NonNull<ThreadControlBlock>),
) {
    let q = &MIGRATION_QUEUES[cpu.0 as usize];
    while let Some(tcb) = q.pop() {
        // Update current CPU in TCB.
        let tcb_mut = &mut *(tcb.as_ptr());
        tcb_mut.cpu_id.store(cpu.0 as u64, Ordering::Release);
        MIGRATIONS_RECEIVED.fetch_add(1, Ordering::Relaxed);
        drain_cb(tcb);
    }
}
