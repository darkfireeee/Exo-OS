pub type Spinlock<T> = spin::Mutex<T>;
pub type SpinlockGuard<'a, T> = spin::MutexGuard<'a, T>;
