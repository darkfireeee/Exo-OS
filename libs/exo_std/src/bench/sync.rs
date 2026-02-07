//! Synchronization primitives benchmarks
//!
//! Benchmarks for Mutex, RwLock, Futex, and other sync primitives.

use crate::bench::Benchmark;
use crate::sync::{Mutex, FutexMutex, Semaphore, FutexSemaphore};
use crate::println;

/// Benchmark Mutex lock/unlock (uncontended)
pub fn bench_mutex_uncontended() {
    let mutex = Mutex::new(0u64);

    let _result = Benchmark::new("Mutex uncontended")
        .iterations(10000)
        .run(|| {
            let mut guard = mutex.lock().unwrap();
            *guard += 1;
        });

    #[cfg(feature = "test_mode")]
    {
        println!("Mutex uncontended: {} ops, avg {}ns",
            _result.iterations, _result.avg_nanos());
    }
}

/// Benchmark FutexMutex lock/unlock (uncontended)
pub fn bench_futex_mutex_uncontended() {
    let mutex = FutexMutex::new();

    let _result = Benchmark::new("FutexMutex uncontended")
        .iterations(10000)
        .run(|| {
            mutex.lock();
            mutex.unlock();
        });

    #[cfg(feature = "test_mode")]
    {
        println!("FutexMutex uncontended: {} ops, avg {}ns",
            _result.iterations, _result.avg_nanos());
    }
}

/// Benchmark Semaphore acquire/release
pub fn bench_semaphore() {
    let sem = Semaphore::new(1);

    let _result = Benchmark::new("Semaphore")
        .iterations(10000)
        .run(|| {
            sem.acquire();
            sem.release();
        });

    #[cfg(feature = "test_mode")]
    {
        println!("Semaphore: {} ops, avg {}ns",
            _result.iterations, _result.avg_nanos());
    }
}

/// Benchmark FutexSemaphore try_acquire
pub fn bench_futex_semaphore() {
    let sem = FutexSemaphore::new(1);

    let _result = Benchmark::new("FutexSemaphore")
        .iterations(10000)
        .run(|| {
            if sem.try_acquire() {
                sem.release();
            }
        });

    #[cfg(feature = "test_mode")]
    {
        println!("FutexSemaphore: {} ops, avg {}ns",
            _result.iterations, _result.avg_nanos());
    }
}

/// Run all sync benchmarks
pub fn run_all() {
    bench_mutex_uncontended();
    bench_futex_mutex_uncontended();
    bench_semaphore();
    bench_futex_semaphore();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bench_mutex() {
        bench_mutex_uncontended();
    }

    #[test]
    fn test_bench_futex_mutex() {
        bench_futex_mutex_uncontended();
    }
}
