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
    /// Virtual or physical futex key address on which this thread waits.
    pub virt_addr: AtomicU64,
    /// Namespace key separating processes (private) vs shared memory (`space_id`).
    pub space_id: AtomicU64,
    /// Expected value (checked upon enqueue).
    pub expected_val: u32,
    /// Return code transmitted to thread on wakeup.
    pub wake_code: i32,
    /// Waiter thread ID.
    pub tid: u64,
    /// Wake function injected by scheduler.
    pub wake_fn: WakeFn,
    /// Link in the bucket intrusive list.
    pub next: Option<core::ptr::NonNull<FutexWaiter>>,
    /// Flag: waiter has been woken up / cancelled.
    pub woken: AtomicBool,
    _pad: [u8; 7],
}

impl FutexWaiter {
    pub const fn new(virt_addr: u64, expected_val: u32, tid: u64, wake_fn: WakeFn) -> Self {
        Self {
            virt_addr: AtomicU64::new(virt_addr),
            space_id: AtomicU64::new(0),
            expected_val,
            wake_code: 0,
            tid,
            wake_fn,
            next: None,
            woken: AtomicBool::new(false),
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

    /// Wakes up to `max` waiters for `(space_id, virt_addr)`. Returns number of woken waiters.
    ///
    /// # Safety: pointed FutexWaiters must remain valid during call.
    unsafe fn wake(&mut self, space_id: u64, virt_addr: u64, max: u32, wake_code: i32) -> u32 {
        let mut woken = 0u32;
        let mut prev: *mut Option<core::ptr::NonNull<FutexWaiter>> = &mut self.head;
        let mut cur = self.head;

        while let Some(node) = cur {
            let w = node.as_ptr();
            let next = (*w).next;

            if (*w).space_id.load(Ordering::Acquire) == space_id
                && (*w).virt_addr.load(Ordering::Acquire) == virt_addr
                && !(*w).woken.load(Ordering::Acquire)
            {
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

    /// Requeues up to `max_requeue` waiters from `(src_space, src_addr)` to `(dst_space, dst_addr)`
    /// in another bucket `dst_inner`. Returns number re-queued.
    ///
    /// # Safety: valid pointers during operation.
    unsafe fn requeue_to(
        &mut self,
        src_space: u64,
        src_addr: u64,
        dst_space: u64,
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

            if (*w).space_id.load(Ordering::Acquire) == src_space
                && (*w).virt_addr.load(Ordering::Acquire) == src_addr
                && !(*w).woken.load(Ordering::Acquire)
            {
                // Remove from this bucket.
                *prev = next;
                self.count = self.count.saturating_sub(1);

                // Update key and push to dst while both buckets are locked.
                (*w).space_id.store(dst_space, Ordering::Release);
                (*w).virt_addr.store(dst_addr, Ordering::Release);
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

    /// Requeues up to `max_requeue` waiters from `(src_space, src_addr)` to `(dst_space, dst_addr)`
    /// within the same bucket.
    unsafe fn requeue_inplace(
        &mut self,
        src_space: u64,
        src_addr: u64,
        dst_space: u64,
        dst_addr: u64,
        max_requeue: u32,
    ) -> u32 {
        let mut requeued = 0u32;
        let mut cur = self.head;

        while let Some(node) = cur {
            let w = node.as_ptr();
            let next = (*w).next;

            if (*w).space_id.load(Ordering::Acquire) == src_space
                && (*w).virt_addr.load(Ordering::Acquire) == src_addr
                && !(*w).woken.load(Ordering::Acquire)
            {
                (*w).space_id.store(dst_space, Ordering::Release);
                (*w).virt_addr.store(dst_addr, Ordering::Release);
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

const KERNEL_SPACE_ID: u64 = 0;
const SHARED_SPACE_ID: u64 = 1;
const PRIVATE_SPACE_BIT: u64 = 1u64 << 63;

#[inline]
fn private_space_id(pid: u32) -> u64 {
    PRIVATE_SPACE_BIT | (pid as u64)
}

/// Hash of `(space_id, key_addr)` -> bucket index `[0, FUTEX_HASH_BUCKETS)`.
/// Uses SipHash-1-3 keyed if seed is initialized, FNV otherwise (boot).
#[inline]
fn bucket_index(space_id: u64, virt_addr: u64) -> usize {
    if FUTEX_SEED_SET.load(Ordering::Acquire) != 0 {
        // SipHash-1-3 keyed — anti-DoS (RULE MEM-FUTEX / V-34)
        let k0 = FUTEX_SEED_K0.load(Ordering::Relaxed) ^ space_id;
        let k1 = FUTEX_SEED_K1.load(Ordering::Relaxed) ^ space_id.rotate_right(29);
        (siphash13(k0, k1, virt_addr) as usize) & (FUTEX_HASH_BUCKETS - 1)
    } else {
        // Fallback FNV-1a (before init_futex_seed, during early boot)
        const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const FNV_PRIME: u64 = 0x0000_0100_0000_01B3;
        let mut h = FNV_OFFSET;
        for b in space_id.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(FNV_PRIME);
        }
        for b in virt_addr.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(FNV_PRIME);
        }
        h as usize & (FUTEX_HASH_BUCKETS - 1)
    }
}

#[inline]
unsafe fn read_futex_word(virt_addr: u64, is_user: bool) -> Result<(u32, u64), FutexError> {
    if virt_addr == 0 || !virt_addr.is_multiple_of(4) {
        return Err(FutexError::InvalidAddr);
    }
    if is_user {
        return crate::syscall::validation::resolve_user_read_u32_and_phys(virt_addr)
            .map_err(|_| FutexError::InvalidAddr);
    }
    #[cfg(target_os = "none")]
    if virt_addr < crate::syscall::validation::USER_ADDR_MAX {
        return crate::syscall::validation::resolve_user_read_u32_and_phys(virt_addr)
            .map_err(|_| FutexError::InvalidAddr);
    }
    Ok(((virt_addr as *const u32).read_volatile(), virt_addr))
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

unsafe fn futex_wait_in_space(
    space_id: u64,
    key_addr: u64,
    value_addr: u64,
    is_user: bool,
    expected: u32,
    waiter: *mut FutexWaiter,
    wake_fn: WakeFn,
) -> Result<FutexWaitResult, FutexError> {
    FUTEX_STATS.wait_calls.fetch_add(1, Ordering::Relaxed);

    let idx = bucket_index(space_id, key_addr);
    let mut bucket = FUTEX_TABLE.buckets[idx].inner.lock();

    let (current_val, _) = read_futex_word(value_addr, is_user)?;
    if current_val != expected {
        FUTEX_STATS.value_mismatches.fetch_add(1, Ordering::Relaxed);
        return Ok(FutexWaitResult::ValueMismatch);
    }

    // Initialize waiter under the bucket lock before publishing it to the list.
    (*waiter).space_id.store(space_id, Ordering::Release);
    (*waiter).virt_addr.store(key_addr, Ordering::Release);
    (*waiter).expected_val = expected;
    (*waiter).wake_fn = wake_fn;
    (*waiter).woken.store(false, Ordering::Release);
    (*waiter).next = None;

    bucket.push(waiter);

    let depth = bucket.count;
    drop(bucket);
    let prev_max = FUTEX_STATS.max_bucket_depth.load(Ordering::Relaxed);
    if depth > prev_max {
        FUTEX_STATS.max_bucket_depth.store(depth, Ordering::Relaxed);
    }

    Ok(FutexWaitResult::Waiting)
}

/// Enqueues a waiter on kernel-space key `virt_addr` if `*virt_addr == expected`.
///
/// # Safety
/// - `waiter` must point to a valid, unused `FutexWaiter`.
/// - `virt_addr` must point to a valid 4-byte-aligned `u32`.
pub unsafe fn futex_wait(
    virt_addr: u64,
    expected: u32,
    waiter: *mut FutexWaiter,
    wake_fn: WakeFn,
) -> FutexWaitResult {
    futex_wait_in_space(KERNEL_SPACE_ID, virt_addr, virt_addr, false, expected, waiter, wake_fn)
        .unwrap_or(FutexWaitResult::ValueMismatch)
}

/// Removes a waiter from whichever bucket it currently occupies (cancelling wait, e.g. timeout).
///
/// Must be called before freeing stack-allocated waiter memory.
///
/// # Safety
/// `waiter` must point to a live `FutexWaiter`.
pub unsafe fn futex_cancel(waiter: *mut FutexWaiter) {
    if waiter.is_null() {
        return;
    }
    loop {
        if (*waiter).woken.load(Ordering::Acquire) {
            return;
        }
        let space_id = (*waiter).space_id.load(Ordering::Acquire);
        let virt_addr = (*waiter).virt_addr.load(Ordering::Acquire);
        let idx = bucket_index(space_id, virt_addr);
        let mut bucket = FUTEX_TABLE.buckets[idx].inner.lock();
        if (*waiter).woken.load(Ordering::Acquire) {
            return;
        }
        // Re-check key under lock in case concurrent futex_requeue migrated `waiter`.
        if (*waiter).space_id.load(Ordering::Acquire) != space_id
            || (*waiter).virt_addr.load(Ordering::Acquire) != virt_addr
        {
            continue;
        }
        if bucket.remove(waiter) {
            (*waiter).next = None;
            (*waiter).wake_code = -1;
            (*waiter).woken.store(true, Ordering::Release);
            FUTEX_STATS.timeouts.fetch_add(1, Ordering::Relaxed);
        }
        return;
    }
}

/// Wakes up to `max` threads waiting on `(space_id, key_addr)`.
///
/// # Safety
/// Caller guarantees waiters are valid.
pub unsafe fn futex_wake_in_space(
    space_id: u64,
    key_addr: u64,
    max: u32,
    wake_code: i32,
) -> u32 {
    FUTEX_STATS.wake_calls.fetch_add(1, Ordering::Relaxed);
    let idx = bucket_index(space_id, key_addr);
    let mut bucket = FUTEX_TABLE.buckets[idx].inner.lock();
    let woken = bucket.wake(space_id, key_addr, max, wake_code);
    FUTEX_STATS
        .total_woken
        .fetch_add(woken as u64, Ordering::Relaxed);
    woken
}

/// Wakes threads waiting on `clear_child_tid` (`pthread_join`) in either private or shared namespace.
///
/// # Safety
/// Must be called in the exiting thread's process context.
pub unsafe fn wake_clear_child_tid(pid: u32, virt_addr: u64, phys_addr: Option<u64>) {
    let woke_priv = futex_wake_in_space(private_space_id(pid), virt_addr, 1, 0);
    if woke_priv == 0 {
        if let Some(phys) = phys_addr {
            let _ = futex_wake_in_space(SHARED_SPACE_ID, phys, 1, 0);
        }
    }
}

/// Wakes up to `max` threads waiting on kernel key `virt_addr`.
/// Returns number of woken threads.
///
/// # Safety
/// Caller guarantees waiters are valid.
pub unsafe fn futex_wake(virt_addr: u64, max: u32, wake_code: i32) -> u32 {
    futex_wake_in_space(KERNEL_SPACE_ID, virt_addr, max, wake_code)
}

/// Wakes up to `n` threads waiting on kernel key `virt_addr`.
///
/// # Safety
/// Caller guarantees waiters are valid.
#[inline]
pub unsafe fn futex_wake_n(virt_addr: u64, n: u32) -> u32 {
    futex_wake(virt_addr, n, 0)
}

unsafe fn futex_requeue_in_space(
    src_space: u64,
    src_key: u64,
    dst_space: u64,
    dst_key: u64,
    cmp_expected: Option<(u64, u32, bool)>,
    max_wake: u32,
    max_requeue: u32,
    wake_code: i32,
) -> Result<(u32, u32), FutexError> {
    FUTEX_STATS.requeue_calls.fetch_add(1, Ordering::Relaxed);

    let src_idx = bucket_index(src_space, src_key);
    let dst_idx = bucket_index(dst_space, dst_key);

    if src_idx == dst_idx {
        let mut bucket = FUTEX_TABLE.buckets[src_idx].inner.lock();
        if let Some((uaddr, expected_val, is_user)) = cmp_expected {
            let (cur_val, _) = read_futex_word(uaddr, is_user)?;
            if cur_val != expected_val {
                return Err(FutexError::ValueMismatch);
            }
        }
        let woken = bucket.wake(src_space, src_key, max_wake, wake_code);
        let requeued = if (src_space, src_key) != (dst_space, dst_key) {
            bucket.requeue_inplace(src_space, src_key, dst_space, dst_key, max_requeue)
        } else {
            0
        };
        FUTEX_STATS
            .total_woken
            .fetch_add(woken as u64, Ordering::Relaxed);
        return Ok((woken, requeued));
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
    let (src_inner, dst_inner) = if src_idx < dst_idx {
        (&mut *lo_ptr, &mut *hi_ptr)
    } else {
        (&mut *hi_ptr, &mut *lo_ptr)
    };

    if let Some((uaddr, expected_val, is_user)) = cmp_expected {
        let (cur_val, _) = read_futex_word(uaddr, is_user)?;
        if cur_val != expected_val {
            return Err(FutexError::ValueMismatch);
        }
    }

    let woken = src_inner.wake(src_space, src_key, max_wake, wake_code);
    let requeued = src_inner.requeue_to(src_space, src_key, dst_space, dst_key, dst_inner, max_requeue);

    FUTEX_STATS
        .total_woken
        .fetch_add(woken as u64, Ordering::Relaxed);
    Ok((woken, requeued))
}

/// Wakes `max_wake` threads on kernel `src_addr` and requeues `max_requeue` others
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
    futex_requeue_in_space(
        KERNEL_SPACE_ID,
        src_addr,
        KERNEL_SPACE_ID,
        dst_addr,
        None,
        max_wake,
        max_requeue,
        wake_code,
    )
    .unwrap_or((0, 0))
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
const FUTEX_CMP_REQUEUE: u32 = 4;
const FUTEX_WAIT_BITSET: u32 = 9;
const FUTEX_WAKE_BITSET: u32 = 10;
/// Mask allowing removal of PRIVATE and CLOCK_REALTIME flags before comparison.
const FUTEX_PRIVATE_FLAG: u32 = 128;
const FUTEX_CLOCK_REALTIME: u32 = 256;

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

/// Signature of the function returning the current thread's opaque wake handle (TCB pointer).
pub type FutexTidHook = fn() -> u64;

static FUTEX_SLEEP_HOOK: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());
static FUTEX_WAKE_HOOK: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());
static FUTEX_TID_HOOK: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

/// Registers the blocking function provided by scheduler.
/// Called upon scheduler initialization.
pub fn register_sleep_hook(hook: FutexSleepHook) {
    FUTEX_SLEEP_HOOK.store(hook as *mut (), Ordering::Release);
}

/// Registers the wake-up callback provided by scheduler.
/// Called upon scheduler initialization.
pub fn register_wake_hook(hook: WakeFn) {
    FUTEX_WAKE_HOOK.store(hook as *mut (), Ordering::Release);
}

/// Registers the current-thread handle callback provided by scheduler.
/// Called upon scheduler initialization.
pub fn register_tid_hook(hook: FutexTidHook) {
    FUTEX_TID_HOOK.store(hook as *mut (), Ordering::Release);
}

/// Returns registered sleep hook, or None if undefined.
fn get_sleep_hook() -> Option<FutexSleepHook> {
    let ptr = FUTEX_SLEEP_HOOK.load(Ordering::Acquire);
    if ptr.is_null() {
        None
    } else {
        // SAFETY: Stored via register_sleep_hook with matching FutexSleepHook signature.
        Some(unsafe { core::mem::transmute::<*mut (), FutexSleepHook>(ptr) })
    }
}

/// Returns registered wake hook, or None if undefined.
fn get_wake_hook() -> Option<WakeFn> {
    let ptr = FUTEX_WAKE_HOOK.load(Ordering::Acquire);
    if ptr.is_null() {
        None
    } else {
        // SAFETY: Stored via register_wake_hook with matching WakeFn signature.
        Some(unsafe { core::mem::transmute::<*mut (), WakeFn>(ptr) })
    }
}

/// Returns registered current-thread handle hook, or None if undefined.
fn get_tid_hook() -> Option<FutexTidHook> {
    let ptr = FUTEX_TID_HOOK.load(Ordering::Acquire);
    if ptr.is_null() {
        None
    } else {
        // SAFETY: Stored via register_tid_hook with matching FutexTidHook signature.
        Some(unsafe { core::mem::transmute::<*mut (), FutexTidHook>(ptr) })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Noop wake (used by sys_futex before scheduler hook registration)
// ─────────────────────────────────────────────────────────────────────────────

fn nop_wake_sys(_tid: u64, _code: i32) {}

// ─────────────────────────────────────────────────────────────────────────────
// sys_futex
// ─────────────────────────────────────────────────────────────────────────────

/// Entry point of `futex(2)` syscall.
///
/// - `uaddr`: user address of 32-bit integer to operate on.
/// - `op`: operation (FUTEX_WAIT/WAKE/REQUEUE + optionally PRIVATE/CLOCK_REALTIME).
/// - `val`: expected value (FUTEX_WAIT) or thread count (FUTEX_WAKE).
/// - `timeout`: timeout in ns (FUTEX_WAIT), or max_requeue (FUTEX_REQUEUE/CMP_REQUEUE).
/// - `uaddr2`: destination address (FUTEX_REQUEUE/CMP_REQUEUE).
/// - `val3`: max_requeue wake count (FUTEX_REQUEUE) or expected value (FUTEX_CMP_REQUEUE).
///
/// Returns number of woken threads (FUTEX_WAKE/REQUEUE) or 0 (FUTEX_WAIT).
#[inline]
fn resolve_user_futex_key(uaddr: u64, is_private: bool) -> Result<(u64, u64), FutexError> {
    if uaddr == 0
        || !uaddr.is_multiple_of(4)
        || !crate::memory::core::address::is_user_canonical(uaddr)
        || uaddr > crate::syscall::validation::USER_ADDR_MAX - 4
    {
        return Err(FutexError::InvalidAddr);
    }
    let (_, phys) = crate::syscall::validation::resolve_user_read_u32_and_phys(uaddr)
        .map_err(|_| FutexError::InvalidAddr)?;
    if is_private {
        let pid = crate::syscall::validation::current_user_pid().unwrap_or(0);
        Ok((private_space_id(pid), uaddr))
    } else {
        Ok((SHARED_SPACE_ID, phys))
    }
}

/// Entry point of syscall `futex(uaddr, op, val, timeout, uaddr2, val3)`.
///
/// - `uaddr`: futex address (user, 4-byte aligned, canonical < USER_ADDR_MAX).
/// - `op`: operation (FUTEX_WAIT/WAKE/REQUEUE + optionally PRIVATE/CLOCK_REALTIME).
/// - `val`: expected value (FUTEX_WAIT) or thread count (FUTEX_WAKE).
/// - `timeout`: timeout in ns (FUTEX_WAIT), or max_requeue (FUTEX_REQUEUE/CMP_REQUEUE).
/// - `uaddr2`: destination address (FUTEX_REQUEUE/CMP_REQUEUE).
/// - `val3`: bitset mask (WAIT/WAKE_BITSET) or expected value (FUTEX_CMP_REQUEUE).
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
    let is_private = (op & FUTEX_PRIVATE_FLAG) != 0;
    let cmd = op & !(FUTEX_PRIVATE_FLAG | FUTEX_CLOCK_REALTIME);

    match cmd {
        // ── FUTEX_WAIT / FUTEX_WAIT_BITSET ────────────────────────────────
        FUTEX_WAIT | FUTEX_WAIT_BITSET => {
            if cmd == FUTEX_WAIT_BITSET && val3 == 0 {
                return Err(FutexError::InvalidOp);
            }
            let (space_id, key_addr) = resolve_user_futex_key(uaddr, is_private)?;
            let tid = get_tid_hook().map(|f| f()).unwrap_or(0);
            let wake_fn = get_wake_hook().unwrap_or(nop_wake_sys);
            // Allocate waiter on current thread stack.
            let mut waiter = FutexWaiter::new(key_addr, val, tid, wake_fn);

            // SAFETY: uaddr is validated; waiter lives on stack until cancelled or woken.
            let result = unsafe {
                futex_wait_in_space(space_id, key_addr, uaddr, true, val, &mut waiter, wake_fn)
            }?;

            match result {
                FutexWaitResult::ValueMismatch => Err(FutexError::ValueMismatch),
                FutexWaitResult::Waiting => {
                    match get_sleep_hook() {
                        Some(sleep) => {
                            let rc = sleep(&mut waiter, timeout);
                            if rc == -4 {
                                // SAFETY: waiter must be unlinked before stack frame unwinds.
                                unsafe { futex_cancel(&mut waiter) };
                                Err(FutexError::Interrupted)
                            } else if rc == -110 {
                                // SAFETY: waiter must be unlinked before stack frame unwinds.
                                unsafe { futex_cancel(&mut waiter) };
                                Err(FutexError::Timeout)
                            } else if !waiter.woken.load(Ordering::Acquire) {
                                // Defense against unexpected sleep return without wake: never leave
                                // stack `waiter` linked inside `FUTEX_TABLE`.
                                unsafe { futex_cancel(&mut waiter) };
                                Err(FutexError::Interrupted)
                            } else {
                                Ok(0)
                            }
                        }
                        None => {
                            // No scheduler: cancel and return EINTR.
                            unsafe { futex_cancel(&mut waiter) };
                            Err(FutexError::Interrupted)
                        }
                    }
                }
            }
        }

        // ── FUTEX_WAKE / FUTEX_WAKE_BITSET ────────────────────────────────
        FUTEX_WAKE | FUTEX_WAKE_BITSET => {
            if cmd == FUTEX_WAKE_BITSET && val3 == 0 {
                return Err(FutexError::InvalidOp);
            }
            let (space_id, key_addr) = resolve_user_futex_key(uaddr, is_private)?;
            // SAFETY: key_addr belongs to a validated user mapping in `space_id`.
            let woken = unsafe { futex_wake_in_space(space_id, key_addr, val, 0) };
            Ok(woken as i64)
        }

        // ── FUTEX_REQUEUE / FUTEX_CMP_REQUEUE ─────────────────────────────
        FUTEX_REQUEUE | FUTEX_CMP_REQUEUE => {
            let (src_space, src_key) = resolve_user_futex_key(uaddr, is_private)?;
            let (dst_space, dst_key) = resolve_user_futex_key(uaddr2, is_private)?;
            let cmp_expected = if cmd == FUTEX_CMP_REQUEUE {
                Some((uaddr, val3, true))
            } else {
                None
            };
            let max_wake = val;
            let max_requeue = timeout as u32;
            // SAFETY: keys belong to validated mappings in their respective spaces.
            let (woken, _requeued) = unsafe {
                futex_requeue_in_space(
                    src_space,
                    src_key,
                    dst_space,
                    dst_key,
                    cmp_expected,
                    max_wake,
                    max_requeue,
                    0,
                )
            }?;
            Ok(woken as i64)
        }

        _ => Err(FutexError::InvalidOp),
    }
}
