// kernel/src/scheduler/sync/wait_queue.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Wait Queue — thread wait queue (Exo-OS · Layer 1)
// ═══════════════════════════════════════════════════════════════════════════════
//
// RULE WAITQ-01: WaitNodes are allocated EXCLUSIVELY from EmergencyPool
//   (never from heap allocator — risk of deadlock during reclamation).
//
// Operation:
//   • A thread needing to wait calls `wait_queue_wait()` with a WaitNode
//     pre-allocated from EmergencyPool.
//   • It is inserted into WaitQueue list, its state transitions to BLOCKED.
//   • `wait_queue_wake_one()` / `wait_queue_wake_all()` wake threads up.
//   • The WaitNode is freed back to EmergencyPool AFTER wakeup.
// ═══════════════════════════════════════════════════════════════════════════════

use super::spinlock::SpinLock;
use crate::scheduler::core::runqueue::run_queue;
use crate::scheduler::core::task::CpuId;
use crate::scheduler::core::task::{TaskState, ThreadControlBlock};
use core::ptr::NonNull;
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// EmergencyPool FFI (Layer 0 — memory/)
// ─────────────────────────────────────────────────────────────────────────────

extern "C" {
    /// Allocates a WaitNode from EmergencyPool. Returns null if exhausted.
    fn emergency_pool_alloc_wait_node() -> *mut WaitNode;
    /// Frees a WaitNode back to EmergencyPool.
    fn emergency_pool_free_wait_node(node: *mut WaitNode);
    /// Publishes/reinitializes scheduler block pool during boot.
    fn emergency_pool_init_wait_node_pool();
}

// ─────────────────────────────────────────────────────────────────────────────
// WaitNode — node of an intrusive linked list
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
pub struct WaitNode {
    pub tcb: *mut ThreadControlBlock,
    pub next: *mut WaitNode,
    pub prev: *mut WaitNode,
    /// Flags: bit 0 = EXCLUSIVE (wake single thread).
    pub flags: u32,
    _pad: u32,
}

const _: () = assert!(
    core::mem::size_of::<WaitNode>() <= 64,
    "scheduler::WaitNode doit tenir dans un bloc SchedNodePool de 64 bytes"
);
const _: () = assert!(
    core::mem::align_of::<WaitNode>() <= 64,
    "scheduler::WaitNode requiert un alignement incompatible avec SchedNodePool"
);

impl WaitNode {
    pub const EXCLUSIVE: u32 = 1 << 0;

    /// Allocates a node from EmergencyPool.
    ///
    /// # Safety
    /// RULE WAITQ-01: MANDATORY call through this function — never `Box::new`.
    pub unsafe fn alloc(tcb: *mut ThreadControlBlock, flags: u32) -> Option<NonNull<WaitNode>> {
        let ptr = emergency_pool_alloc_wait_node();
        let node = NonNull::new(ptr)?;
        let n = &mut *node.as_ptr();
        n.tcb = tcb;
        n.next = core::ptr::null_mut();
        n.prev = core::ptr::null_mut();
        n.flags = flags;
        Some(node)
    }

    /// Frees node back to EmergencyPool.
    ///
    /// # Safety
    /// Node must no longer be in any list.
    pub unsafe fn free(node: NonNull<WaitNode>) {
        emergency_pool_free_wait_node(node.as_ptr());
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// WaitQueue — wait list protected by spinlock
// ─────────────────────────────────────────────────────────────────────────────

use core::cell::UnsafeCell;

/// Mutable data of wait queue (always accessed under `lock`).
struct WaitQueueData {
    head: *mut WaitNode,
    count: usize,
}

pub struct WaitQueue {
    data: UnsafeCell<WaitQueueData>,
    lock: SpinLock<()>,
}

// SAFETY: WaitQueue is protected by a SpinLock. Mutations occur via
// UnsafeCell::get() under lock protection. Safe Sync/Send.
unsafe impl Send for WaitQueue {}
unsafe impl Sync for WaitQueue {}

impl Default for WaitQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl WaitQueue {
    pub const fn new() -> Self {
        Self {
            data: UnsafeCell::new(WaitQueueData {
                head: core::ptr::null_mut(),
                count: 0,
            }),
            lock: SpinLock::new(()),
        }
    }

    /// Inserts a node at end of list (FIFO).
    ///
    /// # Safety
    /// `node` must be valid and not currently in any list.
    pub unsafe fn insert(&self, node: NonNull<WaitNode>) {
        let _g = self.lock.lock();
        let d = &mut *self.data.get();
        let n = node.as_ptr();
        (*n).next = core::ptr::null_mut();
        (*n).prev = core::ptr::null_mut();

        if d.head.is_null() {
            d.head = n;
        } else {
            let mut cur = d.head;
            while !(*cur).next.is_null() {
                cur = (*cur).next;
            }
            (*cur).next = n;
            (*n).prev = cur;
        }
        d.count += 1;
    }

    /// Removes a node from list.
    ///
    /// # Safety
    /// `node` must be in this list.
    pub unsafe fn remove(&self, node: NonNull<WaitNode>) {
        let _g = self.lock.lock();
        let d = &mut *self.data.get();
        let n = node.as_ptr();
        if !(*n).prev.is_null() {
            (*(*n).prev).next = (*n).next;
        } else {
            d.head = (*n).next;
        }
        if !(*n).next.is_null() {
            (*(*n).next).prev = (*n).prev;
        }
        (*n).next = core::ptr::null_mut();
        (*n).prev = core::ptr::null_mut();
        if d.count > 0 {
            d.count -= 1;
        }
    }

    /// Wakes up first waiting thread.
    /// Thread-safe — acquires internal lock.
    pub fn notify_one(&self) -> bool {
        let _g = self.lock.lock();
        // SAFETY: under lock, exclusive access to data.
        unsafe {
            let d = &mut *self.data.get();
            if d.head.is_null() {
                return false;
            }
            let node = d.head;
            d.head = (*node).next;
            if !d.head.is_null() {
                (*d.head).prev = core::ptr::null_mut();
            }
            if d.count > 0 {
                d.count -= 1;
            }

            let tcb = (*node).tcb;
            if !tcb.is_null() {
                // BUG-FIX K: verify CAS transition succeeds.
                // If thread is not in Sleeping state (e.g. already Runnable
                // due to concurrent timeout or signal), DO NOT enqueue
                // into run queue: it might already be there → double-scheduling.
                let transitioned = (*tcb).try_transition(TaskState::Sleeping, TaskState::Runnable);
                if transitioned {
                    // BUG-FIX L: validate cpu_id bounds before run_queue().
                    // In release mode, debug_assert is no-op → UB if cpu out of bounds.
                    let cpu_raw = (*tcb).cpu_id.load(Ordering::Relaxed) as usize;
                    if cpu_raw < crate::scheduler::core::preempt::MAX_CPUS {
                        let _irq = crate::scheduler::core::preempt::IrqGuard::new();
                        let cpu_id = CpuId(cpu_raw as u32);
                        let rq = run_queue(cpu_id);
                        rq.enqueue(NonNull::new_unchecked(tcb));
                    }
                }
            }
            emergency_pool_free_wait_node(node);
            WAITQ_WAKEUPS.fetch_add(1, Ordering::Relaxed);
            true
        }
    }

    /// Wakes up ALL waiting threads.
    /// Thread-safe — acquires internal lock.
    pub fn notify_all(&self) -> usize {
        let mut woken = 0usize;
        while self.notify_one() {
            woken += 1;
        }
        woken
    }

    pub fn is_empty(&self) -> bool {
        let _g = self.lock.lock();
        // SAFETY: under lock.
        unsafe { (*self.data.get()).head.is_null() }
    }

    pub fn count(&self) -> usize {
        let _g = self.lock.lock();
        // SAFETY: under lock.
        unsafe { (*self.data.get()).count }
    }

    // ─────────────────────────────────────────────────────────────────────
    // Upstream compatibility — &mut self wrappers delegating to &self
    // ─────────────────────────────────────────────────────────────────────

    /// Alias of `notify_one` for compatibility.
    ///
    /// # Safety
    /// Called in valid context with thread-safe access.
    #[inline]
    pub unsafe fn wake_one(&self) -> bool {
        self.notify_one()
    }

    /// Alias of `notify_all` for compatibility.
    ///
    /// # Safety
    /// Called in valid context with thread-safe access.
    #[inline]
    pub unsafe fn wake_all(&self) -> usize {
        self.notify_all()
    }

    // ─────────────────────────────────────────────────────────────────────
    // Interruptible blocking
    // ─────────────────────────────────────────────────────────────────────

    /// Blocks current thread until next `notify_one`/`notify_all`.
    /// Returns `true` if woken, `false` if interrupted by signal (-EINTR).
    ///
    /// RULE WAITQ-01: WaitNode allocated from EmergencyPool.
    ///
    /// # Safety
    /// `tcb` must point to TCB of current thread.
    /// Called with thread in a consistent state (Running).
    pub unsafe fn wait_interruptible(&self, tcb: *mut ThreadControlBlock) -> bool {
        // 1. Allocate a WaitNode from EmergencyPool.
        let node = match WaitNode::alloc(tcb, 0) {
            Some(n) => n,
            None => return false, // EmergencyPool exhausted
        };

        // 2. Set state to Sleeping BEFORE inserting (avoids missed wakeup).
        (*tcb).set_state(TaskState::Sleeping);

        // 3. Insert into wait queue.
        self.insert(node);

        // 4. Signal check (AFTER insertion — missed wakeup window avoided).
        if (*tcb).has_signal_pending() {
            self.remove(node);
            WaitNode::free(node);
            (*tcb).set_state(TaskState::Runnable);
            return false; // -EINTR
        }

        // 5. Block — schedule_block only returns when thread is woken.
        // V-38 / PREEMPT-BLOCK: FORBIDDEN to block under active PreemptGuard.
        crate::scheduler::core::preempt::assert_preempt_enabled();
        let cpu_id = CpuId((*tcb).cpu_id.load(Ordering::Relaxed) as u32);
        let rq = run_queue(cpu_id);
        crate::scheduler::core::switch::schedule_block(rq, &mut *tcb);

        // 6. After wakeup: node already freed by waker.
        !(*tcb).has_signal_pending()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Metrics
// ─────────────────────────────────────────────────────────────────────────────

pub static WAITQ_WAKEUPS: AtomicU64 = AtomicU64::new(0);
pub static WAITQ_TIMEOUTS: AtomicU64 = AtomicU64::new(0);

/// Initializes wait_queue subsystem.
///
/// Verifies that EmergencyPool is already initialized.
///
/// # Safety
/// Called from `scheduler::init()`, after `memory::init()`.
pub unsafe fn init() {
    emergency_pool_init_wait_node_pool();
    // Verification: attempt test alloc/free from EmergencyPool.
    let test = emergency_pool_alloc_wait_node();
    if test.is_null() {
        crate::arch::x86_64::terminal::debug_write(b"WaitQueue: EmergencyPool non initialise\n");
        crate::arch::x86_64::halt_cpu();
    }
    emergency_pool_free_wait_node(test);
}
