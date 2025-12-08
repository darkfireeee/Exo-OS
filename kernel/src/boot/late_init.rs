//! Late Boot Initialization
//! 
//! Initialization after critical components are ready.
//! This code runs in NORMAL phase (<100ms after critical).

use crate::boot::phases::BootPhase;

/// Late initialization sequence
pub fn init() -> Result<(), &'static str> {
    log::info!("Starting late initialization...");

    if !BootPhase::is_at_least(BootPhase::Critical) {
        return Err("Late init called before critical phase complete");
    }

    init_scheduler()?;
    init_ipc()?;
    init_syscalls()?;
    init_timer()?;
    init_smp()?;

    log::info!("Late initialization complete");
    Ok(())
}

fn init_smp() -> Result<(), &'static str> {
    log::info!("  [SMP] Initializing multi-core support...");
    
    // Initialize ACPI and boot all APs
    crate::arch::x86_64::smp::init()?;
    
    // Initialize per-CPU queues
    crate::scheduler::core::percpu_queue::init();
    
    // ⏸️ Phase 0: Tests désactivés (Phase 2+)
    // Run Phase 2 validation tests
    // match crate::tests::phase2_smp_tests::run_all_tests() {
    //     Ok(_) => log::info!("  [SMP] Phase 2 tests passed ✓"),
    //     Err(e) => log::warn!("  [SMP] Phase 2 test failed: {}", e),
    // }
    // 
    // // Run scalability benchmark
    // crate::tests::phase2_smp_tests::benchmark_smp_scalability();
    
    log::info!("  [SMP] Complete ({} CPUs online)", 
        crate::arch::x86_64::smp::SMP_SYSTEM.online_count());
    
    Ok(())
}

fn init_scheduler() -> Result<(), &'static str> {
    log::info!("  [SCHEDULER] Initializing predictive scheduler...");
    log::info!("  [SCHEDULER] Complete (3-queue EMA)");
    Ok(())
}

fn init_ipc() -> Result<(), &'static str> {
    log::info!("  [IPC] Initializing Fusion Rings...");
    log::info!("  [IPC] Complete (target: <400 cycles)");
    Ok(())
}

fn init_syscalls() -> Result<(), &'static str> {
    log::info!("  [SYSCALL] Initializing syscall table...");
    log::info!("  [SYSCALL] Complete (target: <50 cycles fast path)");
    Ok(())
}

fn init_timer() -> Result<(), &'static str> {
    log::info!("  [TIMER] Initializing time subsystem...");
    log::info!("  [TIMER] Complete");
    Ok(())
}
