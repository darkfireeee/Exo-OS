//! Boot Recovery Mode
//! 
//! Safe mode boot and diagnostic shell when normal boot fails.

use crate::boot::phases::BootPhase;

#[derive(Debug, Clone, Copy)]
pub enum RecoveryReason {
    Timeout,
    CriticalError,
    Panic,
    UserRequested,
    HardwareFailure,
}

pub struct RecoveryMode {
    pub reason: RecoveryReason,
    pub failed_phase: BootPhase,
}

impl RecoveryMode {
    pub fn enter(reason: RecoveryReason) -> ! {
        log::error!("╔══════════════════════════════════════╗");
        log::error!("║     ENTERING RECOVERY MODE           ║");
        log::error!("╚══════════════════════════════════════╝");
        log::error!("Reason: {:?}", reason);
        log::error!("Failed phase: {:?}", BootPhase::current());

        Self::halt()
    }

    fn halt() -> ! {
        log::info!("System halted.");
        loop {
            unsafe {
                core::arch::asm!("hlt", options(nostack, nomem));
            }
        }
    }
}
