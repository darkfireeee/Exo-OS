//! Tests unitaires pour exo_std
//!
//! Exécuter avec: cargo test --features test_mode

#![cfg(test)]
#![cfg(feature = "test_mode")]

use exo_std::collections::{BoundedVec, SmallVec};
use exo_std::sync::{Mutex, RwLock, Barrier, Once, AtomicCell};
use exo_std::time::Instant;
use core::sync::atomic::{AtomicU32, Ordering};

// ============================================================================
// Collections Tests
// ============================================================================

#[test]
fn test_bounded_vec_basic() {
    let mut buffer = [0u32; 10];
    let mut vec = unsafe { BoundedVec::new(buffer.as_mut_ptr(), 10) };
    
    assert_eq!(vec.len(), 0);
    assert!(vec.is_empty());
    
    vec.push(1).unwrap();
    vec.push(2).unwrap();
    vec.push(3).unwrap();
    
    assert_eq!(vec.len(), 3);
    assert_eq!(vec.first(), Some(&1));
    assert_eq!(vec.last(), Some(&3));
}

#[test]
fn test_bounded_vec_extend() {
    let mut buffer = [0u32; 10];
    let mut vec = unsafe { BoundedVec::new(buffer.as_mut_ptr(), 10) };
    
    vec.extend_from_slice(&[1, 2, 3]).unwrap();
    assert_eq!(vec.len(), 3);
    
    vec.extend_from_slice(&[4, 5]).unwrap();
    assert_eq!(vec.len(), 5);
}

#[test]
fn test_bounded_vec_drain() {
    let mut buffer = [0u32; 10];
    let mut vec = unsafe { BoundedVec::new(buffer.as_mut_ptr(), 10) };
    
    vec.extend_from_slice(&[1, 2, 3, 4, 5]).unwrap();
    
    let drained: Vec<_> = vec.drain(1..4).collect();
    assert_eq!(drained, vec![2, 3, 4]);
    assert_eq!(vec.len(), 2);
}

#[test]
fn test_bounded_vec_retain() {
    let mut buffer = [0u32; 10];
    let mut vec = unsafe { BoundedVec::new(buffer.as_mut_ptr(), 10) };
    
    vec.extend_from_slice(&[1, 2, 3, 4, 5]).unwrap();
    vec.retain(|&x| x % 2 == 0);
    
    assert_eq!(vec.len(), 2);
    assert_eq!(vec.first(), Some(&2));
    assert_eq!(vec.last(), Some(&4));
}

#[test]
fn test_small_vec_inline() {
    let mut vec: SmallVec<u32, 8> = SmallVec::new();
    
    // Devrait rester inline
    for i in 0..8 {
        vec.push(i).unwrap();
    }
    
    assert_eq!(vec.len(), 8);
    assert_eq!(vec.first(), Some(&0));
    assert_eq!(vec.last(), Some(&7));
}

#[test]
fn test_small_vec_spill() {
    let mut vec: SmallVec<u32, 4> = SmallVec::new();
    
    // Rempli inline
    for i in 0..4 {
        vec.push(i).unwrap();
    }
    
    // Devrait spill to heap
    vec.push(4).unwrap();
    vec.push(5).unwrap();
    
    assert_eq!(vec.len(), 6);
    assert_eq!(vec.get(5), Some(&5));
}

// ============================================================================
// Sync Tests
// ============================================================================

#[test]
fn test_mutex_basic() {
    let m = Mutex::new(0);
    
    {
        let mut guard = m.lock().unwrap();
        *guard = 42;
    }
    
    assert_eq!(*m.lock().unwrap(), 42);
}

#[test]
fn test_mutex_multiple_locks() {
    let m = Mutex::new(0);
    
    for _ in 0..100 {
        let mut guard = m.lock().unwrap();
        *guard += 1;
    }
    
    assert_eq!(*m.lock().unwrap(), 100);
}

#[test]
fn test_rwlock_read() {
    let lock = RwLock::new(5);
    
    // Multiple readers OK
    let r1 = lock.read().unwrap();
    let r2 = lock.read().unwrap();
    
    assert_eq!(*r1, 5);
    assert_eq!(*r2, 5);
}

#[test]
fn test_rwlock_write() {
    let lock = RwLock::new(5);
    
    {
        let mut w = lock.write().unwrap();
        *w = 42;
    }
    
    assert_eq!(*lock.read().unwrap(), 42);
}

#[test]
fn test_barrier() {
    let barrier = Barrier::new(1);
    let result = barrier.wait();
    
    // Single thread = leader
    assert!(result.is_leader());
}

#[test]
fn test_once() {
    static ONCE: Once = Once::new();
    static mut COUNTER: u32 = 0;
    
    ONCE.call_once(|| unsafe {
        COUNTER += 1;
    });
    
    ONCE.call_once(|| unsafe {
        COUNTER += 1; // Ne devrait pas s'exécuter
    });
    
    unsafe {
        assert_eq!(COUNTER, 1);
    }
}

#[test]
fn test_atomic_cell() {
    let cell = AtomicCell::new(42u32);
    
    assert_eq!(cell.load(), 42);
    
    cell.store(100);
    assert_eq!(cell.load(), 100);
    
    let old = cell.swap(200);
    assert_eq!(old, 100);
    assert_eq!(cell.load(), 200);
}

// ============================================================================
// Time Tests
// ============================================================================

#[test]
fn test_instant_elapsed() {
    let start = Instant::now();
    
    // Simule un délai (en test_mode, now() retourne 0)
    let elapsed = start.elapsed();
    
    // En test_mode, elapsed devrait être Duration::ZERO
    assert_eq!(elapsed.as_nanos(), 0);
}

#[test]
fn test_instant_arithmetic() {
    use core::time::Duration;
    
    let t1 = Instant::now();
    let t2 = t1 + Duration::from_secs(5);
    let t3 = t2 - Duration::from_secs(2);
    
    let diff = t2 - t1;
    assert_eq!(diff.as_secs(), 5);
    
    let diff2 = t2 - t3;
    assert_eq!(diff2.as_secs(), 2);
}

// ============================================================================
// Error Tests
// ============================================================================

#[test]
fn test_error_display() {
    use exo_std::error::{ExoStdError, IoError};
    
    let err = ExoStdError::Io(IoError::NotFound);
    let s = format!("{}", err);
    
    assert!(s.contains("I/O error"));
    assert!(s.contains("NotFound"));
}

#[test]
fn test_error_debug() {
    use exo_std::error::{ExoStdError, ProcessError};
    
    let err = ExoStdError::Process(ProcessError::InvalidPid);
    let s = format!("{:?}", err);
    
    assert!(s.contains("Process"));
    assert!(s.contains("InvalidPid"));
}

// ============================================================================
// I/O Tests
// ============================================================================

#[test]
fn test_cursor_read_write() {
    use exo_std::io::{Read, Write, Cursor, Seek, SeekFrom};
    
    let mut cursor = Cursor::new(vec![0u8; 100]);
    
    // Write
    cursor.write_all(b"hello").unwrap();
    cursor.write_all(b" ").unwrap();
    cursor.write_all(b"world").unwrap();
    
    // Seek to start
    cursor.seek(SeekFrom::Start(0)).unwrap();
    
    // Read
    let mut buf = [0u8; 11];
    cursor.read_exact(&mut buf).unwrap();
    
    assert_eq!(&buf, b"hello world");
}

#[test]
fn test_cursor_seek() {
    use exo_std::io::{Cursor, Seek, SeekFrom};
    
    let mut cursor = Cursor::new(vec![1, 2, 3, 4, 5]);
    
    // Seek to end
    let pos = cursor.seek(SeekFrom::End(0)).unwrap();
    assert_eq!(pos, 5);
    
    // Seek to middle
    let pos = cursor.seek(SeekFrom::Start(2)).unwrap();
    assert_eq!(pos, 2);
    
    // Relative seek
    let pos = cursor.seek(SeekFrom::Current(1)).unwrap();
    assert_eq!(pos, 3);
}

// ============================================================================
// Integration Tests
// ============================================================================

#[test]
fn test_mutex_with_bounded_vec() {
    let mut buffer = [0u32; 100];
    let vec = unsafe { BoundedVec::new(buffer.as_mut_ptr(), 100) };
    let m = Mutex::new(vec);
    
    {
        let mut guard = m.lock().unwrap();
        guard.push(1).unwrap();
        guard.push(2).unwrap();
        guard.push(3).unwrap();
    }
    
    let guard = m.lock().unwrap();
    assert_eq!(guard.len(), 3);
}

#[test]
fn test_rwlock_with_small_vec() {
    let vec: SmallVec<u32, 8> = SmallVec::new();
    let lock = RwLock::new(vec);
    
    {
        let mut w = lock.write().unwrap();
        w.push(42).unwrap();
    }
    
    let r = lock.read().unwrap();
    assert_eq!(r.first(), Some(&42));
}

// ============================================================================
// Benchmark-like Tests (timing)
// ============================================================================

#[test]
fn bench_mutex_lock_unlock() {
    let m = Mutex::new(0);
    
    // En test_mode, on ne peut pas mesurer le vrai temps
    // Mais on peut vérifier que ça fonctionne
    for _ in 0..1000 {
        let mut guard = m.lock().unwrap();
        *guard += 1;
    }
    
    assert_eq!(*m.lock().unwrap(), 1000);
}

#[test]
fn bench_rwlock_read() {
    let lock = RwLock::new(42);
    
    for _ in 0..1000 {
        let r = lock.read().unwrap();
        let _ = *r;
    }
}

#[test]
fn bench_atomic_cell() {
    let cell = AtomicCell::new(0u32);
    
    for i in 0..1000 {
        cell.store(i);
        let _ = cell.load();
    }
}

// ============================================================================
// Edge Cases
// ============================================================================

#[test]
fn test_bounded_vec_capacity_exceeded() {
    let mut buffer = [0u32; 2];
    let mut vec = unsafe { BoundedVec::new(buffer.as_mut_ptr(), 2) };
    
    vec.push(1).unwrap();
    vec.push(2).unwrap();
    
    // Devrait échouer
    assert!(vec.push(3).is_err());
}

#[test]
fn test_small_vec_empty() {
    let vec: SmallVec<u32, 4> = SmallVec::new();
    
    assert!(vec.is_empty());
    assert_eq!(vec.len(), 0);
    assert_eq!(vec.first(), None);
    assert_eq!(vec.last(), None);
}

#[test]
fn test_cursor_empty() {
    use exo_std::io::{Read, Cursor};
    
    let mut cursor = Cursor::new(vec![]);
    let mut buf = [0u8; 10];
    
    let n = cursor.read(&mut buf).unwrap();
    assert_eq!(n, 0);
}

// ============================================================================
// Concurrency Tests (simulated)
// ============================================================================

#[test]
fn test_mutex_contention_simulation() {
    let m = Mutex::new(0);
    
    // Simule contention avec multiples locks
    for _ in 0..100 {
        let mut g = m.lock().unwrap();
        *g += 1;
        // Drop automatique
    }
    
    assert_eq!(*m.lock().unwrap(), 100);
}

#[test]
fn test_rwlock_multiple_readers() {
    let lock = RwLock::new(vec![1, 2, 3]);
    
    // Simule plusieurs lecteurs
    let r1 = lock.read().unwrap();
    let r2 = lock.read().unwrap();
    let r3 = lock.read().unwrap();
    
    assert_eq!(r1.len(), 3);
    assert_eq!(r2.len(), 3);
    assert_eq!(r3.len(), 3);
}
