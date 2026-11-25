//! Unit tests for `memory::utils::futex`.

use alloc::vec::Vec;
use spin::Mutex;

use super::bucket_table::{FutexWaiter, KERNEL_SPACE_ID};
use super::syscall_ops::FutexError;
use super::wait_wake::{
    check_expired_wait, futex_requeue_in_space, futex_wait, futex_wait_in_space_bitset,
    futex_wake, futex_wake_in_space_bitset, FutexWaitResult,
};

static WAKE_LOG: Mutex<Vec<u64>> = Mutex::new(Vec::new());

fn record_wake(tid: u64, _code: i32) {
    WAKE_LOG.lock().push(tid);
}

#[test]
fn futex_fifo_order_and_bitset_filtering_work() {
    let word: u32 = 42;
    let addr = core::ptr::addr_of!(word) as u64;
    WAKE_LOG.lock().clear();

    let mut w1 = FutexWaiter::new(addr, 42, 101, record_wake);
    let mut w2 = FutexWaiter::new(addr, 42, 102, record_wake);
    let mut w3 = FutexWaiter::new(addr, 42, 103, record_wake);

    unsafe {
        assert_eq!(
            futex_wait_in_space_bitset(
                KERNEL_SPACE_ID,
                addr,
                addr,
                false,
                42,
                0b0001,
                &mut w1,
                record_wake,
            ),
            Ok(FutexWaitResult::Waiting)
        );
        assert_eq!(
            futex_wait_in_space_bitset(
                KERNEL_SPACE_ID,
                addr,
                addr,
                false,
                42,
                0b0010,
                &mut w2,
                record_wake,
            ),
            Ok(FutexWaitResult::Waiting)
        );
        assert_eq!(
            futex_wait_in_space_bitset(
                KERNEL_SPACE_ID,
                addr,
                addr,
                false,
                42,
                0b0001,
                &mut w3,
                record_wake,
            ),
            Ok(FutexWaitResult::Waiting)
        );

        let n = futex_wake_in_space_bitset(KERNEL_SPACE_ID, addr, 0b0010, 10, 0);
        assert_eq!(n, 1);
        let n2 = futex_wake_in_space_bitset(KERNEL_SPACE_ID, addr, 0b0001, 1, 0);
        assert_eq!(n2, 1);
        let n3 = futex_wake_in_space_bitset(KERNEL_SPACE_ID, addr, 0b0001, 1, 0);
        assert_eq!(n3, 1);
    }

    assert_eq!(*WAKE_LOG.lock(), alloc::vec![102, 101, 103]);
}

#[test]
fn futex_requeue_moves_excess_waiters_and_reports_combined_count() {
    let src_word: u32 = 7;
    let dst_word: u32 = 9;
    let src = core::ptr::addr_of!(src_word) as u64;
    let dst = core::ptr::addr_of!(dst_word) as u64;
    WAKE_LOG.lock().clear();

    let mut w1 = FutexWaiter::new(src, 7, 201, record_wake);
    let mut w2 = FutexWaiter::new(src, 7, 202, record_wake);
    let mut w3 = FutexWaiter::new(src, 7, 203, record_wake);

    unsafe {
        assert_eq!(futex_wait(src, 7, &mut w1, record_wake), FutexWaitResult::Waiting);
        assert_eq!(futex_wait(src, 7, &mut w2, record_wake), FutexWaitResult::Waiting);
        assert_eq!(futex_wait(src, 7, &mut w3, record_wake), FutexWaitResult::Waiting);

        let (woken, requeued) = futex_requeue_in_space(
            KERNEL_SPACE_ID,
            src,
            KERNEL_SPACE_ID,
            dst,
            Some((src, 7, false)),
            1,
            2,
            0,
        )
        .expect("requeue");
        assert_eq!(woken, 1);
        assert_eq!(requeued, 2);
        assert_eq!(*WAKE_LOG.lock(), alloc::vec![201]);

        let rem = futex_wake(dst, 10, 0);
        assert_eq!(rem, 2);
        assert_eq!(*WAKE_LOG.lock(), alloc::vec![201, 202, 203]);
    }
}

#[test]
fn expired_wait_check_distinguishes_timeout_and_mismatch() {
    let val: u32 = 55;
    let addr = core::ptr::addr_of!(val) as u64;
    assert_eq!(check_expired_wait(addr, false, 55), FutexError::Timeout);
    assert_eq!(check_expired_wait(addr, false, 99), FutexError::ValueMismatch);
}
