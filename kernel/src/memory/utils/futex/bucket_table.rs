//! Futex waiter node, intrusive FIFO hash buckets, SipHash-1-3 keying, and global table.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use spin::Mutex;

use crate::memory::core::constants::FUTEX_HASH_BUCKETS;

use super::syscall_ops::FutexError;

/// Bitmask matching all futex bitset waiters (POSIX/Linux default).
pub const FUTEX_BITSET_MATCH_ANY: u32 = 0xFFFF_FFFF;

/// Signature of the wake-up function provided by the scheduler.
pub type WakeFn = fn(tid: u64, code: i32);

#[allow(dead_code)]
fn nop_wake(_tid: u64, _code: i32) {}

/// A thread waiting on a futex address.
#[repr(C)]
pub struct FutexWaiter {
    pub virt_addr: AtomicU64,
    pub space_id: AtomicU64,
    pub expected_val: u32,
    pub wake_code: i32,
    pub tid: u64,
    pub wake_fn: WakeFn,
    pub next: Option<core::ptr::NonNull<FutexWaiter>>,
    pub woken: AtomicBool,
    _pad: [u8; 3],
    pub bitset: u32,
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
            _pad: [0; 3],
            bitset: FUTEX_BITSET_MATCH_ANY,
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

pub(super) struct BucketInner {
    pub(super) head: Option<core::ptr::NonNull<FutexWaiter>>,
    pub(super) tail: Option<core::ptr::NonNull<FutexWaiter>>,
    pub(super) count: u32,
}

unsafe impl Send for BucketInner {}
unsafe impl Sync for BucketInner {}

pub(super) type WakeItem = (u64, WakeFn, i32);

impl BucketInner {
    const fn new() -> Self {
        Self {
            head: None,
            tail: None,
            count: 0,
        }
    }

    pub(super) unsafe fn push(&mut self, waiter: *mut FutexWaiter) {
        let Some(nn) = core::ptr::NonNull::new(waiter) else {
            return;
        };
        (*waiter).next = None;
        if let Some(tail) = self.tail {
            (*tail.as_ptr()).next = Some(nn);
        } else {
            self.head = Some(nn);
        }
        self.tail = Some(nn);
        self.count += 1;
    }

    pub(super) unsafe fn remove(&mut self, waiter: *mut FutexWaiter) -> bool {
        let mut prev_node: Option<core::ptr::NonNull<FutexWaiter>> = None;
        let mut cur = self.head;
        while let Some(node) = cur {
            let next = (*node.as_ptr()).next;
            if node.as_ptr() == waiter {
                match prev_node {
                    Some(prev) => (*prev.as_ptr()).next = next,
                    None => self.head = next,
                }
                if self.tail == Some(node) {
                    self.tail = prev_node;
                }
                (*node.as_ptr()).next = None;
                self.count = self.count.saturating_sub(1);
                return true;
            }
            prev_node = Some(node);
            cur = next;
        }
        false
    }

    pub(super) unsafe fn collect_wake(
        &mut self,
        space_id: u64,
        virt_addr: u64,
        bitset_mask: u32,
        max: u32,
        wake_code: i32,
        out: &mut Vec<WakeItem>,
    ) -> u32 {
        let mut collected = 0u32;
        let mut prev_node: Option<core::ptr::NonNull<FutexWaiter>> = None;
        let mut cur = self.head;

        while collected < max {
            let Some(node) = cur else {
                break;
            };
            let w = node.as_ptr();
            let next = (*w).next;

            if (*w).space_id.load(Ordering::Acquire) == space_id
                && (*w).virt_addr.load(Ordering::Acquire) == virt_addr
                && !(*w).woken.load(Ordering::Acquire)
                && ((*w).bitset & bitset_mask) != 0
            {
                match prev_node {
                    Some(prev) => (*prev.as_ptr()).next = next,
                    None => self.head = next,
                }
                if self.tail == Some(node) {
                    self.tail = prev_node;
                }
                (*w).next = None;
                self.count = self.count.saturating_sub(1);

                (*w).wake_code = wake_code;
                (*w).woken.store(true, Ordering::Release);
                out.push(((*w).tid, (*w).wake_fn, wake_code));
                collected += 1;
                cur = next;
            } else {
                prev_node = Some(node);
                cur = next;
            }
        }
        collected
    }

    pub(super) unsafe fn requeue_to(
        &mut self,
        src_space: u64,
        src_addr: u64,
        dst_space: u64,
        dst_addr: u64,
        dst_inner: &mut BucketInner,
        max_requeue: u32,
    ) -> u32 {
        let mut requeued = 0u32;
        let mut prev_node: Option<core::ptr::NonNull<FutexWaiter>> = None;
        let mut cur = self.head;

        while requeued < max_requeue {
            let Some(node) = cur else {
                break;
            };
            let w = node.as_ptr();
            let next = (*w).next;

            if (*w).space_id.load(Ordering::Acquire) == src_space
                && (*w).virt_addr.load(Ordering::Acquire) == src_addr
                && !(*w).woken.load(Ordering::Acquire)
            {
                match prev_node {
                    Some(prev) => (*prev.as_ptr()).next = next,
                    None => self.head = next,
                }
                if self.tail == Some(node) {
                    self.tail = prev_node;
                }
                (*w).next = None;
                self.count = self.count.saturating_sub(1);

                (*w).space_id.store(dst_space, Ordering::Release);
                (*w).virt_addr.store(dst_addr, Ordering::Release);
                dst_inner.push(w);

                requeued += 1;
                cur = next;
            } else {
                prev_node = Some(node);
                cur = next;
            }
        }
        requeued
    }

    pub(super) unsafe fn requeue_inplace(
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
    pub(super) inner: Mutex<BucketInner>,
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

    #[inline]
    pub fn count(&self) -> u32 {
        self.inner.lock().count
    }
}

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

/// Futex hash table — global singleton (`FUTEX_HASH_BUCKETS = 4096`).
pub struct FutexHashTable {
    pub(super) buckets: [FutexBucket; FUTEX_HASH_BUCKETS],
}

unsafe impl Sync for FutexHashTable {}

pub static FUTEX_TABLE: FutexHashTable = unsafe { core::mem::zeroed() };

static FUTEX_SEED_K0: AtomicU64 = AtomicU64::new(0);
static FUTEX_SEED_K1: AtomicU64 = AtomicU64::new(0);
static FUTEX_SEED_SET: AtomicU8 = AtomicU8::new(0);

/// Initializes SipHash seed from `security::crypto::rng` (boot step 18).
pub fn init_futex_seed(key: [u8; 16]) {
    let k0 = u64::from_le_bytes(key[0..8].try_into().unwrap_or([0u8; 8]));
    let k1 = u64::from_le_bytes(key[8..16].try_into().unwrap_or([0u8; 8]));
    FUTEX_SEED_K0.store(k0, Ordering::Release);
    FUTEX_SEED_K1.store(k1, Ordering::Release);
    FUTEX_SEED_SET.store(1, Ordering::Release);
}

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

    let m = data;
    v3 ^= m;
    sipround!(v0, v1, v2, v3);
    v0 ^= m;

    let b: u64 = 0x0800_0000_0000_0000;
    v3 ^= b;
    sipround!(v0, v1, v2, v3);
    v0 ^= b;

    v2 ^= 0xff;
    sipround!(v0, v1, v2, v3);
    sipround!(v0, v1, v2, v3);
    sipround!(v0, v1, v2, v3);

    v0 ^ v1 ^ v2 ^ v3
}

pub(super) const KERNEL_SPACE_ID: u64 = 0;
pub(super) const SHARED_SPACE_ID: u64 = 1;
const PRIVATE_SPACE_BIT: u64 = 1u64 << 63;

#[inline]
pub(super) fn private_space_id(pid: u32) -> u64 {
    PRIVATE_SPACE_BIT | (pid as u64)
}

#[inline]
pub(super) fn bucket_index(space_id: u64, virt_addr: u64) -> usize {
    if FUTEX_SEED_SET.load(Ordering::Acquire) != 0 {
        let k0 = FUTEX_SEED_K0.load(Ordering::Relaxed) ^ space_id;
        let k1 = FUTEX_SEED_K1.load(Ordering::Relaxed) ^ space_id.rotate_right(29);
        (siphash13(k0, k1, virt_addr) as usize) & (FUTEX_HASH_BUCKETS - 1)
    } else {
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
pub(super) unsafe fn read_futex_word(virt_addr: u64, is_user: bool) -> Result<(u32, u64), FutexError> {
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
