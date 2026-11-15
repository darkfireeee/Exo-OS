// kernel/src/scheduler/core/runqueue.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Per-CPU RUN QUEUE — 3 queues (RT / Normal / Idle) (Exo-OS Scheduler · Layer 1)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Architecture:
//   • RT queue:     100-bit bitmap (levels 0–99), O(1) via bit-scan
//   • Normal queue: CFS min-heap (red-black tree simulated by in-place sorted
//                   array to avoid heap alloc) — 512 threads max per-CPU
//   • Idle queue:   single thread (the per-CPU idle_thread)
//
// RULES:
//   • NO-ALLOC: all storage is static (arrays) + indices into a
//     global TCB pool
//   • Run queue is protected by an IRQ-safe spinlock (IrqGuard)
//   • MAX_TASKS_PER_CPU = 512 -> sufficient for any realistic workload
//   • Instrumentation: atomic counters for pick_next latency tracking
//
// LOCK ORDERING (regle_bonus.md): Memory locks -> Scheduler locks
// ═══════════════════════════════════════════════════════════════════════════════

use super::preempt::{IrqGuard, MAX_CPUS};
use super::task::{CpuId, SchedPolicy, TaskState, ThreadControlBlock};
use core::mem::MaybeUninit;
use core::ops::{Deref, DerefMut};
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum number of threads per CFS queue per-CPU.
pub const MAX_TASKS_PER_CPU: usize = 512;
/// Number of RT levels (priorities 0–99).
pub const RT_LEVELS: usize = 100;
/// RT Round-Robin timeslice (ms).
pub const RR_TIMESLICE_MS: u64 = 10;
/// Minimal CFS granularity (µs).
pub const CFS_MIN_GRANULARITY_US: u64 = 750;
/// Target CFS latency (ms).
pub const CFS_TARGET_LATENCY_MS: u64 = 6;

// ─────────────────────────────────────────────────────────────────────────────
// RtBitmap — O(1) find_first_set for 100 RT levels
// ─────────────────────────────────────────────────────────────────────────────

/// 128-bit bitmap for 100 RT priority levels.
/// Bit N = 1 -> at least one thread of priority N is ready.
#[derive(Default)]
#[repr(C)]
struct RtBitmap {
    bits: [u64; 2], // 128 bits -> covers 100 levels with margin
}

impl RtBitmap {
    #[inline(always)]
    fn set(&mut self, prio: u8) {
        debug_assert!((prio as usize) < RT_LEVELS);
        let word = prio as usize / 64;
        let bit = prio as usize % 64;
        self.bits[word] |= 1u64 << bit;
    }

    #[inline(always)]
    fn clear(&mut self, prio: u8) {
        let word = prio as usize / 64;
        let bit = prio as usize % 64;
        self.bits[word] &= !(1u64 << bit);
    }

    /// Returns highest priority (lowest value) ready.
    /// O(1) via leading/trailing zeros instruction.
    #[inline(always)]
    fn find_highest_prio(&self) -> Option<u8> {
        if self.bits[0] != 0 {
            Some(self.bits[0].trailing_zeros() as u8)
        } else if self.bits[1] != 0 {
            Some(64 + self.bits[1].trailing_zeros() as u8)
        } else {
            None
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RT Run Queue — per priority level, doubly linked (static indices)
// ─────────────────────────────────────────────────────────────────────────────

/// A slot in the RT queue (stores a TCB pointer + doubly-linked link).
struct RtEntry {
    tcb: Option<NonNull<ThreadControlBlock>>,
    next: u16, // next index in array (0xFFFF = end)
    prev: u16, // previous index
}

const RT_QUEUE_CAPACITY: usize = 256;
const RT_QUEUE_NONE: u16 = 0xFFFF;

/// Complete RT queue: 100 heads + entry pool.
struct RtRunQueue {
    /// Head of each level (index in `entries`, RT_QUEUE_NONE if empty).
    heads: [u16; RT_LEVELS],
    /// Doubly-linked entry pool.
    entries: [RtEntry; RT_QUEUE_CAPACITY],
    /// Next free slot in entries.
    free_head: u16,
    /// Bitmap of non-empty levels.
    bitmap: RtBitmap,
    /// Total number of ready RT threads.
    count: u32,
}

impl RtRunQueue {
    fn new() -> Self {
        const ENTRY: RtEntry = RtEntry {
            tcb: None,
            next: RT_QUEUE_NONE,
            prev: RT_QUEUE_NONE,
        };
        let mut entries = [ENTRY; RT_QUEUE_CAPACITY];
        // Initialize free list: 0 -> 1 -> 2 -> ... -> CAPACITY-1 -> NONE
        for i in 0..(RT_QUEUE_CAPACITY - 1) {
            entries[i].next = (i + 1) as u16;
        }
        entries[RT_QUEUE_CAPACITY - 1].next = RT_QUEUE_NONE;
        Self {
            heads: [RT_QUEUE_NONE; RT_LEVELS],
            entries,
            free_head: 0,
            bitmap: RtBitmap::default(),
            count: 0,
        }
    }

    /// Enqueues an RT thread at the end of its priority queue (FIFO within level).
    fn enqueue(&mut self, tcb: NonNull<ThreadControlBlock>) -> bool {
        // SAFETY: tcb is a valid NonNull<ThreadControlBlock> supplied by
        // pick_next / enqueue — run queue is the sole owner.
        // Preemption is disabled (PerCpuRunQueue invariant).
        let raw_prio = unsafe { tcb.as_ref() }.priority.0 as usize;
        let prio = raw_prio.min(RT_LEVELS - 1);
        debug_assert!(raw_prio < RT_LEVELS, "RT priority >= RT_LEVELS clamped");

        let slot = self.alloc_slot();
        let Some(slot_idx) = slot else {
            return false;
        };

        self.entries[slot_idx].tcb = Some(tcb);
        self.entries[slot_idx].next = RT_QUEUE_NONE;

        // Add to end of level list (FIFO = Round-Robin fairness).
        let head = self.heads[prio];
        if head == RT_QUEUE_NONE {
            self.entries[slot_idx].prev = RT_QUEUE_NONE;
            self.heads[prio] = slot_idx as u16;
        } else {
            // Find tail.
            let mut cur = head;
            loop {
                let nxt = self.entries[cur as usize].next;
                if nxt == RT_QUEUE_NONE {
                    break;
                }
                cur = nxt;
            }
            self.entries[cur as usize].next = slot_idx as u16;
            self.entries[slot_idx].prev = cur;
        }

        self.bitmap.set(prio as u8);
        self.count += 1;
        true
    }

    /// Dequeues and returns highest priority thread (O(1) via bitmap).
    fn dequeue_highest(&mut self) -> Option<NonNull<ThreadControlBlock>> {
        let prio = self.bitmap.find_highest_prio()? as usize;
        let slot_idx = self.heads[prio] as usize;
        let tcb = self.entries[slot_idx].tcb.take()?;

        // Advance list head.
        let next = self.entries[slot_idx].next;
        self.heads[prio] = next;
        if next != RT_QUEUE_NONE {
            self.entries[next as usize].prev = RT_QUEUE_NONE;
        } else {
            // Empty level -> clear bit.
            self.bitmap.clear(prio as u8);
        }

        self.free_slot(slot_idx);
        self.count -= 1;
        Some(tcb)
    }

    fn alloc_slot(&mut self) -> Option<usize> {
        if self.free_head == RT_QUEUE_NONE {
            return None;
        }
        let idx = self.free_head as usize;
        self.free_head = self.entries[idx].next;
        self.entries[idx].next = RT_QUEUE_NONE;
        self.entries[idx].prev = RT_QUEUE_NONE;
        Some(idx)
    }

    fn free_slot(&mut self, idx: usize) {
        self.entries[idx].tcb = None;
        self.entries[idx].next = self.free_head;
        self.free_head = idx as u16;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CFS Run Queue — array sorted by vruntime (O(n) insertion, O(1) pick)
//
// To avoid heap allocation, a static array of TCB pointers sorted by
// ascending vruntime is used (index 0 = smallest vruntime).
// O(n) insertion is acceptable because pick_next_task is O(1) (priority).
// - MAX_TASKS_PER_CPU = 512 (realistic per-CPU limit on Exo-OS)
// ─────────────────────────────────────────────────────────────────────────────

struct CfsRunQueue {
    /// Array of TCB pointers sorted by ascending vruntime.
    tasks: [Option<NonNull<ThreadControlBlock>>; MAX_TASKS_PER_CPU],
    /// Number of ready CFS threads.
    count: usize,
    /// Weight sum for timeslice calculation.
    weight_sum: u64,
    /// Current minimum vruntime (CFS base value).
    min_vruntime: AtomicU64,
}

impl CfsRunQueue {
    fn new() -> Self {
        Self {
            tasks: [None; MAX_TASKS_PER_CPU],
            count: 0,
            weight_sum: 0,
            min_vruntime: AtomicU64::new(0),
        }
    }

    /// Enqueues a CFS thread maintaining vruntime sorting.
    /// O(log n) bisection insertion + O(n) shift in worst case.
    fn enqueue(&mut self, tcb: NonNull<ThreadControlBlock>) -> bool {
        if self.count >= MAX_TASKS_PER_CPU {
            // Queue full: thread will be retried on next tick.
            // Practically impossible with 512 per-CPU slots.
            return false;
        }

        // SAFETY: tcb is a valid NonNull — same invariant as enqueue().
        let tcb_ref = unsafe { tcb.as_ref() };
        let weight = tcb_ref.priority.cfs_weight();
        let slice_ns = self.timeslice_ns(weight);
        crate::scheduler::policies::cfs::normalize_vruntime_on_enqueue(
            tcb_ref,
            self.min_vruntime.load(Ordering::Relaxed),
            slice_ns,
        );
        let vr = tcb_ref.vruntime.load(Ordering::Acquire);

        // Bisection to find insertion position.
        let pos = {
            let mut lo = 0usize;
            let mut hi = self.count;
            while lo < hi {
                let mid = (lo + hi) / 2;
                // SAFETY: tasks[mid] is Some because mid < self.count.
                let mv = unsafe {
                    self.tasks[mid]
                        .unwrap_unchecked()
                        .as_ref()
                        .vruntime
                        .load(Ordering::Acquire)
                };
                if mv <= vr {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            lo
        };

        // Shift elements after `pos` to the right.
        if pos < self.count {
            self.tasks.copy_within(pos..self.count, pos + 1);
        }
        self.tasks[pos] = Some(tcb);
        self.count += 1;
        self.weight_sum = self.weight_sum.saturating_add(weight as u64);
        true
    }

    /// Dequeues thread with smallest vruntime (head of sorted array).
    fn dequeue_min(&mut self) -> Option<NonNull<ThreadControlBlock>> {
        if self.count == 0 {
            return None;
        }
        let tcb = self.tasks[0].take()?;
        self.tasks.copy_within(1..self.count, 0);
        self.tasks[self.count - 1] = None;
        self.count -= 1;

        // SAFETY: tcb is a valid NonNull — same invariant as enqueue().
        let weight = unsafe { tcb.as_ref() }.priority.cfs_weight() as u64;
        self.weight_sum = self.weight_sum.saturating_sub(weight);
        unsafe { tcb.as_ref() }.clear_queued();

        // Update min_vruntime monotonically so newly normalized or woken threads
        // never drag the CFS virtual clock backward.
        if self.count > 0 {
            // SAFETY: tasks[0] is Some because self.count > 0.
            let new_min = unsafe {
                self.tasks[0]
                    .unwrap_unchecked()
                    .as_ref()
                    .vruntime
                    .load(Ordering::Relaxed)
            };
            self.min_vruntime.fetch_max(new_min, Ordering::Release);
        }

        Some(tcb)
    }

    /// Removes a specific thread (wakeup / migration).
    fn remove(&mut self, target: NonNull<ThreadControlBlock>) -> bool {
        for i in 0..self.count {
            if self.tasks[i] == Some(target) {
                // SAFETY: target is a valid NonNull — same invariant as enqueue().
                let weight = unsafe { target.as_ref() }.priority.cfs_weight() as u64;
                self.tasks.copy_within(i + 1..self.count, i);
                self.tasks[self.count - 1] = None;
                self.count -= 1;
                self.weight_sum = self.weight_sum.saturating_sub(weight);
                return true;
            }
        }
        false
    }

    /// Calculates timeslice allocated to a given thread (CFS targeting).
    fn timeslice_ns(&self, weight: u32) -> u64 {
        if self.count == 0 || self.weight_sum == 0 {
            return CFS_TARGET_LATENCY_MS * 1_000_000 / (self.count.max(1) as u64);
        }
        let target_ns = CFS_TARGET_LATENCY_MS * 1_000_000;
        let slice = target_ns * weight as u64 / self.weight_sum;
        slice.max(CFS_MIN_GRANULARITY_US * 1000)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PerCpuRunQueue — aggregation of 3 queues for 1 CPU
// ─────────────────────────────────────────────────────────────────────────────

/// Run queue statistics (instrumentation).
#[repr(C)]
pub struct RunQueueStats {
    /// Total calls to pick_next_task().
    pub picks_total: AtomicU64,
    /// pick_next calls returning RT.
    pub picks_rt: AtomicU64,
    /// pick_next calls returning CFS.
    pub picks_cfs: AtomicU64,
    /// pick_next calls returning Deadline.
    pub picks_dl: AtomicU64,
    /// pick_next calls returning idle.
    pub picks_idle: AtomicU64,
    /// Number of runnable threads (snapshot).
    pub nr_running: AtomicU32,
    /// Normalized load * 1024 (load average).
    pub load_avg: AtomicU64,
    /// Timestamp of last SMP rebalancing (ns).
    pub last_balance_ns: AtomicU64,
}

impl RunQueueStats {
    const fn new() -> Self {
        Self {
            picks_total: AtomicU64::new(0),
            picks_rt: AtomicU64::new(0),
            picks_cfs: AtomicU64::new(0),
            picks_dl: AtomicU64::new(0),
            picks_idle: AtomicU64::new(0),
            nr_running: AtomicU32::new(0),
            load_avg: AtomicU64::new(0),
            last_balance_ns: AtomicU64::new(0),
        }
    }
}

/// Complete run queue for one logical CPU.
///
/// FIX-RQ-ALIGN-01: #[repr(C, align(64))] guarantees that two adjacent
/// PerCpuRunQueue entries in PER_CPU_RQ[MAX_CPUS] do not share cache lines on
/// their hot fields (stats, cpu, current) -> no SMP false sharing.
/// `stats` MUST stay at offset 0 so `yield_fast_path` in `fast_path.s` reads `stats.nr_running`.
#[repr(C, align(64))]
pub struct PerCpuRunQueue {
    /// Statistics (offset 0 — hardcoded in `fast_path.s`).
    pub stats: RunQueueStats,
    /// Owning CPU ID.
    pub cpu: CpuId,
    /// Real-time queue.
    rt: RtRunQueue,
    /// CFS queue (fair scheduling).
    cfs: CfsRunQueue,
    /// Idle thread for this CPU (always present, not counted in nr_running).
    pub idle_thread: Option<NonNull<ThreadControlBlock>>,
    /// Current thread (currently running).
    pub current: Option<NonNull<ThreadControlBlock>>,
    /// Clock vruntime — monotonic time used for CFS lag.
    pub clock_task_ns: u64,
}

// FIX-RQ-ALIGN-01 + fast_path.s yield_fast_path offset assertions.
const _: () = assert!(
    core::mem::align_of::<PerCpuRunQueue>() >= 64,
    "FIX-RQ-ALIGN-01: PerCpuRunQueue doit être aligné sur 64 octets minimum"
);
const _: () = assert!(
    core::mem::offset_of!(PerCpuRunQueue, stats)
        + core::mem::offset_of!(RunQueueStats, nr_running)
        == 40,
    "yield_fast_path (fast_path.s) expects stats.nr_running at offset 40"
);

impl PerCpuRunQueue {
    /// Creates a run queue for CPU `cpu`.
    pub fn new(cpu: CpuId) -> Self {
        Self {
            stats: RunQueueStats::new(),
            cpu,
            rt: RtRunQueue::new(),
            cfs: CfsRunQueue::new(),
            idle_thread: None,
            current: None,
            clock_task_ns: 0,
        }
    }

    #[inline(always)]
    fn dec_nr_running(&self) {
        let prev = self.stats.nr_running.fetch_sub(1, Ordering::Relaxed);
        self.update_load_avg(prev.saturating_sub(1) as u64);
    }

    /// Drains pending cross-CPU migrations targeting this CPU into its local run queue.
    ///
    /// # Safety
    /// Caller holds this CPU's run queue lock.
    pub unsafe fn drain_migrations(&mut self) {
        while let Some(tcb) = crate::scheduler::smp::migration::pop_pending_migration(self.cpu) {
            if !self.enqueue(tcb) {
                let state = tcb.as_ref().state();
                debug_assert!(
                    state != TaskState::Runnable,
                    "drain_migrations: failed to enqueue runnable migrated TCB"
                );
            }
        }
    }

    /// Registers this CPU's idle thread (called once at boot).
    pub fn set_idle_thread(&mut self, idle: NonNull<ThreadControlBlock>) {
        // SAFETY: idle is valid, run queue owns it.
        unsafe { idle.as_ref() }
            .sched_state
            .fetch_or(super::task::SCHED_IDLE_BIT, Ordering::Relaxed);
        self.idle_thread = Some(idle);
    }

    /// Adds a thread to run queue according to its policy.
    ///
    /// INVARIANT: called with preemption disabled.
    pub fn enqueue(&mut self, tcb: NonNull<ThreadControlBlock>) -> bool {
        // SAFETY: tcb is a valid NonNull, preemption disabled (invariant).
        let tcb_ref = unsafe { tcb.as_ref() };
        if tcb_ref.state() != TaskState::Runnable {
            return false;
        }
        if !tcb_ref.try_mark_queued() {
            return false;
        }

        let policy = tcb_ref.policy;
        let enqueued = match policy {
            SchedPolicy::Fifo | SchedPolicy::RoundRobin => self.rt.enqueue(tcb),
            SchedPolicy::Normal | SchedPolicy::Batch => self.cfs.enqueue(tcb),
            SchedPolicy::Deadline => {
                // SCHED_DEADLINE -> dedicated EDF queue (earliest deadline first).
                // SAFETY: preemption disabled (INVARIANT), cpu.0 < MAX_CPUS guaranteed.
                unsafe {
                    crate::scheduler::timer::deadline_timer::dl_enqueue(self.cpu.0 as usize, tcb)
                }
            }
            SchedPolicy::Idle => false, /* handled by idle_thread */
        };
        if !enqueued {
            tcb_ref.clear_queued();
            debug_assert!(
                policy == SchedPolicy::Idle,
                "RunQueue::enqueue: file pleine ou échec d'enfilage pour {:?}",
                policy
            );
            return false;
        }
        let prev = self.stats.nr_running.fetch_add(1, Ordering::Relaxed);
        self.update_load_avg(prev as u64 + 1);
        true
    }

    /// Removes a specific thread from queue (migration, death signal).
    pub fn remove(&mut self, tcb: NonNull<ThreadControlBlock>) -> bool {
        // SAFETY: tcb is a valid NonNull, preemption disabled (invariant).
        let policy = unsafe { tcb.as_ref() }.policy;
        let removed = match policy {
            SchedPolicy::Fifo | SchedPolicy::RoundRobin => {
                // Search in RT slots — O(RT_QUEUE_CAPACITY)
                // SAFETY: same invariant as for policy.
                let prio = (unsafe { tcb.as_ref() }.priority.0 as usize).min(RT_LEVELS - 1);
                let head = self.rt.heads[prio];
                if head == RT_QUEUE_NONE {
                    return false;
                }
                let mut cur = head;
                loop {
                    let entry_tcb = self.rt.entries[cur as usize].tcb;
                    if entry_tcb == Some(tcb) {
                        let prev_entry = self.rt.entries[cur as usize].prev;
                        let next_entry = self.rt.entries[cur as usize].next;
                        if prev_entry == RT_QUEUE_NONE {
                            self.rt.heads[prio] = next_entry;
                        } else {
                            self.rt.entries[prev_entry as usize].next = next_entry;
                        }
                        if next_entry != RT_QUEUE_NONE {
                            self.rt.entries[next_entry as usize].prev = prev_entry;
                        }
                        if self.rt.heads[prio] == RT_QUEUE_NONE {
                            self.rt.bitmap.clear(prio as u8);
                        }
                        self.rt.free_slot(cur as usize);
                        self.rt.count -= 1;
                        break true;
                    }
                    cur = self.rt.entries[cur as usize].next;
                    if cur == RT_QUEUE_NONE {
                        break false;
                    }
                }
            }
            SchedPolicy::Deadline => {
                // SCHED_DEADLINE -> remove from dedicated EDF queue.
                // SAFETY: DL_QUEUES initialized, preemption disabled.
                unsafe {
                    crate::scheduler::timer::deadline_timer::dl_remove(self.cpu.0 as usize, tcb)
                }
            }
            _ => self.cfs.remove(tcb),
        };
        if removed {
            unsafe { tcb.as_ref() }.clear_queued();
            self.dec_nr_running();
        }
        removed
    }

    /// Selects next thread to run — O(1) via RT bitmap or CFS head.
    ///
    /// GUARANTEE: 100-150 cycles in non-contended mode (DOC3 target).
    /// INVARIANT: called with preemption disabled.
    pub fn pick_next(&mut self) -> Option<NonNull<ThreadControlBlock>> {
        self.stats.picks_total.fetch_add(1, Ordering::Relaxed);

        // 1. Absolute priority: RT queue.
        if self.rt.count > 0 {
            self.stats.picks_rt.fetch_add(1, Ordering::Relaxed);
            return self.dequeue_highest_rt();
        }

        // 2. DEADLINE queue (EDF — earliest deadline first).
        // SAFETY: DL_QUEUES initialized by scheduler::init() step 8.
        let dl_candidate =
            unsafe { crate::scheduler::timer::deadline_timer::dl_pick_next(self.cpu.0 as usize) };
        if let Some(tcb) = dl_candidate {
            self.stats.picks_dl.fetch_add(1, Ordering::Relaxed);
            self.dec_nr_running();
            return Some(tcb);
        }

        // 3. CFS queue (normal / batch).
        if self.cfs.count > 0 {
            self.stats.picks_cfs.fetch_add(1, Ordering::Relaxed);
            let tcb = self.cfs.dequeue_min()?;
            self.dec_nr_running();
            return Some(tcb);
        }

        // 4. Idle thread (always present — not counted in nr_running).
        self.stats.picks_idle.fetch_add(1, Ordering::Relaxed);
        self.idle_thread
    }

    /// Returns number of running threads (RT + CFS).
    #[inline(always)]
    pub fn nr_running(&self) -> u32 {
        self.stats.nr_running.load(Ordering::Relaxed)
    }

    /// Computes timeslice for current thread (CFS).
    pub fn timeslice_for(&self, tcb: NonNull<ThreadControlBlock>) -> u64 {
        // SAFETY: tcb is a valid NonNull, called with preemption disabled.
        let weight = unsafe { tcb.as_ref() }.priority.cfs_weight();
        self.cfs.timeslice_ns(weight)
    }

    /// Updates exponential load average (EMA, α = 1/8, unit * 1024).
    fn update_load_avg(&self, nr: u64) {
        let old = self.stats.load_avg.load(Ordering::Relaxed);
        let new_avg = (old * 7 + nr * 1024) >> 3;
        self.stats.load_avg.store(new_avg, Ordering::Relaxed);
    }

    /// Advances run queue clock (called by tick).
    #[inline(always)]
    pub fn advance_clock(&mut self, delta_ns: u64) {
        self.clock_task_ns = self.clock_task_ns.wrapping_add(delta_ns);
    }

    /// Returns number of active threads (RT + CFS) as `usize`.
    #[inline(always)]
    pub fn nr_running_usize(&self) -> usize {
        self.stats.nr_running.load(Ordering::Acquire) as usize
    }

    /// Sum of CFS weights of all threads in CFS queue (for timeslice).
    pub fn total_cfs_weight(&self) -> u64 {
        let mut total = 0u64;
        for i in 0..self.cfs.count {
            if let Some(tcb) = self.cfs.tasks[i] {
                // SAFETY: tasks[i] is Some and valid (CfsRunQueue invariant).
                let w = unsafe { tcb.as_ref() }.priority.cfs_weight() as u64;
                total = total.saturating_add(w);
            }
        }
        total
    }

    /// Highest RT priority currently in RT queue (0 = none).
    /// Returns 0 if RT queue is empty.
    pub fn rt_highest_prio(&self) -> u8 {
        if self.rt.count == 0 {
            return 0;
        }
        self.rt.bitmap.find_highest_prio().unwrap_or(0)
    }

    /// Peeks (without dequeuing) 2nd CFS thread sorted by vruntime.
    /// Used by ai_guided::maybe_prefer() for lightweight comparison.
    pub fn cfs_peek_second(&self) -> Option<NonNull<ThreadControlBlock>> {
        if self.cfs.count >= 2 {
            self.cfs.tasks[1]
        } else {
            None
        }
    }

    /// Dequeues a CFS thread migratable to CPU `dst_cpu`.
    /// Searches from the end (highest vruntime = lowest priority = best candidate to migrate).
    /// NEVER moves RT threads.
    pub fn cfs_dequeue_for_migration(
        &mut self,
        dst_cpu: CpuId,
    ) -> Option<NonNull<ThreadControlBlock>> {
        // Search from the end of CFS array (descending order of vruntime),
        // skipping `self.current` so a switching thread is never pulled mid-switch.
        let mut found_idx = None;
        for i in (0..self.cfs.count).rev() {
            if let Some(tcb) = self.cfs.tasks[i] {
                if self.current == Some(tcb) {
                    continue;
                }
                // SAFETY: tasks[i] is Some and valid (CfsRunQueue invariant).
                if unsafe { tcb.as_ref() }.allowed_on(dst_cpu) {
                    found_idx = Some(i);
                    break;
                }
            }
        }
        let idx = found_idx?;
        let tcb = self.cfs.tasks[idx];
        // Remove slot by shifting.
        let mut j = idx;
        while j + 1 < self.cfs.count {
            self.cfs.tasks[j] = self.cfs.tasks[j + 1];
            j += 1;
        }
        self.cfs.tasks[self.cfs.count - 1] = None;
        self.cfs.count -= 1;
        // BUG-FIX C: update weight_sum. Without this fix, CFS timeslice
        // calculation became increasingly erroneous after each migration.
        if let Some(t) = tcb {
            // SAFETY: t is a valid NonNull taken from tasks[idx].
            let weight = unsafe { t.as_ref() }.priority.cfs_weight() as u64;
            self.cfs.weight_sum = self.cfs.weight_sum.saturating_sub(weight);
            unsafe { t.as_ref() }.clear_queued();
        }
        self.dec_nr_running();
        tcb
    }

    /// Returns highest ready RT priority (O(1) via bitmap).
    /// Exposes `RtRunQueue::bitmap` outside module.
    #[inline(always)]
    pub fn rt_bitmap_highest_prio(&self) -> Option<u8> {
        if self.rt.count == 0 {
            return None;
        }
        self.rt.bitmap.find_highest_prio()
    }

    /// Dequeues highest priority RT thread and decrements `nr_running`.
    #[inline(always)]
    pub fn dequeue_highest_rt(&mut self) -> Option<NonNull<ThreadControlBlock>> {
        let tcb = self.rt.dequeue_highest()?;
        unsafe { tcb.as_ref() }.clear_queued();
        self.dec_nr_running();
        Some(tcb)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Global run queue array — 1 per CPU, protected by per-CPU IRQ-safe spinlocks
// ─────────────────────────────────────────────────────────────────────────────

static mut PER_CPU_RQ: [MaybeUninit<PerCpuRunQueue>; MAX_CPUS] = {
    // SAFETY: MaybeUninit::uninit() is valid for uninitialized storage.
    unsafe { MaybeUninit::uninit().assume_init() }
};
#[repr(C, align(64))]
struct RqSpinLock(AtomicBool, [u8; 63]);

static RQ_LOCKS: [RqSpinLock; MAX_CPUS] =
    [const { RqSpinLock(AtomicBool::new(false), [0u8; 63]) }; MAX_CPUS];

static RUNQUEUE_INITIALIZED: AtomicBool = AtomicBool::new(false);
static RUNQUEUE_NR_CPUS: AtomicU32 = AtomicU32::new(0);

#[inline(always)]
fn validate_cpu_rq(cpu: CpuId) -> usize {
    assert!(
        (cpu.0 as usize) < MAX_CPUS,
        "run_queue: cpu {} hors limites (MAX_CPUS={})",
        cpu.0,
        MAX_CPUS,
    );
    assert!(
        RUNQUEUE_INITIALIZED.load(Ordering::Acquire),
        "run_queue: appele avant init_percpu()",
    );
    let configured_cpus = RUNQUEUE_NR_CPUS.load(Ordering::Acquire);
    assert!(
        cpu.0 < configured_cpus,
        "run_queue: cpu {} non initialise (nr_cpus={})",
        cpu.0,
        configured_cpus,
    );
    cpu.0 as usize
}

#[inline(always)]
fn acquire_rq_spinlock(idx: usize) {
    while RQ_LOCKS[idx]
        .0
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
}

#[inline(always)]
fn release_rq_spinlock(idx: usize) {
    RQ_LOCKS[idx].0.store(false, Ordering::Release);
}

/// RAII guard for exclusive IRQ-safe access to a CPU's `PerCpuRunQueue`.
pub struct RunQueueGuard {
    cpu_idx: usize,
    irq: IrqGuard,
}

impl RunQueueGuard {
    /// Releases the run-queue spinlock and preemption depth while keeping IRQs
    /// disabled, returning saved `RFLAGS` for restoration after `context_switch`.
    #[inline(always)]
    pub fn unlock_keep_irqs(self) -> u64 {
        let cpu_idx = self.cpu_idx;
        let irq = unsafe { core::ptr::read(&self.irq) };
        core::mem::forget(self);
        release_rq_spinlock(cpu_idx);
        irq.release_keep_irqs_disabled()
    }
}

impl Deref for RunQueueGuard {
    type Target = PerCpuRunQueue;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        unsafe { PER_CPU_RQ[self.cpu_idx].assume_init_ref() }
    }
}

impl DerefMut for RunQueueGuard {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { PER_CPU_RQ[self.cpu_idx].assume_init_mut() }
    }
}

impl Drop for RunQueueGuard {
    #[inline(always)]
    fn drop(&mut self) {
        release_rq_spinlock(self.cpu_idx);
    }
}

/// Pairwise RAII lock guard acquiring `min(a, b)` then `max(a, b)` (RULE LB-01).
pub struct TwoRunQueuesGuard {
    first_idx: usize,
    second_idx: usize,
    _irq: IrqGuard,
}

impl Drop for TwoRunQueuesGuard {
    fn drop(&mut self) {
        release_rq_spinlock(self.second_idx);
        release_rq_spinlock(self.first_idx);
    }
}

/// Executes with two run queues locked in strict increasing CPU order (`RULE LB-01`).
///
/// # Safety
/// Both CPUs must be initialized and no run queue lock may already be held on this CPU.
pub unsafe fn with_two_run_queues<R>(
    a: CpuId,
    b: CpuId,
    f: impl FnOnce(&mut PerCpuRunQueue, &mut PerCpuRunQueue) -> R,
) -> Option<R> {
    if a == b {
        return None;
    }
    let a_idx = validate_cpu_rq(a);
    let b_idx = validate_cpu_rq(b);
    let (first_idx, second_idx) = if a_idx < b_idx {
        (a_idx, b_idx)
    } else {
        (b_idx, a_idx)
    };
    let irq = IrqGuard::new();
    acquire_rq_spinlock(first_idx);
    acquire_rq_spinlock(second_idx);
    let _guard = TwoRunQueuesGuard {
        first_idx,
        second_idx,
        _irq: irq,
    };
    let ptr_a = PER_CPU_RQ[a_idx].as_mut_ptr();
    let ptr_b = PER_CPU_RQ[b_idx].as_mut_ptr();
    Some(f(&mut *ptr_a, &mut *ptr_b))
}

/// Acquires the IRQ-safe per-CPU spinlock and returns an exclusive guard for `cpu`'s run queue.
///
/// # Safety
/// The run queue MUST have been initialized by `init_percpu` prior to any call.
/// Callers must never hold a `RunQueueGuard` across `context_switch()` (`unlock_keep_irqs` first).
#[inline(always)]
pub unsafe fn run_queue(cpu: CpuId) -> RunQueueGuard {
    let idx = validate_cpu_rq(cpu);
    let irq = IrqGuard::new();
    acquire_rq_spinlock(idx);
    RunQueueGuard { cpu_idx: idx, irq }
}

/// Initializes run queues for all CPUs.
/// Called from `scheduler::init()` — step 2 of sequence.
pub fn init_percpu(nr_cpus: usize) {
    RUNQUEUE_INITIALIZED.store(false, Ordering::Release);
    let configured_cpus = nr_cpus.min(MAX_CPUS);
    // SAFETY: init called once, prior to any user thread.
    unsafe {
        for i in 0..configured_cpus {
            RQ_LOCKS[i].0.store(false, Ordering::Relaxed);
            PER_CPU_RQ[i].write(PerCpuRunQueue::new(CpuId(i as u32)));
        }
    }
    RUNQUEUE_NR_CPUS.store(configured_cpus as u32, Ordering::Release);
    RUNQUEUE_INITIALIZED.store(true, Ordering::Release);
}
