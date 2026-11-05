#![allow(clippy::missing_safety_doc)]
// kernel/src/memory/physical/frame/emergency_pool.rs
//
// EmergencyPool — EMERGENCY_POOL_SIZE (>=256) WaitNodes pre-allocated at BOOT.
// Canonical size: memory::core::constants::EMERGENCY_POOL_SIZE = 256.
// RULE EMERGENCY-01: Initialized BEFORE any other kernel module.
//
// Rationale:
//   WaitQueue::wait() can be called from a memory reclaim context.
//   Allocating from the heap during reclaim -> recursive deadlock.
//   This pool resolves this issue by providing pre-allocated nodes in
//   static .bss, never freed, never compacted.
//
// Legitimate users: scheduler/sync/wait_queue.rs ONLY.
// Layer 0 — no external dependencies.

use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use crate::memory::core::constants::EMERGENCY_POOL_SIZE;

// ─────────────────────────────────────────────────────────────────────────────
// WAIT NODE — wait queue node
// ─────────────────────────────────────────────────────────────────────────────

/// WaitNode — structure allocated from EmergencyPool for wait queues.
/// Must stay as small as possible (targets 64 bytes = 1 cache line).
#[repr(C, align(64))]
pub struct WaitNode {
    /// Waiting thread ID (0 = free node).
    pub thread_id: AtomicUsize,
    /// Next node in the linked list (null = end).
    pub next: AtomicUsize, // *mut WaitNode as usize
    /// Wakeup result (0 = signaled cleanly, other = error/timeout).
    pub wakeup_result: AtomicUsize,
    /// Wait entry timestamp (monotonic nanoseconds, debug).
    pub enqueue_ts: AtomicUsize,
    /// Padding to reach exactly 64 bytes.
    _pad: [u8; 64 - 4 * core::mem::size_of::<AtomicUsize>()],
}

const _: () = assert!(
    core::mem::size_of::<WaitNode>() == 64,
    "WaitNode doit faire exactement 64 bytes (1 cache line)"
);
const _: () = assert!(
    core::mem::align_of::<WaitNode>() == 64,
    "WaitNode doit être aligné sur 64 bytes"
);

impl WaitNode {
    /// NULL pointer constant value for the `next` field.
    pub const NULL_PTR: usize = 0;

    /// Creates an empty WaitNode (thread_id = 0 = free).
    #[inline(always)]
    pub const fn new_free() -> Self {
        WaitNode {
            thread_id: AtomicUsize::new(0),
            next: AtomicUsize::new(Self::NULL_PTR),
            wakeup_result: AtomicUsize::new(0),
            enqueue_ts: AtomicUsize::new(0),
            _pad: [0u8; 64 - 4 * core::mem::size_of::<AtomicUsize>()],
        }
    }

    /// Checks whether this node is free (not used by a wait queue).
    #[inline(always)]
    pub fn is_free(&self) -> bool {
        self.thread_id.load(Ordering::Acquire) == 0
    }

    /// Marks node as allocated for thread `tid`.
    /// Returns false if node was already allocated (CAS failure).
    #[inline(always)]
    pub fn try_acquire(&self, tid: usize) -> bool {
        debug_assert_ne!(tid, 0, "thread_id 0 est réservé pour 'libre'");
        self.thread_id
            .compare_exchange(0, tid, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// Frees node (resets thread_id to 0).
    /// SAFETY: Caller must guarantee no wait queue
    ///         references this node anymore.
    #[inline(always)]
    pub unsafe fn release(&self) {
        self.next.store(Self::NULL_PTR, Ordering::Release);
        self.wakeup_result.store(0, Ordering::Release);
        self.enqueue_ts.store(0, Ordering::Release);
        // Release thread_id last so that is_free() is consistent
        self.thread_id.store(0, Ordering::Release);
    }

    /// Signals waiting thread (wakes up, result code = 0 = OK).
    #[inline(always)]
    pub fn signal_ok(&self) {
        self.wakeup_result.store(0, Ordering::Release);
    }

    /// Signals thread with an error code.
    #[inline(always)]
    pub fn signal_err(&self, code: usize) {
        self.wakeup_result.store(code, Ordering::Release);
    }

    /// Returns wakeup result code.
    #[inline(always)]
    pub fn result(&self) -> usize {
        self.wakeup_result.load(Ordering::Acquire)
    }

    /// Returns address of this node as usize (for lock-free linked lists).
    #[inline(always)]
    pub fn as_usize_ptr(&self) -> usize {
        self as *const WaitNode as usize
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// EMERGENCY POOL — static pool of WaitNodes
// ─────────────────────────────────────────────────────────────────────────────

/// Global emergency pool — allocated in .bss, initialized at boot.
///
/// RULE EMERGENCY-01: `init()` MUST be called BEFORE any other module.
/// If a call to `acquire()` occurs before `init()`, it panics in debug
/// and returns None in release (anti-crash).
pub struct EmergencyPool {
    /// Static table of WaitNodes — in .bss, never deallocated.
    nodes: UnsafeCell<[MaybeUninit<WaitNode>; EMERGENCY_POOL_SIZE]>,
    /// Initialization flag.
    initialized: AtomicBool,
    /// Current allocation count (telemetry).
    alloc_count: AtomicUsize,
    /// Peak concurrent allocations (high watermark — telemetry).
    peak_alloc: AtomicUsize,
    /// Allocation failure counter (pool exhausted).
    exhausted_count: AtomicUsize,
}

// SAFETY: EmergencyPool is thread-safe via its internal AtomicBool/AtomicUsize.
// Access to WaitNodes is protected by the CAS protocol of try_acquire/release.
unsafe impl Sync for EmergencyPool {}
unsafe impl Send for EmergencyPool {}

impl EmergencyPool {
    /// Creates an uninitialized instance of the pool.
    /// MUST be placed in a static (UnsafeCell requires Sync).
    pub const fn new_uninit() -> Self {
        EmergencyPool {
            nodes: UnsafeCell::new(
                // SAFETY: MaybeUninit::uninit() is safe in const context.
                [const { MaybeUninit::uninit() }; EMERGENCY_POOL_SIZE],
            ),
            initialized: AtomicBool::new(false),
            alloc_count: AtomicUsize::new(0),
            peak_alloc: AtomicUsize::new(0),
            exhausted_count: AtomicUsize::new(0),
        }
    }

    /// Initializes the pool — MUST be called first at boot.
    ///
    /// # Safety
    /// Must be called once, from a single CPU (before SMP init),
    /// before any use of the pool. Repeated call is a safe no-op.
    pub unsafe fn init(&self) {
        if self.initialized.load(Ordering::Acquire) {
            return; // Already initialized — idempotent
        }

        let nodes_ptr = self.nodes.get();
        // SAFETY: Exclusive access guaranteed by single-CPU initialization protocol.
        // MaybeUninit entries are written once here before any concurrent access.
        for i in 0..EMERGENCY_POOL_SIZE {
            (*nodes_ptr)[i].write(WaitNode::new_free());
        }

        // Publication barrier: all writes to nodes are visible
        // before initialized becomes true.
        self.initialized.store(true, Ordering::Release);
    }

    /// Allocates a free WaitNode from the pool for thread `thread_id`.
    ///
    /// Returns `None` if the pool is exhausted (always O(n) in worst case but
    /// n=64, linear acceptable for non-hot path).
    ///
    /// # Panics
    /// In debug mode, panics if the pool is not initialized.
    pub fn acquire(&self, thread_id: usize) -> Option<&WaitNode> {
        debug_assert!(
            self.initialized.load(Ordering::Acquire),
            "EmergencyPool::acquire() appelé avant init() — RÈGLE EMERGENCY-01 violée"
        );

        if !self.initialized.load(Ordering::Acquire) {
            return None;
        }

        let nodes_ptr = self.nodes.get();
        let nodes: &[MaybeUninit<WaitNode>; EMERGENCY_POOL_SIZE] =
            // SAFETY: After init(), all WaitNodes are initialized (Release barrier).
            unsafe { &*nodes_ptr };

        for node_uninit in nodes.iter() {
            // SAFETY: All nodes were initialized in init().
            let node: &WaitNode = unsafe { node_uninit.assume_init_ref() };
            if node.try_acquire(thread_id) {
                // Atomically update count and peak
                let prev = self.alloc_count.fetch_add(1, Ordering::Relaxed);
                let new_count = prev + 1;
                // Update peak if necessary (best-effort, no exact guarantee)
                let mut peak = self.peak_alloc.load(Ordering::Relaxed);
                while new_count > peak {
                    match self.peak_alloc.compare_exchange_weak(
                        peak,
                        new_count,
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    ) {
                        Ok(_) => break,
                        Err(p) => peak = p,
                    }
                }
                return Some(node);
            }
        }

        // Pool exhausted
        self.exhausted_count.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Releases a previously acquired WaitNode.
    ///
    /// # Safety
    /// The node MUST have been obtained via `acquire()` from this pool.
    /// No wait queue must reference this node at the time of call.
    pub unsafe fn release(&self, node: &WaitNode) {
        debug_assert!(
            self.initialized.load(Ordering::Acquire),
            "EmergencyPool::release() appelé avant init()"
        );
        // Verify that the node belongs to this pool
        debug_assert!(
            self.owns(node),
            "EmergencyPool::release() : nœud ne vient pas de ce pool"
        );

        node.release();
        self.alloc_count.fetch_sub(1, Ordering::Relaxed);
    }

    /// Checks whether a `WaitNode` belongs to this pool.
    #[inline]
    pub fn owns(&self, node: &WaitNode) -> bool {
        let pool_start = self.nodes.get() as usize;
        let pool_end =
            pool_start + core::mem::size_of::<[MaybeUninit<WaitNode>; EMERGENCY_POOL_SIZE]>();
        let node_addr = node as *const WaitNode as usize;
        node_addr >= pool_start && node_addr < pool_end
    }

    /// Returns pool statistics (telemetry).
    #[inline]
    pub fn stats(&self) -> EmergencyPoolStats {
        EmergencyPoolStats {
            capacity: EMERGENCY_POOL_SIZE,
            allocated: self.alloc_count.load(Ordering::Relaxed),
            peak_allocated: self.peak_alloc.load(Ordering::Relaxed),
            exhausted_count: self.exhausted_count.load(Ordering::Relaxed),
            initialized: self.initialized.load(Ordering::Relaxed),
        }
    }

    /// Returns number of currently allocated nodes.
    #[inline(always)]
    pub fn allocated_count(&self) -> usize {
        self.alloc_count.load(Ordering::Relaxed)
    }

    /// Returns `true` if pool is initialized.
    #[inline(always)]
    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Acquire)
    }

    /// Returns `true` if pool is exhausted (no free nodes left).
    #[inline(always)]
    pub fn is_full(&self) -> bool {
        self.alloc_count.load(Ordering::Relaxed) >= EMERGENCY_POOL_SIZE
    }

    /// Counts available free nodes (O(n) — use only for diagnostics).
    pub fn free_count(&self) -> usize {
        if !self.initialized.load(Ordering::Acquire) {
            return 0;
        }
        let nodes_ptr = self.nodes.get();
        // SAFETY: After init(), all nodes are initialized.
        let nodes: &[MaybeUninit<WaitNode>; EMERGENCY_POOL_SIZE] =
            // SAFETY: nodes_ptr valid post-init(); immutable access.
            unsafe { &*nodes_ptr };
        nodes
            .iter()
            .map(|n| unsafe { n.assume_init_ref() })
            .filter(|n| n.is_free())
            .count()
    }
}

/// EmergencyPool statistics for telemetry/debug.
#[derive(Copy, Clone, Debug)]
pub struct EmergencyPoolStats {
    pub capacity: usize,
    pub allocated: usize,
    pub peak_allocated: usize,
    pub exhausted_count: usize,
    pub initialized: bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// GLOBAL INSTANCE
// ─────────────────────────────────────────────────────────────────────────────

/// Global emergency pool — MUST be initialized first at boot.
///
/// RULE EMERGENCY-01: Call `EMERGENCY_POOL.init()` BEFORE any other module.
/// See module comments for anti-deadlock reasoning.
pub static EMERGENCY_POOL: EmergencyPool = EmergencyPool::new_uninit();

/// Initializes the emergency pool AND the SchedNodePool.
/// SAFETY: Same as `EmergencyPool::init()` — see its documentation.
///
/// Must be the FIRST kernel initialization call in `kernel_init()`.
pub unsafe fn init() {
    EMERGENCY_POOL.init();
    // Initialize SchedNodePool (external bitmap -> all blocks free).
    // RULE WAITQ-01: available before any wait queue creation.
    init_sched_pool();
}

/// Allocates a WaitNode from the global pool.
/// Returns `None` if pool is exhausted (critical situation, kernel warning).
#[inline]
pub fn acquire(thread_id: usize) -> Option<&'static WaitNode> {
    EMERGENCY_POOL.acquire(thread_id)
}

/// Releases a WaitNode to the global pool.
/// SAFETY: See `EmergencyPool::release()`.
#[inline]
pub unsafe fn release(node: &'static WaitNode) {
    EMERGENCY_POOL.release(node)
}

/// Returns global pool statistics.
#[inline]
pub fn stats() -> EmergencyPoolStats {
    EMERGENCY_POOL.stats()
}

// ─────────────────────────────────────────────────────────────────────────────
// STATIC CHECKS
// ─────────────────────────────────────────────────────────────────────────────

const _: () = assert!(
    EMERGENCY_POOL_SIZE >= 64,
    "EMERGENCY_POOL_SIZE < 64 — risque de deadlock dans wait_queue"
);
const _: () = assert!(
    core::mem::size_of::<WaitNode>() == 64,
    "WaitNode doit faire 64 bytes"
);
const _: () = assert!(
    core::mem::align_of::<WaitNode>() == 64,
    "WaitNode doit être aligné sur 64 bytes"
);

// ═════════════════════════════════════════════════════════════════════════════
// SCHED NODE POOL — raw blocks pool for scheduler/sync/wait_queue.rs
// ═════════════════════════════════════════════════════════════════════════════
//
// Architectural issue:
//   • EmergencyPool::WaitNode uses (thread_id: AtomicUsize) at byte 0
//     to manage its own free/occupied state via CAS.
//   • scheduler::sync::wait_queue::WaitNode places (tcb: *mut TCB) at byte 0.
//   • These two definitions are INCOMPATIBLE: writing the TCB corrupts
//     the tracking field of EmergencyPool.
//
// Solution:
//   SchedNodePool manages 256 64-byte blocks 64-aligned via an EXTERNAL BITSET
//   ([AtomicU64; 4]) — the pool NEVER touches block contents.
//   The scheduler owns the entirety of the block and writes its own structure into it.
//
// Lock-free O(1) CAS for alloc/free via trailing_zeros on bitset.
// Initialization: all bits at 1 = all blocks free.
//
// RULE WAITQ-01: blocks are pre-allocated in .bss, zero heap allocation.
// ═════════════════════════════════════════════════════════════════════════════

/// Number of blocks in SchedNodePool.
const SCHED_POOL_SIZE: usize = 256;
const SCHED_POOL_WORDS: usize = SCHED_POOL_SIZE / 64;
const _: () = assert!(
    SCHED_POOL_WORDS * 64 == SCHED_POOL_SIZE,
    "SCHED_POOL_SIZE doit être un multiple de 64"
);

/// An opaque 64-byte memory block 64-aligned — used by scheduler
/// to construct its own WaitNode (#[repr(C)]).
#[repr(C, align(64))]
struct RawBlock64 {
    data: [u8; 64],
}

const _: () = assert!(
    core::mem::size_of::<RawBlock64>() == 64,
    "RawBlock64 doit faire exactement 64 bytes"
);
const _: () = assert!(
    core::mem::align_of::<RawBlock64>() == 64,
    "RawBlock64 doit être aligné 64 bytes"
);

/// Raw blocks pool for scheduler/sync/wait_queue.rs.
///
/// Uses a bitmap [AtomicU64; 4] for tracking (bit N=1 <-> block N free).
/// Zero dependencies towards scheduler or scheduler types.
struct SchedNodePool {
    /// 256 aligned 64-byte blocks — in .bss, never deallocated.
    blocks: UnsafeCell<[RawBlock64; SCHED_POOL_SIZE]>,
    /// Free bitset: bit N = 1 -> block N available.
    /// Initialized to u64::MAX for each of the 4 words (all free).
    free_bits: [AtomicU64; SCHED_POOL_WORDS],
    initialized: AtomicBool,
    /// Active allocation counter (diagnostic).
    alloc_count: AtomicUsize,
    /// Counter of times the pool was exhausted (diagnostic).
    exhausted: AtomicUsize,
}

unsafe impl Sync for SchedNodePool {}
unsafe impl Send for SchedNodePool {}

impl SchedNodePool {
    const fn new_uninit() -> Self {
        SchedNodePool {
            // SAFETY: RawBlock64 = [u8; 64], all-zeros valid + no drop.
            blocks: UnsafeCell::new(unsafe { core::mem::zeroed() }),
            free_bits: [const { AtomicU64::new(0) }; SCHED_POOL_WORDS],
            initialized: AtomicBool::new(false),
            alloc_count: AtomicUsize::new(0),
            exhausted: AtomicUsize::new(0),
        }
    }

    /// Initializes the pool (idempotent).
    ///
    /// # Safety
    /// Called from a single CPU before any concurrent access.
    unsafe fn init(&self) {
        // Boot-idempotent: kernel_init() can re-call this init after the
        // early arch path. No scheduler user should hold blocks yet
        // at this stage, so we explicitly republish the entire bitmap.
        for word in &self.free_bits {
            word.store(u64::MAX, Ordering::Release);
        }
        self.alloc_count.store(0, Ordering::Release);
        self.exhausted.store(0, Ordering::Release);
        self.initialized.store(true, Ordering::Release);
    }

    /// Allocates a 64-byte block, returns null if exhausted.
    ///
    /// Lock-free CAS via trailing_zeros on the bitset — O(1) amortized.
    fn alloc(&self) -> *mut u8 {
        if !self.initialized.load(Ordering::Acquire) {
            return core::ptr::null_mut();
        }
        for word_idx in 0..SCHED_POOL_WORDS {
            loop {
                let bits = self.free_bits[word_idx].load(Ordering::Acquire);
                if bits == 0 {
                    break;
                }
                let bit = bits.trailing_zeros() as usize;
                let idx = word_idx * 64 + bit;
                let new_bits = bits & !(1u64 << bit);
                match self.free_bits[word_idx].compare_exchange_weak(
                    bits,
                    new_bits,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => {
                        self.alloc_count.fetch_add(1, Ordering::Relaxed);
                        // SAFETY: idx < SCHED_POOL_SIZE by construction, blocks initialized.
                        let blocks_ptr = self.blocks.get();
                        return unsafe { (*blocks_ptr)[idx].data.as_mut_ptr() };
                    }
                    Err(_) => continue, // retry CAS on this word
                }
            }
        }
        self.exhausted.fetch_add(1, Ordering::Relaxed);
        core::ptr::null_mut()
    }

    /// Releases a block previously allocated via `alloc()`.
    ///
    /// # Safety
    /// `ptr` must be a pointer returned by `alloc()` from this pool.
    unsafe fn free(&self, ptr: *mut u8) {
        if ptr.is_null() {
            return;
        }
        // SAFETY: self.blocks is a static UnsafeCell<[RawBlock64; N]> of the pool;
        // we only read base address of array (no dereferencing of content)
        // for bounds check below. Ring 0 access.
        let blocks_base = unsafe { (*self.blocks.get()).as_ptr() as usize };
        let ptr_addr = ptr as usize;
        // Bounds checking (defensive)
        let pool_size = core::mem::size_of::<[RawBlock64; SCHED_POOL_SIZE]>();
        if ptr_addr < blocks_base || ptr_addr >= blocks_base + pool_size {
            debug_assert!(false, "SchedNodePool::free() — pointeur hors bornes");
            return;
        }
        let offset = ptr_addr - blocks_base;
        let idx = offset / 64;
        // Atomically reset bit to 1 (free block)
        let word_idx = idx / 64;
        let bit = idx % 64;
        self.free_bits[word_idx].fetch_or(1u64 << bit, Ordering::Release);
        self.alloc_count.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Global instance of SchedNodePool.
static SCHED_NODE_POOL: SchedNodePool = SchedNodePool::new_uninit();

/// Initializes SchedNodePool (called from memory::init() after EmergencyPool).
///
/// # Safety
/// Called once from BSP before any scheduler usage.
pub unsafe fn init_sched_pool() {
    SCHED_NODE_POOL.init();
}

// ─────────────────────────────────────────────────────────────────────────────
// C ABI EXPORTS — scheduler/sync/wait_queue.rs interface
// ─────────────────────────────────────────────────────────────────────────────
//
// These `#[no_mangle] extern "C"` functions are the FFI bridge between
// scheduler/sync/wait_queue.rs (RULE WAITQ-01 DOC3) and this module.
//
// scheduler/ is Layer 1 and cannot directly import memory/.
// Separation is preserved: scheduler/ receives a raw opaque pointer
// which it casts to its own definition of WaitNode.
//
// SYNCHRONIZATION: if scheduler::WaitNode size changes beyond
// 64 bytes, SCHED_POOL_SIZE or RawBlock64 size must be increased.
// ─────────────────────────────────────────────────────────────────────────────

/// Allocates an aligned 64-byte block from EmergencyPool for scheduler.
///
/// Returns `null` if pool is exhausted.
///
/// ## User
/// `scheduler::sync::wait_queue::WaitNode::alloc()` — RULE WAITQ-01.
///
/// ## Guaranteed layout
/// The returned block is exactly 64 bytes, aligned to 64 bytes.
/// Scheduler writes its own struct WaitNode (#[repr(C)], <= 64 bytes).
///
/// # Safety
/// Called from Ring 0 code, preemption disabled recommended.
/// The returned pointer is valid until `emergency_pool_free_wait_node()`.
#[no_mangle]
pub unsafe extern "C" fn emergency_pool_alloc_wait_node() -> *mut u8 {
    SCHED_NODE_POOL.alloc()
}

/// Frees a block previously allocated by `emergency_pool_alloc_wait_node()`.
///
/// ## User
/// `scheduler::sync::wait_queue::WaitNode::free()` — RULE WAITQ-01.
///
/// # Safety
/// `node` must be a pointer returned by `emergency_pool_alloc_wait_node()`.
/// Block must no longer be in any wait queue list.
#[no_mangle]
pub unsafe extern "C" fn emergency_pool_free_wait_node(node: *mut u8) {
    SCHED_NODE_POOL.free(node);
}

/// Reinitializes/publishes scheduler blocks pool during boot sequence.
///
/// Used by `scheduler::sync::wait_queue::init()` as safeguard if
/// early arch path and `kernel_init()` were replayed in a different order.
#[no_mangle]
pub unsafe extern "C" fn emergency_pool_init_wait_node_pool() {
    init_sched_pool();
}

#[cfg(test)]
mod tests {
    use super::*;

    static SCHED_POOL_TEST_LOCK: spin::Mutex<()> = spin::Mutex::new(());

    #[test]
    fn emergency_pool_init_is_idempotent() {
        let pool = EmergencyPool::new_uninit();

        unsafe {
            pool.init();
            pool.init();
        }

        assert!(pool.is_initialized());
        assert_eq!(pool.allocated_count(), 0);

        let node = pool
            .acquire(1)
            .expect("pool should allocate after double init");
        assert!(pool.owns(node));
        assert_eq!(pool.allocated_count(), 1);

        unsafe {
            pool.release(node);
        }
        assert_eq!(pool.allocated_count(), 0);
    }

    #[test]
    fn emergency_pool_double_init_stress_preserves_capacity() {
        let pool = EmergencyPool::new_uninit();

        unsafe {
            pool.init();
            pool.init();
        }

        let mut acquired: [Option<&WaitNode>; EMERGENCY_POOL_SIZE] =
            [const { None }; EMERGENCY_POOL_SIZE];
        for (idx, slot) in acquired.iter_mut().enumerate() {
            *slot = pool.acquire(idx + 1);
            assert!(slot.is_some(), "allocation {} should succeed", idx);
        }

        assert!(pool.is_full());
        assert!(pool.acquire(EMERGENCY_POOL_SIZE + 1).is_none());

        for slot in acquired.into_iter().flatten() {
            unsafe {
                pool.release(slot);
            }
        }

        assert_eq!(pool.allocated_count(), 0);
        assert_eq!(pool.free_count(), EMERGENCY_POOL_SIZE);
    }

    #[test]
    fn sched_node_pool_alloc_free_smoke() {
        let _guard = SCHED_POOL_TEST_LOCK.lock();

        unsafe {
            init_sched_pool();
        }

        let ptr = unsafe { emergency_pool_alloc_wait_node() };
        assert!(
            !ptr.is_null(),
            "SchedNodePool should return one block after init"
        );

        unsafe {
            emergency_pool_free_wait_node(ptr);
        }
    }

    #[test]
    fn sched_node_pool_capacity_stress() {
        let _guard = SCHED_POOL_TEST_LOCK.lock();

        unsafe {
            init_sched_pool();
        }

        let mut blocks: [*mut u8; SCHED_POOL_SIZE] = [core::ptr::null_mut(); SCHED_POOL_SIZE];
        for (idx, slot) in blocks.iter_mut().enumerate() {
            let ptr = unsafe { emergency_pool_alloc_wait_node() };
            assert!(!ptr.is_null(), "allocation {} should succeed", idx);
            *slot = ptr;
        }

        assert!(
            unsafe { emergency_pool_alloc_wait_node() }.is_null(),
            "SchedNodePool should report exhaustion once all blocks are allocated"
        );

        for ptr in blocks {
            unsafe {
                emergency_pool_free_wait_node(ptr);
            }
        }

        let ptr = unsafe { emergency_pool_alloc_wait_node() };
        assert!(
            !ptr.is_null(),
            "SchedNodePool should be reusable after a full alloc/free cycle"
        );
        unsafe {
            emergency_pool_free_wait_node(ptr);
        }
    }
}
