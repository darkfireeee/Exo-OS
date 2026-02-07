//! Task abstraction for async execution
//!
//! Tasks are the unit of async execution in the runtime. Each task
//! wraps a Future and tracks its state and ID.

extern crate alloc;

use alloc::boxed::Box;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};
use core::sync::atomic::{AtomicU64, Ordering};

/// Unique identifier for a task
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskId(u64);

impl TaskId {
    /// Generate a new unique task ID
    pub fn new() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        TaskId(NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }

    /// Get the raw ID value
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl Default for TaskId {
    fn default() -> Self {
        Self::new()
    }
}

/// A spawned async task
///
/// This type wraps a future and provides an interface for the executor
/// to poll it.
pub struct Task {
    /// Unique task identifier
    id: TaskId,
    /// The future to execute
    future: Pin<Box<dyn Future<Output = ()> + 'static>>,
}

impl Task {
    /// Create a new task from a future
    pub fn new<F>(future: F) -> Self
    where
        F: Future<Output = ()> + 'static,
    {
        Self {
            id: TaskId::new(),
            future: Box::pin(future),
        }
    }

    /// Get the task ID
    pub fn id(&self) -> TaskId {
        self.id
    }

    /// Poll the task's future
    ///
    /// Returns Poll::Ready(()) when complete, Poll::Pending otherwise.
    pub fn poll(&mut self, context: &mut Context<'_>) -> Poll<()> {
        self.future.as_mut().poll(context)
    }
}

/// Handle to a spawned task that can be awaited
///
/// Currently a placeholder - full implementation would allow
/// awaiting task completion and retrieving results.
pub struct JoinHandle<T> {
    task_id: TaskId,
    _phantom: core::marker::PhantomData<T>,
}

impl<T> JoinHandle<T> {
    /// Create a new join handle
    pub(crate) fn new(task_id: TaskId) -> Self {
        Self {
            task_id,
            _phantom: core::marker::PhantomData,
        }
    }

    /// Get the task ID
    pub fn task_id(&self) -> TaskId {
        self.task_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_id_unique() {
        let id1 = TaskId::new();
        let id2 = TaskId::new();
        assert_ne!(id1, id2);
        assert_eq!(id1.as_u64() + 1, id2.as_u64());
    }

    #[test]
    fn test_task_creation() {
        async fn test_future() {
            // Simple async function
        }

        let task = Task::new(test_future());
        let id = task.id();
        assert!(id.as_u64() > 0);
    }
}
