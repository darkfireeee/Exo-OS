//! Boot Phases Management
//! 
//! Manages the different phases of boot process:
//! - CRITICAL: Memory, GDT, IDT (<50ms target)
//! - NORMAL: Scheduler, IPC (<100ms target)
//! - DEFERRED: Drivers, AI agents (lazy initialization)

use core::sync::atomic::{AtomicU8, Ordering};

/// Boot phase states
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum BootPhase {
    /// Pre-boot (before Rust entry)
    PreBoot = 0,
    /// Critical phase (<50ms)
    Critical = 1,
    /// Normal phase (<100ms)
    Normal = 2,
    /// Deferred phase (lazy)
    Deferred = 3,
    /// Boot complete
    Complete = 4,
}

static CURRENT_PHASE: AtomicU8 = AtomicU8::new(BootPhase::PreBoot as u8);

impl BootPhase {
    /// Get current boot phase
    pub fn current() -> Self {
        match CURRENT_PHASE.load(Ordering::Acquire) {
            0 => BootPhase::PreBoot,
            1 => BootPhase::Critical,
            2 => BootPhase::Normal,
            3 => BootPhase::Deferred,
            4 => BootPhase::Complete,
            _ => BootPhase::PreBoot,
        }
    }

    /// Advance to next phase
    pub fn advance(next: BootPhase) -> Result<(), &'static str> {
        let current = Self::current();
        if next as u8 <= current as u8 {
            return Err("Cannot go backwards in boot phases");
        }

        CURRENT_PHASE.store(next as u8, Ordering::Release);
        log::info!("Boot phase: {:?} -> {:?}", current, next);
        Ok(())
    }

    /// Check if phase is at least the specified level
    pub fn is_at_least(phase: BootPhase) -> bool {
        Self::current() >= phase
    }
}

/// Boot phase timing
pub struct PhaseTimer {
    name: &'static str,
    start: u64,
}

impl PhaseTimer {
    /// Start timing a phase
    pub fn start(name: &'static str) -> Self {
        log::info!("Starting phase: {}", name);
        let start = 0; // TODO: crate::time::tsc::read_tsc();
        PhaseTimer { name, start }
    }

    /// End timing and report
    pub fn end(self) {
        let end: u64 = 0; // TODO: crate::time::tsc::read_tsc();
        let cycles = end.saturating_sub(self.start);
        let ms = cycles / 3_000_000; // Approximation @ 3GHz
        log::info!("Phase '{}' completed in ~{}ms ({} cycles)", self.name, ms, cycles);
    }
}

/// Execute critical boot phase
pub fn execute_critical() -> Result<(), &'static str> {
    let _timer = PhaseTimer::start("CRITICAL");
    BootPhase::advance(BootPhase::Critical)?;

    // Critical initialization (from early_init)
    log::info!("  - GDT setup");
    log::info!("  - IDT setup");
    log::info!("  - Memory detection");
    log::info!("  - Physical allocator");
    log::info!("  - Virtual memory");

    Ok(())
}

/// Execute normal boot phase
pub fn execute_normal() -> Result<(), &'static str> {
    let _timer = PhaseTimer::start("NORMAL");
    BootPhase::advance(BootPhase::Normal)?;

    // Normal initialization (from late_init)
    log::info!("  - Scheduler init");
    log::info!("  - IPC init");
    log::info!("  - Syscall init");

    Ok(())
}

/// Execute deferred boot phase
pub fn execute_deferred() -> Result<(), &'static str> {
    let _timer = PhaseTimer::start("DEFERRED");
    BootPhase::advance(BootPhase::Deferred)?;

    // Deferred initialization (lazy)
    log::info!("  - Driver discovery");
    log::info!("  - AI agents spawn");
    log::info!("  - Network stack");

    Ok(())
}

/// Mark boot complete
pub fn complete() -> Result<(), &'static str> {
    BootPhase::advance(BootPhase::Complete)?;
    log::info!("Boot sequence completed!");
    Ok(())
}
