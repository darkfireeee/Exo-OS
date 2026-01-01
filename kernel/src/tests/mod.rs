//! Test module for Phase 0-1c validation

pub mod process_tests;
pub mod phase2_smp_tests;
pub mod keyboard_test;
pub mod exec_test;
pub mod benchmark_real_threads;
pub mod signal_tests;
pub mod simple_multithread;
pub mod validation;
pub mod smp_tests;       // Phase 2b: SMP scheduler tests
pub mod smp_bench;       // Phase 2b: SMP benchmarks
pub mod smp_regression;  // Phase 2c: Regression tests
pub mod fpu_lazy_tests;  // Phase 2c TODO #7: FPU lazy switching tests
