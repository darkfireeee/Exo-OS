//! Test module for Phase 0-1c validation

pub mod process_tests;
pub mod phase2_smp_tests;
pub mod keyboard_test;
pub mod exec_test;
pub mod exec_tests;           // JOUR 1: Tests RÉELS load_elf_binary()
pub mod exec_tests_real;      // JOUR 2: Test avec binaire compilé réel
pub mod benchmark_real_threads;
pub mod signal_tests;
pub mod simple_multithread;
pub mod validation;
pub mod smp_tests;            // Phase 2b: SMP scheduler tests
pub mod smp_bench;            // Phase 2b: SMP benchmarks
pub mod smp_regression;       // Phase 2c Week 1: Regression tests
pub mod fpu_lazy_tests;       // Phase 2c Week 2: FPU lazy switching
pub mod week3_timer_pi_tests; // Phase 2c Week 3: Timer sleep + Priority inheritance
pub mod week4_hardware_tests; // Phase 2c Week 4: Hardware SMP validation
pub mod phase2d_tests;
pub mod phase2d_test_runner;  // ✅ Phase 2d: Test runner intégré
pub mod cow_fork_test;        // Jour 4: CoW fork() avec métriques réelles
pub mod cow_advanced_tests;   // Jour 4 Phase 3B: Tests avancés CoW
pub mod cow_real_tests;       // Jour 4 Phase 4: Tests RÉELS avec vraies pages
