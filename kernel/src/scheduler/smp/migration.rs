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
use crate::scheduler::core::preempt::IrqGuard;
use crate::scheduler::core::task::{CpuId, ThreadControlBlock};
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Per-CPU migration channel
// ─────────────────────────────────────────────────────────────────────────────

const MIGRATION_QUEUE_DEPTH: usize = 16;

/// Circular migration queue for a target CPU, serialized by an IRQ-safe spinlock
/// so concurrent pushes from multiple load-balancing CPUs cannot corrupt indices.
struct MigrationQueue {
    lock: AtomicBool,
    head: AtomicU32,
    tail: AtomicU32,
    slots: [core::sync::atomic::AtomicPtr<ThreadControlBlock>; MIGRATION_QUEUE_DEPTH],
}

impl MigrationQueue {
    const fn new() -> Self {
        Self {
            lock: AtomicBool::new(false),
            head: AtomicU32::new(0),
            tail: AtomicU32::new(0),
            slots: [const { core::sync::atomic::AtomicPtr::new(core::ptr::null_mut()) }; MIGRATION_QUEUE_DEPTH],
        }
    }

    fn with_lock<R>(&self, f: impl FnOnce() -> R) -> R {
        let _irq = IrqGuard::new();
        while self
            .lock
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        struct Unlock<'a>(&'a AtomicBool);
        impl Drop for Unlock<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _unlock = Unlock(&self.lock);
        f()
    }

    /// Enqueues `tcb` into queue. Returns `false` if full.
    fn push(&self, tcb: NonNull<ThreadControlBlock>) -> bool {
        self.with_lock(|| {
            let tail = self.tail.load(Ordering::Relaxed);
            let next = (tail + 1) % MIGRATION_QUEUE_DEPTH as u32;
            if next == self.head.load(Ordering::Relaxed) {
                return false; // Full.
            }
            self.slots[tail as usize].store(tcb.as_ptr(), Ordering::Release);
            self.tail.store(next, Ordering::Release);
            true
        })
    }

    /// Dequeues next TCB. Returns `None` if empty.
    fn pop(&self) -> Option<NonNull<ThreadControlBlock>> {
        self.with_lock(|| {
            let head = self.head.load(Ordering::Relaxed);
            if head == self.tail.load(Ordering::Relaxed) {
                return None; // Empty.
            }
            let ptr = self.slots[head as usize].swap(core::ptr::null_mut(), Ordering::AcqRel);
            let next = (head + 1) % MIGRATION_QUEUE_DEPTH as u32;
            self.head.store(next, Ordering::Release);
            NonNull::new(ptr)
        })
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

/// Re-enqueues `tcb` back onto its home CPU run queue when target queue is unavailable.
unsafe fn re_enqueue_home(tcb: NonNull<ThreadControlBlock>) {
    let home_raw = tcb.as_ref().cpu_id.load(Ordering::Acquire) as usize;
    if home_raw < nr_cpus() {
        let home_cpu = CpuId(home_raw as u32);
        crate::scheduler::core::runqueue::run_queue(home_cpu).enqueue(tcb);
    }
}

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
        MIGRATIONS_DROPPED.fetch_add(1, Ordering::Relaxed);
        re_enqueue_home(tcb);
        return;
    }

    if !MIGRATION_QUEUES[target_idx].push(tcb) {
        // Queue full: re-enqueue on home CPU to avoid losing the already-dequeued thread.
        MIGRATIONS_DROPPED.fetch_add(1, Ordering::Relaxed);
        re_enqueue_home(tcb);
        return;
    }
    MIGRATIONS_SENT.fetch_add(1, Ordering::Relaxed);
    arch_send_reschedule_ipi(target.0);
}

/// Pops one pending migrated TCB targeting `cpu`, updating its `cpu_id`.
///
/// # Safety
/// CPL 0, `cpu` must be the local CPU.
pub(crate) unsafe fn pop_pending_migration(cpu: CpuId) -> Option<NonNull<ThreadControlBlock>> {
    let idx = cpu.0 as usize;
    if idx >= nr_cpus() {
        return None;
    }
    let tcb = MIGRATION_QUEUES[idx].pop()?;
    let tcb_mut = &mut *tcb.as_ptr();
    tcb_mut.cpu_id.store(cpu.0 as u64, Ordering::Release);
    MIGRATIONS_RECEIVED.fetch_add(1, Ordering::Relaxed);
    Some(tcb)
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
    while let Some(tcb) = pop_pending_migration(cpu) {
        drain_cb(tcb);
    }
}
