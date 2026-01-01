//! Phase 2c TODO #7: FPU Lazy Switching Tests
//! 
//! Tests pour vérifier:
//! 1. Lazy switching: pas de save/restore si thread n'utilise pas FPU
//! 2. State preservation: état FPU préservé across context switches
//! 3. Multi-thread: 10 threads utilisant FPU simultanément

#[cfg(test)]
mod tests {
    use crate::scheduler::SCHEDULER;
    use crate::arch::x86_64::utils::fpu;
    
    /// Test 1: Lazy Switching - thread sans FPU ne trigger pas #NM
    /// 
    /// Expected behavior:
    /// - Thread qui ne fait pas d'opérations FPU ne doit pas avoir fpu_used = true
    /// - CR0.TS reste set, pas de #NM exception triggered
    #[test_case]
    fn test_fpu_lazy_no_trigger() {
        // Create thread qui fait juste des calculs entiers
        let thread_id = SCHEDULER.spawn_kernel_thread(
            || {
                let mut sum = 0u64;
                for i in 0..1000 {
                    sum = sum.wrapping_add(i);
                }
                sum
            },
            "fpu_no_use"
        ).expect("Failed to spawn thread");
        
        // Wait for completion
        core::hint::spin_loop(); // Simplistic wait
        
        // Verify FPU was NOT used
        SCHEDULER.with_thread(thread_id, |thread| {
            assert!(!thread.fpu_used(), "Thread should NOT have used FPU");
        });
    }
    
    /// Test 2: State Preservation - état FPU préservé across switches
    /// 
    /// Expected behavior:
    /// - Thread set XMM0 = 1.234
    /// - Context switch (autre thread runs)
    /// - Thread resume, XMM0 still = 1.234
    #[test_case]
    fn test_fpu_state_preservation() {
        use core::arch::x86_64::_mm_set_ps;
        use core::arch::x86_64::_mm_cvtss_f32;
        
        let thread_id = SCHEDULER.spawn_kernel_thread(
            || {
                unsafe {
                    // Set XMM0 = [1.234, 5.678, 9.012, 3.456]
                    let xmm0 = _mm_set_ps(1.234, 5.678, 9.012, 3.456);
                    
                    // Force context switch (yield to other threads)
                    crate::scheduler::yield_now();
                    
                    // After resume, XMM0 should still be same value
                    let result = _mm_cvtss_f32(xmm0);
                    
                    // Verify (aproximate float comparison)
                    assert!((result - 3.456).abs() < 0.001, 
                            "FPU state lost: expected 3.456, got {}", result);
                }
            },
            "fpu_preserve"
        ).expect("Failed to spawn thread");
        
        // Wait for completion
        core::hint::spin_loop();
        
        // Verify FPU WAS used
        SCHEDULER.with_thread(thread_id, |thread| {
            assert!(thread.fpu_used(), "Thread SHOULD have used FPU");
        });
    }
    
    /// Test 3: Multi-Thread FPU - 10 threads utilisant FPU simultanément
    /// 
    /// Expected behavior:
    /// - Chaque thread fait calculs FPU uniques
    /// - Pas de corruption de state entre threads
    /// - Tous les threads get correct results
    #[test_case]
    fn test_fpu_multithread() {
        const NUM_THREADS: usize = 10;
        let mut thread_ids = Vec::with_capacity(NUM_THREADS);
        
        for i in 0..NUM_THREADS {
            let value = (i as f32) * 1.5; // Unique value per thread
            
            let tid = SCHEDULER.spawn_kernel_thread(
                move || {
                    unsafe {
                        use core::arch::x86_64::_mm_set_ps;
                        use core::arch::x86_64::_mm_cvtss_f32;
                        use core::arch::x86_64::_mm_add_ps;
                        
                        // Set XMM0 = [value, value, value, value]
                        let xmm0 = _mm_set_ps(value, value, value, value);
                        
                        // Do some FPU operations
                        let xmm1 = _mm_set_ps(2.0, 2.0, 2.0, 2.0);
                        let result_vec = _mm_add_ps(xmm0, xmm1);
                        
                        // Force context switches (yield 5 times)
                        for _ in 0..5 {
                            crate::scheduler::yield_now();
                        }
                        
                        // Verify result = value + 2.0
                        let result = _mm_cvtss_f32(result_vec);
                        let expected = value + 2.0;
                        
                        assert!((result - expected).abs() < 0.001, 
                                "Thread {} FPU corruption: expected {}, got {}", 
                                i, expected, result);
                    }
                },
                &format!("fpu_mt_{}", i)
            ).expect("Failed to spawn FPU thread");
            
            thread_ids.push(tid);
        }
        
        // Wait for all threads
        for _ in 0..100 {
            core::hint::spin_loop();
        }
        
        // Verify all used FPU
        for tid in thread_ids {
            SCHEDULER.with_thread(tid, |thread| {
                assert!(thread.fpu_used(), "Thread {} should have used FPU", tid);
            });
        }
    }
}
