//! Futex wait, cancel, wake, clear_child_tid, and requeue queue operations.

use alloc::vec::Vec;
use core::sync::atomic::Ordering;

use super::bucket_table::{
    bucket_index, private_space_id, read_futex_word, BucketInner, FutexWaiter, WakeFn,
    FUTEX_BITSET_MATCH_ANY, FUTEX_STATS, FUTEX_TABLE, KERNEL_SPACE_ID, SHARED_SPACE_ID,
};
use super::syscall_ops::FutexError;

/// Result of `futex_wait`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FutexWaitResult {
    Waiting,
    ValueMismatch,
}

pub(super) unsafe fn futex_wait_in_space_bitset(
    space_id: u64,
    key_addr: u64,
    value_addr: u64,
    is_user: bool,
    expected: u32,
    bitset: u32,
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

    (*waiter).space_id.store(space_id, Ordering::Release);
    (*waiter).virt_addr.store(key_addr, Ordering::Release);
    (*waiter).expected_val = expected;
    (*waiter).bitset = bitset;
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

#[allow(dead_code)]
unsafe fn futex_wait_in_space(
    space_id: u64,
    key_addr: u64,
    value_addr: u64,
    is_user: bool,
    expected: u32,
    waiter: *mut FutexWaiter,
    wake_fn: WakeFn,
) -> Result<FutexWaitResult, FutexError> {
    futex_wait_in_space_bitset(
        space_id,
        key_addr,
        value_addr,
        is_user,
        expected,
        FUTEX_BITSET_MATCH_ANY,
        waiter,
        wake_fn,
    )
}

pub(super) fn check_expired_wait(value_addr: u64, is_user: bool, expected: u32) -> FutexError {
    match unsafe { read_futex_word(value_addr, is_user) } {
        Ok((cur, _)) if cur != expected => {
            FUTEX_STATS.value_mismatches.fetch_add(1, Ordering::Relaxed);
            FutexError::ValueMismatch
        }
        Ok(_) => {
            FUTEX_STATS.timeouts.fetch_add(1, Ordering::Relaxed);
            FutexError::Timeout
        }
        Err(e) => e,
    }
}

/// Enqueues a waiter on kernel-space key `virt_addr` if `*virt_addr == expected`.
pub unsafe fn futex_wait(
    virt_addr: u64,
    expected: u32,
    waiter: *mut FutexWaiter,
    wake_fn: WakeFn,
) -> FutexWaitResult {
    futex_wait_in_space_bitset(
        KERNEL_SPACE_ID,
        virt_addr,
        virt_addr,
        false,
        expected,
        FUTEX_BITSET_MATCH_ANY,
        waiter,
        wake_fn,
    )
    .unwrap_or(FutexWaitResult::ValueMismatch)
}

/// Removes a waiter from whichever bucket it currently occupies.
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

/// Wakes up to `max` threads waiting on `(space_id, key_addr)` matching `bitset_mask`.
pub unsafe fn futex_wake_in_space_bitset(
    space_id: u64,
    key_addr: u64,
    bitset_mask: u32,
    max: u32,
    wake_code: i32,
) -> u32 {
    FUTEX_STATS.wake_calls.fetch_add(1, Ordering::Relaxed);
    if max == 0 || bitset_mask == 0 {
        return 0;
    }
    let idx = bucket_index(space_id, key_addr);
    let mut wake_batch = Vec::new();
    let woken = {
        let mut bucket = FUTEX_TABLE.buckets[idx].inner.lock();
        bucket.collect_wake(
            space_id,
            key_addr,
            bitset_mask,
            max,
            wake_code,
            &mut wake_batch,
        )
    };
    for (tid, wake_fn, code) in wake_batch {
        wake_fn(tid, code);
    }
    FUTEX_STATS
        .total_woken
        .fetch_add(woken as u64, Ordering::Relaxed);
    woken
}

/// Wakes up to `max` threads waiting on `(space_id, key_addr)`.
pub unsafe fn futex_wake_in_space(
    space_id: u64,
    key_addr: u64,
    max: u32,
    wake_code: i32,
) -> u32 {
    futex_wake_in_space_bitset(space_id, key_addr, FUTEX_BITSET_MATCH_ANY, max, wake_code)
}

/// Wakes threads waiting on `clear_child_tid` (`pthread_join`).
pub unsafe fn wake_clear_child_tid(pid: u32, virt_addr: u64, phys_addr: Option<u64>) {
    let woke_priv = futex_wake_in_space(private_space_id(pid), virt_addr, 1, 0);
    if woke_priv == 0 {
        if let Some(phys) = phys_addr {
            let _ = futex_wake_in_space(SHARED_SPACE_ID, phys, 1, 0);
        }
    }
}

/// Wakes up to `max` threads waiting on kernel key `virt_addr`.
pub unsafe fn futex_wake(virt_addr: u64, max: u32, wake_code: i32) -> u32 {
    futex_wake_in_space(KERNEL_SPACE_ID, virt_addr, max, wake_code)
}

/// Wakes up to `n` threads waiting on kernel key `virt_addr`.
#[inline]
pub unsafe fn futex_wake_n(virt_addr: u64, n: u32) -> u32 {
    futex_wake(virt_addr, n, 0)
}

pub(super) unsafe fn futex_requeue_in_space(
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
    let mut wake_batch = Vec::new();

    let (woken, requeued) = if src_idx == dst_idx {
        let mut bucket = FUTEX_TABLE.buckets[src_idx].inner.lock();
        if let Some((uaddr, expected_val, is_user)) = cmp_expected {
            let (cur_val, _) = read_futex_word(uaddr, is_user)?;
            if cur_val != expected_val {
                return Err(FutexError::ValueMismatch);
            }
        }
        let woken = if max_wake > 0 {
            bucket.collect_wake(
                src_space,
                src_key,
                FUTEX_BITSET_MATCH_ANY,
                max_wake,
                wake_code,
                &mut wake_batch,
            )
        } else {
            0
        };
        let requeued = if max_requeue > 0 && (src_space, src_key) != (dst_space, dst_key) {
            bucket.requeue_inplace(src_space, src_key, dst_space, dst_key, max_requeue)
        } else {
            0
        };
        (woken, requeued)
    } else {
        let (lo, hi) = if src_idx < dst_idx {
            (src_idx, dst_idx)
        } else {
            (dst_idx, src_idx)
        };

        let lo_guard = FUTEX_TABLE.buckets[lo].inner.lock();
        let hi_guard = FUTEX_TABLE.buckets[hi].inner.lock();

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

        let woken = if max_wake > 0 {
            src_inner.collect_wake(
                src_space,
                src_key,
                FUTEX_BITSET_MATCH_ANY,
                max_wake,
                wake_code,
                &mut wake_batch,
            )
        } else {
            0
        };
        let requeued = if max_requeue > 0 && (src_space, src_key) != (dst_space, dst_key) {
            src_inner.requeue_to(src_space, src_key, dst_space, dst_key, dst_inner, max_requeue)
        } else {
            0
        };
        (woken, requeued)
    };

    for (tid, wake_fn, code) in wake_batch {
        wake_fn(tid, code);
    }

    FUTEX_STATS
        .total_woken
        .fetch_add(woken as u64, Ordering::Relaxed);
    Ok((woken, requeued))
}

/// Wakes `max_wake` threads on kernel `src_addr` and requeues `max_requeue` others.
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

/// Init futex table — structure already initialized const.
pub fn init() {}
