// kernel/src/process/signal/mask.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// POSIX signal mask (sigprocmask, pthread_sigmask) — Exo-OS Layer 1.5
// ═══════════════════════════════════════════════════════════════════════════════
//
// The signal mask is a 64-bit bitmap per thread.
// Bit i = signal (i+1) is blocked.
// SIGKILL (bit 8) and SIGSTOP (bit 18) cannot be blocked: ignored.

use super::default::Signal;
use crate::scheduler::core::task::ThreadControlBlock;
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

pub const SIG_BLOCK: i32 = 0;
pub const SIG_UNBLOCK: i32 = 1;
pub const SIG_SETMASK: i32 = 2;

/// Unblockable signals: SIGKILL (9) and SIGSTOP (19).
/// Bit i corresponds to signal (i+1).
/// SIGKILL = bit 8, SIGSTOP = bit 18.
const NON_BLOCKABLE: u64 =
    (1u64 << (Signal::SIGKILL as u8 - 1)) | (1u64 << (Signal::SIGSTOP as u8 - 1));
const _: () = assert!(
    NON_BLOCKABLE & (1u64 << (Signal::SIGKILL as u8 - 1)) != 0,
    "[ASSERT-SIG01] SIGKILL missing from NON_BLOCKABLE"
);
const _: () = assert!(
    NON_BLOCKABLE & (1u64 << (Signal::SIGSTOP as u8 - 1)) != 0,
    "[ASSERT-SIG02] SIGSTOP missing from NON_BLOCKABLE"
);
const VALID_SIGNAL_MASK: u64 = (1u64 << Signal::SIGRTMAX) - 1;

// ─────────────────────────────────────────────────────────────────────────────
// SigMask — pure value (not atomic)
// ─────────────────────────────────────────────────────────────────────────────

/// Non-atomic signal mask (pure value for local use).
#[derive(Copy, Clone, Default, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct SigMask(pub u64);

impl SigMask {
    pub const EMPTY: Self = Self(0);
    pub const FULL: Self = Self(VALID_SIGNAL_MASK & !NON_BLOCKABLE);

    #[inline(always)]
    pub fn set(&mut self, sig: u8) {
        if sig == 0 || sig > Signal::SIGRTMAX {
            return;
        }
        self.0 |= 1u64 << (sig - 1);
        self.0 &= !NON_BLOCKABLE;
    }

    #[inline(always)]
    pub fn clear(&mut self, sig: u8) {
        if sig == 0 || sig > Signal::SIGRTMAX {
            return;
        }
        self.0 &= !(1u64 << (sig - 1));
    }

    #[inline(always)]
    pub fn is_set(&self, sig: u8) -> bool {
        if sig == 0 || sig > Signal::SIGRTMAX {
            return false;
        }
        self.0 & (1u64 << (sig - 1)) != 0
    }

    #[inline(always)]
    pub fn union(&self, other: SigMask) -> SigMask {
        SigMask((self.0 | other.0) & !NON_BLOCKABLE)
    }

    #[inline(always)]
    pub fn intersect(&self, other: SigMask) -> SigMask {
        SigMask(self.0 & other.0)
    }

    #[inline(always)]
    pub fn difference(&self, other: SigMask) -> SigMask {
        SigMask(self.0 & !other.0)
    }
}

impl From<u64> for SigMask {
    fn from(v: u64) -> Self {
        SigMask(v & VALID_SIGNAL_MASK & !NON_BLOCKABLE)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SigSet — atomic wrappers (for storage in TCB)
// ─────────────────────────────────────────────────────────────────────────────

/// Atomic pending signal set (seen from scheduler TCB).
/// Accessible from multiple CPUs simultaneously: atomic operations.
#[repr(transparent)]
pub struct SigSet(AtomicU64);

impl SigSet {
    pub const fn empty() -> Self {
        Self(AtomicU64::new(0))
    }

    #[inline(always)]
    pub fn load(&self) -> SigMask {
        SigMask(self.0.load(Ordering::Acquire))
    }

    #[inline(always)]
    pub fn store(&self, m: SigMask) {
        self.0.store(SigMask::from(m.0).0, Ordering::Release);
    }

    /// Sets signal bit lock-free (atomic fetch_or).
    #[inline(always)]
    pub fn set_signal(&self, sig: u8) {
        if sig == 0 || sig > Signal::SIGRTMAX {
            return;
        }
        self.0.fetch_or(1u64 << (sig - 1), Ordering::AcqRel);
    }

    /// Clears signal bit lock-free.
    #[inline(always)]
    pub fn clear_signal(&self, sig: u8) {
        if sig == 0 || sig > Signal::SIGRTMAX {
            return;
        }
        self.0.fetch_and(!(1u64 << (sig - 1)), Ordering::AcqRel);
    }

    /// Checks if at least one unblocked signal is pending.
    #[inline(always)]
    pub fn has_unblocked(&self, mask: SigMask) -> bool {
        self.0.load(Ordering::Acquire) & !mask.0 != 0
    }

    /// First unblocked pending signal (0 = none).
    pub fn first_pending(&self, mask: SigMask) -> u8 {
        let pending = self.0.load(Ordering::Acquire) & !mask.0;
        if pending == 0 {
            0
        } else {
            pending.trailing_zeros() as u8 + 1
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// sigprocmask / pthread_sigmask
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigmaskError {
    InvalidHow,
    InvalidSignal,
}

/// POSIX sigprocmask(2).
/// Operates on the mask stored in the current TCB via the
/// `signal_mask: AtomicU64` field of `ThreadControlBlock`.
///
/// `how`: SIG_BLOCK (0), SIG_UNBLOCK (1), SIG_SETMASK (2).
/// `set`: new mask (Some) or NULL (None).
/// Returns the old mask.
pub fn sigprocmask(
    tcb: &ThreadControlBlock,
    how: i32,
    set: Option<SigMask>,
) -> Result<SigMask, SigmaskError> {
    let old = SigMask::from(tcb.signal_mask.load(Ordering::Acquire));
    if let Some(new_set) = set {
        let safe_set = SigMask(new_set.0 & !NON_BLOCKABLE);
        let updated = match how {
            SIG_BLOCK => SigMask((old.0 | safe_set.0) & !NON_BLOCKABLE),
            SIG_UNBLOCK => SigMask(old.0 & !safe_set.0),
            SIG_SETMASK => SigMask(safe_set.0 & !NON_BLOCKABLE),
            _ => return Err(SigmaskError::InvalidHow),
        };
        tcb.signal_mask.store(updated.0, Ordering::Release);
    }
    Ok(old)
}

/// Resets POSIX handlers to SIG_DFL during execve.
/// Called exclusively from lifecycle/exec.rs.
///
/// Policy:
/// - All user handlers → SIG_DFL.
/// - Ignored signals (SIG_IGN) remain ignored.
/// - The signal mask is preserved.
pub fn reset_signals_on_exec(tcb: &ThreadControlBlock) {
    // Acquire pointer to PCB to reset handler table.
    // The PCB contains the `sig_handlers: SpinLock<SigHandlerTable>` field.

    use crate::process::core::pid::Pid;
    use crate::process::core::registry::PROCESS_REGISTRY;

    let pid = Pid(tcb.pid.0);
    if let Some(pcb) = PROCESS_REGISTRY.find_by_pid(pid) {
        let mut handlers = pcb.sig_handlers.lock();
        handlers.reset_on_exec();
    }
    // The signal mask is preserved by POSIX (unless SA_RESETHAND).
}

/// Blocks all signals except SIGKILL and SIGSTOP (LAC-08 / PROC-03).
///
/// Called at the start of do_execve() **before** ELF loading to avoid
/// the PROC-03 race: a signal delivered between load_elf() and reset_signals_on_exec()
/// would invoke the old handler in a partially
/// replaced address space → undefined behavior / potential exploit.
///
/// The calling mask is restored by do_execve() after resetting handlers and
/// purging pending queues from the old image.
pub fn block_all_except_kill(tcb: &ThreadControlBlock) {
    // SigMask::FULL = all signals except SIGKILL (9) and SIGSTOP (19).
    // NON_BLOCKABLE is already masked in SigMask::FULL.
    tcb.signal_mask.store(SigMask::FULL.0, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::{SigMask, Signal, VALID_SIGNAL_MASK};

    #[test]
    fn full_mask_keeps_only_valid_signals() {
        assert_eq!(SigMask::FULL.0 & !VALID_SIGNAL_MASK, 0);
        assert!(!SigMask::FULL.is_set(Signal::SIGKILL as u8));
        assert!(!SigMask::FULL.is_set(Signal::SIGSTOP as u8));
        assert!(SigMask::FULL.is_set(Signal::SIGRTMAX));
    }

    #[test]
    fn signal_64_is_not_representable() {
        let mut mask = SigMask::EMPTY;

        mask.set(64);

        assert_eq!(mask.0, 0);
        assert!(!mask.is_set(64));
    }
}
