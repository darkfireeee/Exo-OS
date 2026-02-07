//! Waker implementation for async tasks
//!
//! Wakers notify the executor when a task is ready to make progress.
//! This implementation uses a simple channel-based notification system.

extern crate alloc;

use alloc::sync::Arc;
use alloc::task::Wake;
use core::task::{RawWaker, RawWakerVTable, Waker as CoreWaker};
use crate::async_rt::task::TaskId;

/// Waker that notifies the executor when a task is ready
///
/// When a waker is invoked (via wake()), it sends the task ID
/// to the executor's wake queue.
pub struct Waker {
    task_id: TaskId,
    wake_fn: fn(TaskId),
}

impl Waker {
    /// Create a new waker for a task
    pub fn new(task_id: TaskId, wake_fn: fn(TaskId)) -> Self {
        Self { task_id, wake_fn }
    }

    /// Wake the associated task
    pub fn wake(&self) {
        (self.wake_fn)(self.task_id);
    }

    /// Convert to a core::task::Waker
    pub fn into_core_waker(self) -> CoreWaker {
        let arc_waker = Arc::new(self);
        unsafe { CoreWaker::from_raw(waker_to_raw_waker(Arc::into_raw(arc_waker))) }
    }
}

impl Wake for Waker {
    fn wake(self: Arc<Self>) {
        self.as_ref().wake();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.as_ref().wake();
    }
}

/// Convert Arc<Waker> pointer to RawWaker
unsafe fn waker_to_raw_waker(ptr: *const Waker) -> RawWaker {
    RawWaker::new(ptr as *const (), &WAKER_VTABLE)
}

/// RawWaker vtable for our Waker type
const WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
    waker_clone,
    waker_wake,
    waker_wake_by_ref,
    waker_drop,
);

unsafe fn waker_clone(ptr: *const ()) -> RawWaker {
    let arc = Arc::from_raw(ptr as *const Waker);
    let cloned = arc.clone();
    core::mem::forget(arc); // Don't drop original
    waker_to_raw_waker(Arc::into_raw(cloned))
}

unsafe fn waker_wake(ptr: *const ()) {
    let arc = Arc::from_raw(ptr as *const Waker);
    Wake::wake(arc); // Consumes the arc
}

unsafe fn waker_wake_by_ref(ptr: *const ()) {
    let arc = Arc::from_raw(ptr as *const Waker);
    Wake::wake_by_ref(&arc);
    core::mem::forget(arc); // Don't drop
}

unsafe fn waker_drop(ptr: *const ()) {
    drop(Arc::from_raw(ptr as *const Waker));
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicBool, Ordering};

    static WOKEN: AtomicBool = AtomicBool::new(false);

    fn test_wake_fn(task_id: TaskId) {
        WOKEN.store(true, Ordering::SeqCst);
        assert!(task_id.as_u64() > 0);
    }

    #[test]
    fn test_waker_wake() {
        WOKEN.store(false, Ordering::SeqCst);
        let task_id = TaskId::new();
        let waker = Waker::new(task_id, test_wake_fn);

        waker.wake();
        assert!(WOKEN.load(Ordering::SeqCst));
    }

    #[test]
    fn test_core_waker_conversion() {
        let task_id = TaskId::new();
        let waker = Waker::new(task_id, test_wake_fn);
        let core_waker = waker.into_core_waker();

        // Should not panic
        core_waker.wake_by_ref();
    }
}
