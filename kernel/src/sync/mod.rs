pub mod mutex;
pub mod spinlock;
pub mod wait_queue;

pub use mutex::Mutex;
pub use spinlock::Spinlock;
pub use wait_queue::WaitQueue;

// Alias for compatibility
pub type SpinLock<T> = Spinlock<T>;
