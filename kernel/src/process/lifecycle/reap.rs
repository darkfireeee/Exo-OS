// kernel/src/process/lifecycle/reap.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Reaper kthread — asynchronous cleanup of zombie resources (Layer 1.5)
// ═══════════════════════════════════════════════════════════════════════════════
//
// RULE PROC-07: resource cleanup of a terminated thread/process
// is performed by a dedicated kthread (never inline in exit()).
//
// Reaper queue:
//   • MPSC ring of 4096 entries + dynamic best-effort overflow.
//   • Each entry = (pid, tid) of thread to reap.
//   • The reaper kthread reads the queue, finds ProcessThread via registry,
//     and frees all resources.
// ═══════════════════════════════════════════════════════════════════════════════

use crate::process::core::pcb::ProcessState;
use crate::process::core::pid::{Pid, Tid};
use crate::process::core::registry::PROCESS_REGISTRY;
use crate::process::lifecycle::create::{create_kthread, KthreadParams};
use crate::scheduler::core::runqueue::run_queue;
use crate::scheduler::core::switch::{cooperative_reschedule, current_thread_raw, schedule_block};
use crate::scheduler::core::task::{CpuId, Priority, TaskState};
use crate::scheduler::sync::wait_queue::{WaitNode, WaitQueue};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

// ─────────────────────────────────────────────────────────────────────────────
// ReaperEntry — an entry in the reaper queue
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Default)]
pub(crate) struct ReaperEntry {
    /// Process PID (non-zero if last thread).
    pid: u32,
    /// Thread TID to free.
    tid: u32,
}

// ─────────────────────────────────────────────────────────────────────────────
// ReaperQueue — bounded MPSC ring + dynamic overflow
// ─────────────────────────────────────────────────────────────────────────────

const REAPER_RING_SIZE: usize = 4096;

struct OverflowQueue {
    entries: Vec<ReaperEntry>,
    head: usize,
}

impl OverflowQueue {
    const fn new() -> Self {
        Self {
            entries: Vec::new(),
            head: 0,
        }
    }

    fn push(&mut self, entry: ReaperEntry) -> bool {
        if self.entries.len() == self.entries.capacity() && self.entries.try_reserve(1).is_err() {
            return false;
        }
        self.entries.push(entry);
        true
    }

    fn pop(&mut self) -> Option<ReaperEntry> {
        if self.head >= self.entries.len() {
            return None;
        }
        let entry = self.entries[self.head];
        self.head += 1;

        if self.head >= self.entries.len() {
            self.entries.clear();
            self.head = 0;
        } else if self.head >= 64 && self.head * 2 >= self.entries.len() {
            self.entries.drain(0..self.head);
            self.head = 0;
        }

        Some(entry)
    }

    fn is_empty(&self) -> bool {
        self.head >= self.entries.len()
    }
}

static REAPER_OVERFLOW: Mutex<OverflowQueue> = Mutex::new(OverflowQueue::new());
static REAPER_SLEEP_LOCK: Mutex<()> = Mutex::new(());
static REAPER_WAIT_QUEUE: WaitQueue = WaitQueue::new();

/// Bounded MPSC queue for the reaper.
pub struct ReaperQueue {
    /// Bounded primary ring.
    ring: [core::cell::UnsafeCell<ReaperEntry>; REAPER_RING_SIZE],
    /// Producer position.
    head: AtomicUsize,
    /// Consumer position.
    tail: AtomicUsize,
    /// Serializes multiple producers (`do_exit()` can run concurrently on multiple CPUs).
    producer_lock: Mutex<()>,
    /// Total enqueue counter for debugging purposes.
    total_enqueued: AtomicU64,
    /// Dropped entries due to lack of space even in overflow.
    lost: AtomicU64,
}

// SAFETY: `head` is protected by `producer_lock`, `tail` is atomic, and the
// consumer is unique (reaper kthread).
unsafe impl Sync for ReaperQueue {}

pub static REAPER_QUEUE: ReaperQueue = ReaperQueue::new();

impl Default for ReaperQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl ReaperQueue {
    pub const fn new() -> Self {
        Self {
            ring: [const { core::cell::UnsafeCell::new(ReaperEntry { pid: 0, tid: 0 }) };
                REAPER_RING_SIZE],
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            producer_lock: Mutex::new(()),
            total_enqueued: AtomicU64::new(0),
            lost: AtomicU64::new(0),
        }
    }

    /// Enqueues a thread to be reaped.
    /// Called from do_exit() when a thread transitions to Dead.
    pub fn enqueue(&self, pid: Pid, tid: Tid) {
        let entry = ReaperEntry {
            pid: pid.0,
            tid: tid.0,
        };
        let enqueued = {
            let _producer_guard = self.producer_lock.lock();
            let head = self.head.load(Ordering::Relaxed);
            let next = (head + 1) % REAPER_RING_SIZE;
            let tail = self.tail.load(Ordering::Acquire);
            if next != tail {
                // SAFETY: `head < REAPER_RING_SIZE` guaranteed by modulo.
                unsafe {
                    *self.ring[head].get() = entry;
                }
                self.head.store(next, Ordering::Release);
                true
            } else {
                REAPER_OVERFLOW.lock().push(entry)
            }
        };

        if enqueued {
            self.total_enqueued.fetch_add(1, Ordering::Relaxed);
            let _sleep_guard = REAPER_SLEEP_LOCK.lock();
            // SAFETY: opportunistic wake; no effect if reaper is not asleep.
            unsafe {
                REAPER_WAIT_QUEUE.wake_one();
            }
        } else {
            self.lost.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Dequeues an entry. Returns None if the queue is empty.
    pub(crate) fn dequeue(&self) -> Option<ReaperEntry> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        if tail == head {
            return REAPER_OVERFLOW.lock().pop();
        }
        // SAFETY: tail < REAPER_RING_SIZE.
        let entry = unsafe { core::ptr::read(self.ring[tail].get()) };
        self.tail
            .store((tail + 1) % REAPER_RING_SIZE, Ordering::Release);
        Some(entry)
    }

    /// Statistics.
    pub fn stats(&self) -> (u64, u64) {
        (
            self.total_enqueued.load(Ordering::Relaxed),
            self.lost.load(Ordering::Relaxed),
        )
    }

    fn has_pending(&self) -> bool {
        let tail = self.tail.load(Ordering::Acquire);
        let head = self.head.load(Ordering::Acquire);
        if tail != head {
            return true;
        }
        !REAPER_OVERFLOW.lock().is_empty()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Reaper kthread
// ─────────────────────────────────────────────────────────────────────────────

/// Number of processes/threads reaped since boot.
static REAPED_COUNT: AtomicU64 = AtomicU64::new(0);

fn sleep_until_work() {
    let tcb = current_thread_raw();
    if tcb.is_null() {
        core::hint::spin_loop();
        unsafe {
            cooperative_reschedule();
        }
        return;
    }

    // SAFETY: reaper is a valid scheduler kthread; WaitNode lives until wakeup.
    let Some(node) = (unsafe { WaitNode::alloc(tcb, 0) }) else {
        core::hint::spin_loop();
        unsafe {
            cooperative_reschedule();
        }
        return;
    };

    let sleep_guard = REAPER_SLEEP_LOCK.lock();
    if REAPER_QUEUE.has_pending() {
        // SAFETY: node was never inserted into queue.
        unsafe {
            WaitNode::free(node);
        }
        return;
    }

    // SAFETY: `tcb` points to current thread; it is safe to put it to sleep
    // then insert it into wait queue under the rendezvous lock.
    unsafe {
        (*tcb).set_state(TaskState::Sleeping);
        REAPER_WAIT_QUEUE.insert(node);
    }
    drop(sleep_guard);

    let cpu_id = CpuId(unsafe { (*tcb).cpu_id.load(Ordering::Relaxed) as u32 });
    // SAFETY: `cpu_id` comes from current executing TCB.
    let rq = unsafe { run_queue(cpu_id) };
    // SAFETY: thread is now visible in wait queue, so wakeup
    // will not be lost. `schedule_block` resumes only after wake_one().
    unsafe {
        schedule_block(rq, &mut *tcb);
    }
}

/// Main loop of reaper kthread.
/// NEVER returns.
fn reaper_loop(_arg: usize) -> ! {
    #[cfg(all(target_arch = "x86_64", debug_assertions))]
    unsafe {
        core::arch::asm!("mov al, 0x57", "out 0xe9, al", options(nomem, nostack));
    }

    loop {
        let mut processed = 0usize;
        while let Some(entry) = REAPER_QUEUE.dequeue() {
            reap_entry(entry);
            processed += 1;
            if processed >= 64 {
                unsafe {
                    cooperative_reschedule();
                }
                break;
            }
        }

        if processed >= 64 {
            continue;
        }
        sleep_until_work();
    }
}

/// Frees resources associated with a reaper entry.
fn reap_entry(entry: ReaperEntry) {
    let pid = Pid(entry.pid);
    let tid = Tid(entry.tid);

    // Check if this is the last thread of the process.
    let is_last_thread = PROCESS_REGISTRY
        .find_by_pid(pid)
        .map(|pcb| {
            let last = pcb.thread_count.load(Ordering::Relaxed) == 0 && pcb.state() == ProcessState::Zombie;
            if !last {
                // Secondary thread terminated: free its resources immediately.
                let main_ptr = pcb.main_thread_rawptr.load(Ordering::Acquire);
                for slot in &pcb.thread_slots {
                    let ptr = slot.load(Ordering::Acquire);
                    if !ptr.is_null() && ptr != main_ptr {
                        // SAFETY: ptr points to a valid ProcessThread.
                        let thread_tid = unsafe { (*ptr).tid };
                        if thread_tid == tid
                            && slot
                                .compare_exchange(
                                    ptr,
                                    core::ptr::null_mut(),
                                    Ordering::AcqRel,
                                    Ordering::Acquire,
                                )
                                .is_ok()
                        {
                            // SAFETY: thread allocated by Box::into_raw, state Dead.
                            unsafe {
                                let thread_box = alloc::boxed::Box::from_raw(ptr);
                                drop(thread_box);
                                crate::process::core::pid::TID_ALLOCATOR.free(tid.0);
                            }
                            break;
                        }
                    }
                }
            }
            last
        })
        .unwrap_or(false);

    if is_last_thread {
        // WAIT-04/EXIT-03: a Zombie PCB remains observable until waitpid().
        // The reaper therefore does not remove the final process from registry.
    }

    REAPED_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Launches the reaper kthread. Called once from process::init().
///
/// # Safety
/// Called after `scheduler::init()` from the BSP.
pub fn init_reaper() {
    // SAFETY: bounded E9 trace to accurately pinpoint reaper init at boot.
    unsafe {
        core::arch::asm!("mov al, 0x52", "out 0xe9, al", options(nomem, nostack));
        // 'R'
    }
    let reaper = create_kthread(&KthreadParams {
        name: "reaper",
        entry: reaper_loop,
        arg: 0,
        target_cpu: 0,
        priority: Priority::NORMAL_DEFAULT,
    });
    if reaper.is_err() {
        // Boot must not panic in low memory path. Zombies
        // will remain observable via waitpid(), and counters will signal
        // lack of progress from the reaper.
        unsafe {
            core::arch::asm!("mov al, 0x21", "out 0xe9, al", options(nomem, nostack));
        }
        return;
    }
    // SAFETY: bounded E9 trace to confirm completion of init_reaper.
    unsafe {
        core::arch::asm!("mov al, 0x72", "out 0xe9, al", options(nomem, nostack));
        // 'r'
    }
}

/// Number of processes/threads freed since boot.
pub fn reaped_count() -> u64 {
    REAPED_COUNT.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_uses_overflow_when_ring_saturates() {
        let q = ReaperQueue::new();
        let total = REAPER_RING_SIZE + 8;

        for i in 0..total {
            q.enqueue(Pid((i + 1) as u32), Tid((i + 1) as u32));
        }

        let mut drained = 0usize;
        while q.dequeue().is_some() {
            drained += 1;
        }

        assert_eq!(drained, total);
        assert_eq!(q.stats().1, 0);
    }

    #[test]
    fn queue_reports_pending_when_only_overflow_has_entries() {
        let q = ReaperQueue::new();
        for i in 0..(REAPER_RING_SIZE + 1) {
            q.enqueue(Pid((i + 1) as u32), Tid((i + 1) as u32));
        }

        for _ in 0..(REAPER_RING_SIZE - 1) {
            let _ = q.dequeue();
        }

        assert!(q.has_pending());
        while q.dequeue().is_some() {}
        assert!(!q.has_pending());
    }
}
