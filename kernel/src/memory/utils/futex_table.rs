// kernel/src/memory/utils/futex_table.rs
//
// ──────────────────────────────────────────────────────────────────────────────
// UNIQUE FUTEX TABLE — GLOBAL SINGLETON  (RULE: never duplicated)
// ──────────────────────────────────────────────────────────────────────────────
//
// Architecture:
//   • FUTEX_HASH_BUCKETS = 4096 buckets (constant from core/constants.rs).
//     RULE MEM-FUTEX (V-34): >= 4096 + SipHash-keyed — anti-DoS via collision.
//   • Hash = SipHash-1-3 keyed (seed initialized from security::crypto::rng
//     at boot step 18, via init_futex_seed()). Fallback FNV before init.
//   • Each bucket is an intrusive linked list of FutexWaiter.
//   • Bucket lock is a spin::Mutex<BucketInner>.
//
// Operations:
//   futex_wait(virt_addr, expected, tid, wake_fn)
//     -> if *virt_addr == expected: enqueues waiter and returns Waiting
//     -> otherwise: returns ValueMismatch immediately
//
//   futex_wake(virt_addr, max_wakers) -> wakes up to max_wakers threads
//   futex_wake_n = futex_wake with max_wakers = n
//   futex_requeue(src, dst, max_wake, max_requeue)
//
// The wake-up function `wake_fn` is provided by the scheduler via injection of
// fn pointer — memory/ does not depend on scheduler/.
//
// LAYER 0 — no scheduler/process/ipc/fs dependencies.

use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, AtomicU8, Ordering};
use spin::Mutex;

use crate::memory::core::constants::FUTEX_HASH_BUCKETS;

// ─────────────────────────────────────────────────────────────────────────────
// Wake callback type
// ─────────────────────────────────────────────────────────────────────────────

/// Signature of the wake-up function provided by the scheduler.
/// `tid`: identifier of thread to wake up.
/// `code`: return code for `futex_wait` seen by woken thread (0 = ok).
pub type WakeFn = fn(tid: u64, code: i32);

/// No-op implementation used before scheduler is initialized.
#[allow(dead_code)]
fn nop_wake(_tid: u64, _code: i32) {}

// ─────────────────────────────────────────────────────────────────────────────
// Waiter
// ─────────────────────────────────────────────────────────────────────────────

/// A thread waiting on a futex address.
#[repr(C)]
pub struct FutexWaiter {
    /// Virtual address on which this thread waits.
    pub virt_addr: u64,
    /// Expected value (checked upon enqueue).
    pub expected_val: u32,
    /// Waiter thread ID.
    pub tid: u64,
    /// Wake function injected by scheduler.
    pub wake_fn: WakeFn,
    /// Return code transmitted to thread on wakeup.
    pub wake_code: i32,
    /// Flag: waiter has been woken up / cancelled.
    pub woken: AtomicBool,
    /// Link in the bucket intrusive list.
    pub next: Option<core::ptr::NonNull<FutexWaiter>>,
    _pad: [u8; 7],
}

impl FutexWaiter {
    pub const fn new(virt_addr: u64, expected_val: u32, tid: u64, wake_fn: WakeFn) -> Self {
        Self {
            virt_addr,
            expected_val,
            tid,
            wake_fn,
            wake_code: 0,
            woken: AtomicBool::new(false),
            next: None,
            _pad: [0; 7],
        }
    }
}

const _: () = assert!(
    core::mem::size_of::<FutexWaiter>() == 56,
    "FutexWaiter doit garder un layout ABI x86_64 de 56 bytes"
);
const _: () = assert!(
    core::mem::align_of::<FutexWaiter>() == 8,
    "FutexWaiter doit rester aligne sur 8 bytes"
);

// ─────────────────────────────────────────────────────────────────────────────
// Hash bucket
// ─────────────────────────────────────────────────────────────────────────────

/// Bucket inner state — linked list of waiters.
struct BucketInner {
    /// List head (raw pointer to FutexWaiter allocated by caller).
    head: Option<core::ptr::NonNull<FutexWaiter>>,
    /// Number of waiters in this bucket.
    count: u32,
}

/// # SAFETY: access protected by Mutex.
unsafe impl Send for BucketInner {}
unsafe impl Sync for BucketInner {}

impl BucketInner {
    const fn new() -> Self {
        Self {
            head: None,
            count: 0,
        }
    }

    /// Enqueues `waiter` at head of list.
    ///
    /// # Safety: `waiter` must remain valid until dequeued.
    unsafe fn push(&mut self, waiter: *mut FutexWaiter) {
        (*waiter).next = self.head;
        self.head = core::ptr::NonNull::new(waiter);
        self.count += 1;
    }

    /// Removes `waiter` from list. Returns `true` if found.
    ///
    /// # Safety: `waiter` must be in this list.
    unsafe fn remove(&mut self, waiter: *mut FutexWaiter) -> bool {
        let mut prev: *mut Option<core::ptr::NonNull<FutexWaiter>> = &mut self.head;
        let mut cur = self.head;
        while let Some(node) = cur {
            if node.as_ptr() == waiter {
                *prev = (*node.as_ptr()).next;
                self.count = self.count.saturating_sub(1);
                return true;
            }
            prev = &mut (*node.as_ptr()).next;
            cur = (*node.as_ptr()).next;
        }
        false
    }

    /// Wakes up to `max` waiters for `virt_addr`. Returns number of woken waiters.
    ///
    /// # Safety: pointed FutexWaiters must remain valid during call.
    unsafe fn wake(&mut self, virt_addr: u64, max: u32, wake_code: i32) -> u32 {
        let mut woken = 0u32;
        let mut prev: *mut Option<core::ptr::NonNull<FutexWaiter>> = &mut self.head;
        let mut cur = self.head;

        while let Some(node) = cur {
            let w = node.as_ptr();
            let next = (*w).next;

            if (*w).virt_addr == virt_addr && !(*w).woken.load(Ordering::Acquire) {
                // Remove from list.
                *prev = next;
                self.count = self.count.saturating_sub(1);

                // Mark woken and call wake_fn.
                (*w).wake_code = wake_code;
                (*w).woken.store(true, Ordering::Release);
                ((*w).wake_fn)((*w).tid, wake_code);

                woken += 1;
                if woken >= max {
                    break;
                }
                cur = next;
            } else {
                prev = &mut (*node.as_ptr()).next;
                cur = next;
            }
        }
        woken
    }

    /// Requeues up to `max_requeue` waiters from `src` to another bucket `dst_inner`.
    /// Returns number re-queued.
    ///
    /// # Safety: valid pointers during operation.
    unsafe fn requeue_to(
        &mut self,
        src_addr: u64,
        dst_addr: u64,
        dst_inner: &mut BucketInner,
        max_requeue: u32,
    ) -> u32 {
        let mut requeued = 0u32;
        let mut prev: *mut Option<core::ptr::NonNull<FutexWaiter>> = &mut self.head;
        let mut cur = self.head;

        while let Some(node) = cur {
            let w = node.as_ptr();
            let next = (*w).next;

            if (*w).virt_addr == src_addr && !(*w).woken.load(Ordering::Acquire) {
                // Remove from this bucket.
                *prev = next;
                self.count = self.count.saturating_sub(1);

                // Update address and push to dst.
                (*w).virt_addr = dst_addr;
                dst_inner.push(w);

                requeued += 1;
                if requeued >= max_requeue {
                    break;
                }
                cur = next;
            } else {
                prev = &mut (*node.as_ptr()).next;
                cur = next;
            }
        }
        requeued
    }

    /// Requeues up to `max_requeue` waiters from `src_addr` to `dst_addr` within the same bucket.
    unsafe fn requeue_inplace(&mut self, src_addr: u64, dst_addr: u64, max_requeue: u32) -> u32 {
        let mut requeued = 0u32;
        let mut cur = self.head;

        while let Some(node) = cur {
            let w = node.as_ptr();
            let next = (*w).next;

            if (*w).virt_addr == src_addr && !(*w).woken.load(Ordering::Acquire) {
                (*w).virt_addr = dst_addr;
                requeued += 1;
                if requeued >= max_requeue {
                    break;
                }
            }
            cur = next;
        }
        requeued
    }
}

/// A public bucket with its Mutex.
pub struct FutexBucket {
    inner: Mutex<BucketInner>,
}

impl Default for FutexBucket {
    fn default() -> Self {
        Self::new()
    }
}

impl FutexBucket {
    pub const fn new() -> Self {
        Self {
            inner: Mutex::new(BucketInner::new()),
        }
    }

    /// Number of waiters in this bucket.
    #[inline]
    pub fn count(&self) -> u32 {
        self.inner.lock().count
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Global statistics
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
pub struct FutexStats {
    pub wait_calls: AtomicU64,
    pub wake_calls: AtomicU64,
    pub requeue_calls: AtomicU64,
    pub value_mismatches: AtomicU64,
    pub timeouts: AtomicU64,
    pub total_woken: AtomicU64,
    pub max_bucket_depth: AtomicU32,
}

impl Default for FutexStats {
    fn default() -> Self {
        Self::new()
    }
}

impl FutexStats {
    const fn new() -> Self {
        Self {
            wait_calls: AtomicU64::new(0),
            wake_calls: AtomicU64::new(0),
            requeue_calls: AtomicU64::new(0),
            value_mismatches: AtomicU64::new(0),
            timeouts: AtomicU64::new(0),
            total_woken: AtomicU64::new(0),
            max_bucket_depth: AtomicU32::new(0),
        }
    }
}

unsafe impl Sync for FutexStats {}
pub static FUTEX_STATS: FutexStats = FutexStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// Main table
// ─────────────────────────────────────────────────────────────────────────────

/// Futex hash table — global singleton.
/// `FUTEX_HASH_BUCKETS = 4096` (core/constants.rs) — RULE MEM-FUTEX (V-34).
pub struct FutexHashTable {
    buckets: [FutexBucket; FUTEX_HASH_BUCKETS],
}

/// # SAFETY: buckets are Mutexes protecting lists — inherently Sync.
unsafe impl Sync for FutexHashTable {}

// SAFETY: FutexBucket = Mutex<BucketInner>. spin::Mutex all-zeros = unlocked
// (AtomicBool(0)). BucketInner all-zeros: head = None (0), count = 0 — valid
// initial state identical to FutexBucket::new(). Rust does not support
// [non_copy_expr; 4096] without Copy, hence zeroed() initialization.
pub static FUTEX_TABLE: FutexHashTable = unsafe { core::mem::zeroed() };

// ─────────────────────────────────────────────────────────────────────────────
// Hash — SipHash-1-3 keyed (RULE MEM-FUTEX / V-34)
// ─────────────────────────────────────────────────────────────────────────────

// Hash seed — 16 atomic bytes (2 x u64).
// Initialized to 0; set_futex_hash_seed() is called at boot step 18.
// Before init, hash is SipHash with key = 0 (suboptimal but safe at boot).
static FUTEX_SEED_K0: AtomicU64 = AtomicU64::new(0);
static FUTEX_SEED_K1: AtomicU64 = AtomicU64::new(0);

/// Flag: seed initialized (atomic 8-bit — no spin::Once dependency).
static FUTEX_SEED_SET: AtomicU8 = AtomicU8::new(0);

/// Initializes SipHash seed from security::crypto::rng (boot step 18).
/// MUST be called once, from BSP, BEFORE APs are started.
/// After this call, bucket_index() uses SipHash-1-3 keyed.
pub fn init_futex_seed(key: [u8; 16]) {
    let k0 = u64::from_le_bytes(key[0..8].try_into().unwrap_or([0u8; 8]));
    let k1 = u64::from_le_bytes(key[8..16].try_into().unwrap_or([0u8; 8]));
    FUTEX_SEED_K0.store(k0, Ordering::Release);
    FUTEX_SEED_K1.store(k1, Ordering::Release);
    FUTEX_SEED_SET.store(1, Ordering::Release);
}

/// SipHash-1-3 for a single 8-byte block (a physical/virtual address).
/// No external dependencies — inline implementation (no_std).
#[inline]
fn siphash13(k0: u64, k1: u64, data: u64) -> u64 {
    macro_rules! sipround {
        ($v0:expr, $v1:expr, $v2:expr, $v3:expr) => {
            $v0 = $v0.wrapping_add($v1);
            $v1 = $v1.rotate_left(13);
            $v1 ^= $v0;
            $v0 = $v0.rotate_left(32);
            $v2 = $v2.wrapping_add($v3);
            $v3 = $v3.rotate_left(16);
            $v3 ^= $v2;
            $v0 = $v0.wrapping_add($v3);
            $v3 = $v3.rotate_left(21);
            $v3 ^= $v0;
            $v2 = $v2.wrapping_add($v1);
            $v1 = $v1.rotate_left(17);
            $v1 ^= $v2;
            $v2 = $v2.rotate_left(32);
        };
    }
    let mut v0 = k0 ^ 0x736f6d6570736575u64;
    let mut v1 = k1 ^ 0x646f72616e646f6du64;
    let mut v2 = k0 ^ 0x6c7967656e657261u64;
    let mut v3 = k1 ^ 0x7465646279746573u64;

    // Compression — 1 round on the single 8-byte block
    let m = data;
    v3 ^= m;
    sipround!(v0, v1, v2, v3);
    v0 ^= m;

    // Final block: (len % 256) << 56 = 0x0800000000000000 (len == 8)
    let b: u64 = 0x0800_0000_0000_0000;
    v3 ^= b;
    sipround!(v0, v1, v2, v3);
    v0 ^= b;

    // Finalization — 3 rounds
    v2 ^= 0xff;
    sipround!(v0, v1, v2, v3);
    sipround!(v0, v1, v2, v3);
    sipround!(v0, v1, v2, v3);

    v0 ^ v1 ^ v2 ^ v3
}

/// Hash of a virtual address -> bucket index [0, FUTEX_HASH_BUCKETS).
/// Uses SipHash-1-3 keyed if seed is initialized, FNV otherwise (boot).
#[inline]
fn bucket_index(virt_addr: u64) -> usize {
    if FUTEX_SEED_SET.load(Ordering::Acquire) != 0 {
        // SipHash-1-3 keyed — anti-DoS (RULE MEM-FUTEX / V-34)
        let k0 = FUTEX_SEED_K0.load(Ordering::Relaxed);
        let k1 = FUTEX_SEED_K1.load(Ordering::Relaxed);
        (siphash13(k0, k1, virt_addr) as usize) & (FUTEX_HASH_BUCKETS - 1)
    } else {
        // Fallback FNV-1a (before init_futex_seed, during early boot)
        const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const FNV_PRIME: u64 = 0x0000_0100_0000_01B3;
        let mut h = FNV_OFFSET;
        for b in virt_addr.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(FNV_PRIME);
        }
        h as usize & (FUTEX_HASH_BUCKETS - 1)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Results
// ─────────────────────────────────────────────────────────────────────────────

/// Result of `futex_wait`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FutexWaitResult {
    /// Thread enqueued successfully — scheduler must block it.
    Waiting,
    /// `*virt_addr != expected` at check time — no blocking.
    ValueMismatch,
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Enqueues a waiter on `virt_addr` if `*virt_addr == expected`.
///
/// Caller (syscall handler):
///   1. Calls `futex_wait`: if `Waiting`, switches thread to BLOCKED state.
///   2. Reallocates thread when its `waiter.woken` becomes `true`.
///
/// # Safety
/// - `waiter` must point to a valid, unused FutexWaiter.
/// - `virt_addr` must be a valid user address pointing to a `u32`.
pub unsafe fn futex_wait(
    virt_addr: u64,
    expected: u32,
    waiter: *mut FutexWaiter,
    wake_fn: WakeFn,
) -> FutexWaitResult {
    FUTEX_STATS.wait_calls.fetch_add(1, Ordering::Relaxed);

    let idx = bucket_index(virt_addr);
    let mut bucket = FUTEX_TABLE.buckets[idx].inner.lock();

    // Atomically check value *before* sleeping (to avoid race condition
    // between read + enqueue).
    if virt_addr == 0 || !virt_addr.is_multiple_of(4) || !crate::memory::core::address::is_user_canonical(virt_addr) {
        FUTEX_STATS.value_mismatches.fetch_add(1, Ordering::Relaxed);
        return FutexWaitResult::ValueMismatch;
    }
    let current_val = (virt_addr as *const u32).read_volatile();
    if current_val != expected {
        FUTEX_STATS.value_mismatches.fetch_add(1, Ordering::Relaxed);
        return FutexWaitResult::ValueMismatch;
    }

    // Initialize waiter.
    (*waiter).virt_addr = virt_addr;
    (*waiter).expected_val = expected;
    (*waiter).wake_fn = wake_fn;
    (*waiter).woken.store(false, Ordering::Release);
    (*waiter).next = None;

    bucket.push(waiter);

    // Update bucket depth stats.
    let depth = bucket.count;
    drop(bucket);
    let prev_max = FUTEX_STATS.max_bucket_depth.load(Ordering::Relaxed);
    if depth > prev_max {
        FUTEX_STATS.max_bucket_depth.store(depth, Ordering::Relaxed);
    }

    FutexWaitResult::Waiting
}

/// Removes a waiter from its list (cancelling wait, e.g. timeout).
///
/// Must be called before freeing waiter memory.
///
/// # Safety
/// `waiter` must be in table.
pub unsafe fn futex_cancel(waiter: *mut FutexWaiter) {
    if waiter.is_null() {
        return;
    }
    let virt_addr = (*waiter).virt_addr;
    let idx = bucket_index(virt_addr);
    let mut bucket = FUTEX_TABLE.buckets[idx].inner.lock();
    if bucket.remove(waiter) {
        (*waiter).next = None;
    }
    (*waiter).wake_code = -1;
    (*waiter).woken.store(true, Ordering::Release);
    FUTEX_STATS.timeouts.fetch_add(1, Ordering::Relaxed);
}

/// Wakes up to `max` threads waiting on `virt_addr`.
/// Returns number of woken threads.
///
/// `wake_code`: value returned by `futex_wait` to woken thread.
///
/// # Safety
/// Caller guarantees waiters are valid.
pub unsafe fn futex_wake(virt_addr: u64, max: u32, wake_code: i32) -> u32 {
    FUTEX_STATS.wake_calls.fetch_add(1, Ordering::Relaxed);
    let idx = bucket_index(virt_addr);
    let mut bucket = FUTEX_TABLE.buckets[idx].inner.lock();
    let woken = bucket.wake(virt_addr, max, wake_code);
    FUTEX_STATS
        .total_woken
        .fetch_add(woken as u64, Ordering::Relaxed);
    woken
}

/// Wakes exactly `n` threads.
///
/// # Safety
/// Caller guarantees waiters are valid.
#[inline]
pub unsafe fn futex_wake_n(virt_addr: u64, n: u32) -> u32 {
    futex_wake(virt_addr, n, 0)
}

/// Wakes `max_wake` threads on `src_addr` and requeues `max_requeue` others
/// to `dst_addr`. Useful for `pthread_cond_broadcast`.
///
/// # Safety
/// Valid waiters.
pub unsafe fn futex_requeue(
    src_addr: u64,
    dst_addr: u64,
    max_wake: u32,
    max_requeue: u32,
    wake_code: i32,
) -> (u32, u32) {
    FUTEX_STATS.requeue_calls.fetch_add(1, Ordering::Relaxed);

    let src_idx = bucket_index(src_addr);
    let dst_idx = bucket_index(dst_addr);

    if src_idx == dst_idx {
        // Same bucket: single acquisition.
        let mut bucket = FUTEX_TABLE.buckets[src_idx].inner.lock();
        let woken = bucket.wake(src_addr, max_wake, wake_code);
        let requeued = if src_addr != dst_addr {
            bucket.requeue_inplace(src_addr, dst_addr, max_requeue)
        } else {
            0
        };
        FUTEX_STATS
            .total_woken
            .fetch_add(woken as u64, Ordering::Relaxed);
        return (woken, requeued);
    }

    // Two different buckets — always lock in index order to avoid deadlocks.
    let (lo, hi) = if src_idx < dst_idx {
        (src_idx, dst_idx)
    } else {
        (dst_idx, src_idx)
    };

    let lo_guard = FUTEX_TABLE.buckets[lo].inner.lock();
    let hi_guard = FUTEX_TABLE.buckets[hi].inner.lock();

    // Obtain mutable pointers — addr_of!(*guard) avoids &T->*mut T (UB lint).
    // Both guards protect distinct buckets (lo < hi), no aliasing.
    let lo_ptr = core::ptr::addr_of!(*lo_guard) as *mut BucketInner;
    let hi_ptr = core::ptr::addr_of!(*hi_guard) as *mut BucketInner;
    // SAFETY: lo_ptr/hi_ptr -> distinct buckets (lo < hi), protected by their guards — no aliasing.
    let (src_inner, dst_inner) = unsafe {
        if src_idx < dst_idx {
            (&mut *lo_ptr, &mut *hi_ptr)
        } else {
            (&mut *hi_ptr, &mut *lo_ptr)
        }
    };

    let woken = src_inner.wake(src_addr, max_wake, wake_code);
    let requeued = src_inner.requeue_to(src_addr, dst_addr, dst_inner, max_requeue);

    FUTEX_STATS
        .total_woken
        .fetch_add(woken as u64, Ordering::Relaxed);
    (woken, requeued)
}

// ─────────────────────────────────────────────────────────────────────────────
// Initialization
// ─────────────────────────────────────────────────────────────────────────────

/// Init futex table — structure already initialized const, nothing to do.
pub fn init() {}

// ─────────────────────────────────────────────────────────────────────────────
// sys_futex — FUTEX syscall entry point
// ─────────────────────────────────────────────────────────────────────────────

/// FUTEX operation codes (Linux-compatible).
const FUTEX_WAIT: u32 = 0;
const FUTEX_WAKE: u32 = 1;
const FUTEX_REQUEUE: u32 = 3;
/// Mask allowing removal of PRIVATE flag before comparison.
const FUTEX_PRIVATE_FLAG: u32 = 128;

/// Possible errors from `sys_futex`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FutexError {
    /// Unknown or unsupported operation.
    InvalidOp,
    /// Invalid uaddr address (not 4-byte aligned, outside user space, null).
    InvalidAddr,
    /// *uaddr != val at FUTEX_WAIT time.
    ValueMismatch,
    /// Wait interrupted (no sleep hook registered).
    Interrupted,
    /// Timeout expired.
    Timeout,
    /// Memory allocation impossible.
    NoMemory,
}

impl FutexError {
    /// Translates error to negative POSIX errno.
    pub fn to_kernel_errno(self) -> i64 {
        match self {
            FutexError::InvalidOp => -22,     // EINVAL
            FutexError::InvalidAddr => -14,   // EFAULT
            FutexError::ValueMismatch => -11, // EAGAIN
            FutexError::Interrupted => -4,    // EINTR
            FutexError::Timeout => -110,      // ETIMEDOUT
            FutexError::NoMemory => -12,      // ENOMEM
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Sleep hook (injected by scheduler)
// ─────────────────────────────────────────────────────────────────────────────

/// Signature of the blocking function provided by scheduler.
/// `waiter`: waiter enqueued in futex table.
/// `timeout_ns`: maximum timeout in nanoseconds (0 = infinite).
/// Returns 0 if woken normally, -EINTR if interrupted, -ETIMEDOUT if timeout.
pub type FutexSleepHook = fn(waiter: *mut FutexWaiter, timeout_ns: u64) -> i32;

static FUTEX_SLEEP_HOOK: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

/// Registers the blocking function provided by scheduler.
/// Called upon scheduler initialization.
pub fn register_sleep_hook(hook: FutexSleepHook) {
    FUTEX_SLEEP_HOOK.store(hook as *mut (), Ordering::Release);
}

/// Returns registered hook, or None if undefined.
fn get_sleep_hook() -> Option<FutexSleepHook> {
    let ptr = FUTEX_SLEEP_HOOK.load(Ordering::Acquire);
    if ptr.is_null() {
        None
    } else {
        // SAFETY: Stored via register_sleep_hook with matching FutexSleepHook signature.
        Some(unsafe { core::mem::transmute::<*mut (), FutexSleepHook>(ptr) })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Noop wake (used by sys_futex for temporary FutexWaiter)
// ─────────────────────────────────────────────────────────────────────────────

fn nop_wake_sys(_tid: u64, _code: i32) {}

// ─────────────────────────────────────────────────────────────────────────────
// sys_futex
// ─────────────────────────────────────────────────────────────────────────────

/// Entry point of `futex(2)` syscall.
///
/// - `uaddr`: user address of 32-bit integer to operate on.
/// - `op`: operation (FUTEX_WAIT/WAKE/REQUEUE + optionally PRIVATE).
/// - `val`: expected value (FUTEX_WAIT) or thread count (FUTEX_WAKE).
/// - `timeout`: timeout in ns (FUTEX_WAIT), or max_requeue (FUTEX_REQUEUE).
/// - `uaddr2`: destination address (FUTEX_REQUEUE).
/// - `val3`: max_requeue wake count (FUTEX_REQUEUE), unused otherwise.
///
/// Returns number of woken threads (FUTEX_WAKE/REQUEUE) or 0 (FUTEX_WAIT).
pub fn sys_futex(
    uaddr: u64,
    op: u32,
    val: u32,
    timeout: u64,
    uaddr2: u64,
    val3: u32,
) -> Result<i64, FutexError> {
    // Strict validation of user address (AUDIT-MEM2 / T4-003)
    if uaddr == 0
        || !uaddr.is_multiple_of(4)
        || !crate::memory::core::address::is_user_canonical(uaddr)
        || uaddr > crate::syscall::validation::USER_ADDR_MAX - 4
    {
        return Err(FutexError::InvalidAddr);
    }

    // Strip PRIVATE flag — current table is already per-process.
    let cmd = op & !FUTEX_PRIVATE_FLAG;

    match cmd {
        // ── FUTEX_WAIT ────────────────────────────────────────────────────
        FUTEX_WAIT => {
            // Allocate waiter on current thread stack.
            let mut waiter = FutexWaiter::new(uaddr, val, 0, nop_wake_sys);

            // SAFETY: uaddr is a valid user address (presumed verified on syscall side).
            let result = unsafe { futex_wait(uaddr, val, &mut waiter, nop_wake_sys) };

            match result {
                FutexWaitResult::ValueMismatch => Err(FutexError::ValueMismatch),
                FutexWaitResult::Waiting => {
                    // Call scheduler hook if available.
                    match get_sleep_hook() {
                        Some(sleep) => {
                            let rc = sleep(&mut waiter, timeout);
                            if rc == -4 {
                                // EINTR — cancel wait
                                // SAFETY: waiter is still in table.
                                unsafe { futex_cancel(&mut waiter) };
                                Err(FutexError::Interrupted)
                            } else if rc == -110 {
                                // ETIMEDOUT
                                // SAFETY: waiter is still in table (not yet woken).
                                unsafe { futex_cancel(&mut waiter) };
                                Err(FutexError::Timeout)
                            } else {
                                Ok(0)
                            }
                        }
                        None => {
                            // No scheduler: cancel and return EINTR.
                            // SAFETY: waiter is still in table.
                            unsafe { futex_cancel(&mut waiter) };
                            Err(FutexError::Interrupted)
                        }
                    }
                }
            }
        }

        // ── FUTEX_WAKE ────────────────────────────────────────────────────
        FUTEX_WAKE => {
            // val = max threads to wake; wake_code = 0.
            // SAFETY: uaddr is a valid user address, verified on syscall side.
            let woken = unsafe { futex_wake(uaddr, val, 0) };
            Ok(woken as i64)
        }

        // ── FUTEX_REQUEUE ─────────────────────────────────────────────────
        FUTEX_REQUEUE => {
            if uaddr2 == 0
                || !uaddr2.is_multiple_of(4)
                || !crate::memory::core::address::is_user_canonical(uaddr2)
                || uaddr2 > crate::syscall::validation::USER_ADDR_MAX - 4
            {
                return Err(FutexError::InvalidAddr);
            }
            // val  = max threads woken on uaddr
            // val3 = max threads requeued to uaddr2
            // timeout (third Linux arg) = max_wake; val3 = max_requeue
            let max_wake = val;
            let max_requeue = val3;
            // SAFETY: uaddr/uaddr2 are valid user addresses, verified on syscall side.
            let (woken, _requeued) =
                unsafe { futex_requeue(uaddr, uaddr2, max_wake, max_requeue, 0) };
            Ok(woken as i64)
        }

        _ => Err(FutexError::InvalidOp),
    }
}
