//! Single-threaded async executor
//!
//! Provides a simple executor that polls tasks to completion.
//! Tasks are stored in a queue and polled in round-robin fashion.

extern crate alloc;

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::future::Future;
use core::task::{Context, Poll};
use crate::async_rt::task::{Task, TaskId, JoinHandle};
use crate::async_rt::waker::Waker;
use crate::collections::HashMap;

/// Single-threaded async task executor
///
/// The executor maintains a queue of ready tasks and polls them
/// until they complete. When a task returns Poll::Pending, it's
/// removed from the ready queue until its waker is invoked.
pub struct Executor {
    /// Queue of tasks ready to be polled
    ready_queue: VecDeque<Task>,
    /// Map of all tasks by ID (including pending ones)
    tasks: HashMap<u64, Task>,
    /// Queue of task IDs that have been woken
    wake_queue: VecDeque<TaskId>,
}

impl Executor {
    /// Create a new executor
    pub fn new() -> Self {
        Self {
            ready_queue: VecDeque::new(),
            tasks: HashMap::new(),
            wake_queue: VecDeque::new(),
        }
    }

    /// Spawn a new task on this executor
    ///
    /// The task will be added to the ready queue and polled
    /// on the next executor tick.
    pub fn spawn<F>(&mut self, future: F) -> JoinHandle<()>
    where
        F: Future<Output = ()> + 'static,
    {
        let task = Task::new(future);
        let task_id = task.id();

        self.ready_queue.push_back(task);

        JoinHandle::new(task_id)
    }

    /// Run the executor until all tasks complete
    ///
    /// This polls tasks from the ready queue until the queue is empty
    /// and no tasks are pending wake-up.
    pub fn run(&mut self) {
        loop {
            // Process all woken tasks
            while let Some(task_id) = self.wake_queue.pop_front() {
                if let Some(task) = self.tasks.remove(&task_id.as_u64()) {
                    self.ready_queue.push_back(task);
                }
            }

            // Poll ready tasks
            if let Some(mut task) = self.ready_queue.pop_front() {
                let task_id = task.id();

                // Create waker for this task
                let waker = Waker::new(task_id, wake_task);
                let core_waker = waker.into_core_waker();
                let mut context = Context::from_waker(&core_waker);

                match task.poll(&mut context) {
                    Poll::Ready(()) => {
                        // Task completed, don't re-queue
                    }
                    Poll::Pending => {
                        // Task not ready, store it for later
                        self.tasks.insert(task_id.as_u64(), task);
                    }
                }
            } else if self.tasks.is_empty() && self.wake_queue.is_empty() {
                // No more tasks to run
                break;
            } else {
                // No ready tasks, but some are pending - yield CPU
                #[cfg(not(feature = "test_mode"))]
                unsafe {
                    crate::syscall::syscall0(crate::syscall::SyscallNumber::ThreadYield);
                }
            }
        }
    }

    /// Run until a specific future completes
    ///
    /// This polls the future in a loop until it completes,
    /// returning its output.
    pub fn block_on<F>(&mut self, future: F) -> F::Output
    where
        F: Future + 'static,
        F::Output: 'static,
    {
        use core::pin::Pin;
        use alloc::boxed::Box;

        let mut future = Box::pin(future);

        loop {
            // Create a dummy waker since we're blocking
            let waker = Waker::new(TaskId::new(), |_| {});
            let core_waker = waker.into_core_waker();
            let mut context = Context::from_waker(&core_waker);

            match future.as_mut().poll(&mut context) {
                Poll::Ready(output) => return output,
                Poll::Pending => {
                    // Run other tasks while waiting
                    if self.has_tasks() {
                        // Process one task
                        if let Some(mut task) = self.ready_queue.pop_front() {
                            let task_id = task.id();
                            let task_waker = Waker::new(task_id, wake_task);
                            let task_core_waker = task_waker.into_core_waker();
                            let mut task_context = Context::from_waker(&task_core_waker);

                            match task.poll(&mut task_context) {
                                Poll::Ready(()) => {
                                    // Task completed
                                }
                                Poll::Pending => {
                                    // Store for later
                                    self.tasks.insert(task_id.as_u64(), task);
                                }
                            }
                        }
                    } else {
                        // Yield CPU
                        #[cfg(not(feature = "test_mode"))]
                        unsafe {
                            crate::syscall::syscall0(crate::syscall::SyscallNumber::ThreadYield);
                        }
                    }
                }
            }
        }
    }

    /// Get the number of ready tasks
    pub fn ready_count(&self) -> usize {
        self.ready_queue.len()
    }

    /// Get the number of pending tasks
    pub fn pending_count(&self) -> usize {
        self.tasks.len()
    }

    /// Check if executor has any tasks
    pub fn has_tasks(&self) -> bool {
        !self.ready_queue.is_empty() || !self.tasks.is_empty() || !self.wake_queue.is_empty()
    }
}

impl Default for Executor {
    fn default() -> Self {
        Self::new()
    }
}

/// Global wake queue for the executor
///
/// In a real implementation, this would use thread-local storage.
/// For simplicity, we use a static mutex-protected queue.
static WAKE_QUEUE: crate::sync::Mutex<VecDeque<TaskId>> = crate::sync::Mutex::new(VecDeque::new());

/// Wake function called by wakers
fn wake_task(task_id: TaskId) {
    let mut queue = WAKE_QUEUE.lock().unwrap();
    queue.push_back(task_id);
}

/// Spawn a task on the global executor
///
/// Note: This requires a global executor instance. In practice,
/// you would create an executor and call its methods directly.
pub fn spawn<F>(future: F) -> JoinHandle<()>
where
    F: Future<Output = ()> + 'static,
{
    // This is a placeholder - real implementation would use
    // a global executor or thread-local executor
    let mut executor = Executor::new();
    executor.spawn(future)
}

/// Block on a future until it completes
///
/// Creates a temporary executor and runs it until the future completes.
pub fn block_on<F>(future: F) -> F::Output
where
    F: Future + 'static,
    F::Output: 'static,
{
    let mut executor = Executor::new();
    executor.block_on(future)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::pin::Pin;
    use core::task::{Context, Poll};

    struct YieldOnce {
        yielded: bool,
    }

    impl Future for YieldOnce {
        type Output = ();

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            if self.yielded {
                Poll::Ready(())
            } else {
                self.yielded = true;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }

    #[test]
    fn test_executor_creation() {
        let executor = Executor::new();
        assert_eq!(executor.ready_count(), 0);
        assert_eq!(executor.pending_count(), 0);
        assert!(!executor.has_tasks());
    }

    #[test]
    fn test_executor_spawn() {
        let mut executor = Executor::new();

        async fn test_future() {
            // Simple future
        }

        executor.spawn(test_future());
        assert!(executor.has_tasks());
    }

    #[test]
    fn test_executor_run_simple() {
        let mut executor = Executor::new();

        async fn test_future() {
            // Completes immediately
        }

        executor.spawn(test_future());
        executor.run();

        assert!(!executor.has_tasks());
    }

    #[test]
    fn test_executor_yield() {
        let mut executor = Executor::new();

        let future = YieldOnce { yielded: false };
        executor.spawn(future);
        executor.run();

        assert!(!executor.has_tasks());
    }

    #[test]
    fn test_block_on() {
        async fn compute() -> u32 {
            42
        }

        let result = block_on(compute());
        assert_eq!(result, 42);
    }

    #[test]
    fn test_multiple_tasks() {
        let mut executor = Executor::new();

        for _ in 0..10 {
            executor.spawn(async {
                // Do nothing
            });
        }

        assert_eq!(executor.ready_count(), 10);
        executor.run();
        assert!(!executor.has_tasks());
    }
}
