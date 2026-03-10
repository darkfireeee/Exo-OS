// kernel/src/process/signal/mask.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Masque de signaux POSIX (sigprocmask, pthread_sigmask) — Exo-OS Couche 1.5
// ═══════════════════════════════════════════════════════════════════════════════
//
// Le masque de signaux est un bitmap 64 bits par thread.
// Bit i = signal (i+1) est bloqué.
// SIGKILL (bit 8) et SIGSTOP (bit 18) ne peuvent être bloqués : ignorés.

#![allow(dead_code)]

use core::sync::atomic::{AtomicU64, Ordering};
use crate::scheduler::core::task::ThreadControlBlock;
use super::default::Signal;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

pub const SIG_BLOCK:   i32 = 0;
pub const SIG_UNBLOCK: i32 = 1;
pub const SIG_SETMASK: i32 = 2;

/// Signaux non-bloquables : SIGKILL (9) et SIGSTOP (19).
/// Bit i correspond au signal (i+1).
/// SIGKILL = bit 8, SIGSTOP = bit 18.
const NON_BLOCKABLE: u64 =
    (1u64 << (Signal::SIGKILL  as u8 - 1)) |
    (1u64 << (Signal::SIGSTOP  as u8 - 1));

// ─────────────────────────────────────────────────────────────────────────────
// SigMask — valeur pure (pas atomique)
// ─────────────────────────────────────────────────────────────────────────────

/// Masque de signaux non-atomique (valeur pure à usage local).
#[derive(Copy, Clone, Default, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct SigMask(pub u64);

impl SigMask {
    pub const EMPTY: Self = Self(0);
    pub const FULL:  Self = Self(!NON_BLOCKABLE);

    #[inline(always)]
    pub fn set(&mut self, sig: u8) {
        if sig == 0 || sig > 64 { return; }
        self.0 |= 1u64 << (sig - 1);
        self.0 &= !NON_BLOCKABLE;
    }

    #[inline(always)]
    pub fn clear(&mut self, sig: u8) {
        if sig == 0 || sig > 64 { return; }
        self.0 &= !(1u64 << (sig - 1));
    }

    #[inline(always)]
    pub fn is_set(&self, sig: u8) -> bool {
        if sig == 0 || sig > 64 { return false; }
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
    fn from(v: u64) -> Self { SigMask(v & !NON_BLOCKABLE) }
}

// ─────────────────────────────────────────────────────────────────────────────
// SigSet — wrappers atomique (pour stockage dans TCB)
// ─────────────────────────────────────────────────────────────────────────────

/// Ensemble atomique de signal pending (vu depuis le TCB scheduler).
/// Accessible depuis plusieurs CPU simultanément : opérations atomiques.
#[repr(transparent)]
pub struct SigSet(AtomicU64);

impl SigSet {
    pub const fn empty() -> Self { Self(AtomicU64::new(0)) }

    #[inline(always)]
    pub fn load(&self) -> SigMask {
        SigMask(self.0.load(Ordering::Acquire))
    }

    #[inline(always)]
    pub fn store(&self, m: SigMask) {
        self.0.store(m.0, Ordering::Release);
    }

    /// Définit le bit du signal sans lock (atomique fetch_or).
    #[inline(always)]
    pub fn set_signal(&self, sig: u8) {
        if sig == 0 || sig > 64 { return; }
        self.0.fetch_or(1u64 << (sig - 1), Ordering::AcqRel);
    }

    /// Efface le bit du signal sans lock.
    #[inline(always)]
    pub fn clear_signal(&self, sig: u8) {
        if sig == 0 || sig > 64 { return; }
        self.0.fetch_and(!(1u64 << (sig - 1)), Ordering::AcqRel);
    }

    /// Vérifie si au moins un signal non-bloqué est en attente.
    #[inline(always)]
    pub fn has_unblocked(&self, mask: SigMask) -> bool {
        self.0.load(Ordering::Acquire) & !mask.0 != 0
    }

    /// Premier signal non-bloqué en attente (0 = aucun).
    pub fn first_pending(&self, mask: SigMask) -> u8 {
        let pending = self.0.load(Ordering::Acquire) & !mask.0;
        if pending == 0 { 0 }
        else { pending.trailing_zeros() as u8 + 1 }
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
/// Opère sur le masque stocké dans le TCB courant via le champ
/// `signal_mask: AtomicU64` de `ThreadControlBlock`.
///
/// `how` : SIG_BLOCK (0), SIG_UNBLOCK (1), SIG_SETMASK (2).
/// `set` : nouveau masque (Some) ou NULL (None).
/// Retourne l'ancien masque.
pub fn sigprocmask(
    tcb:   &ThreadControlBlock,
    how:   i32,
    set:   Option<SigMask>,
) -> Result<SigMask, SigmaskError> {
    let old = SigMask(tcb.signal_mask.load(Ordering::Acquire));
    if let Some(new_set) = set {
        let safe_set = SigMask(new_set.0 & !NON_BLOCKABLE);
        let updated = match how {
            SIG_BLOCK   => SigMask((old.0 | safe_set.0) & !NON_BLOCKABLE),
            SIG_UNBLOCK => SigMask(old.0 & !safe_set.0),
            SIG_SETMASK => SigMask(safe_set.0 & !NON_BLOCKABLE),
            _           => return Err(SigmaskError::InvalidHow),
        };
        tcb.signal_mask.store(updated.0, Ordering::Release);
    }
    Ok(old)
}

/// Réinitialise les handlers POSIX à SIG_DFL lors d'un execve.
/// Appelé exclusivement depuis lifecycle/exec.rs.
///
/// Politique :
/// - Tous les handlers utilisateur → SIG_DFL.
/// - Les signaux ignorés (SIG_IGN) restent ignorés.
/// - Le masque de signal est conservé.
pub fn reset_signals_on_exec(tcb: &ThreadControlBlock) {
    // Acquérir le pointeur vers le PCB pour réinitialiser la table des handlers.
    // Le PCB contient le champ `sig_handlers: SpinLock<SigHandlerTable>`.
    
    use crate::process::core::registry::PROCESS_REGISTRY;
    use crate::process::core::pid::Pid;

    let pid = Pid(tcb.pid.0);
    if let Some(pcb) = PROCESS_REGISTRY.find_by_pid(pid) {
        let mut handlers = pcb.sig_handlers.lock();
        handlers.reset_on_exec();
    }
    // Le masque de signal est conservé par POSIX (sauf si SA_RESETHAND).
}

/// Bloque tous les signaux sauf SIGKILL et SIGSTOP (LAC-08 / PROC-03).
///
/// Appelé au début de do_execve() **avant** le chargement ELF pouréviter le
/// race PROC-03 : un signal livré entre load_elf() et reset_signals_on_exec()
/// invoquerait l'ancien handler dans un espace d'adressage partiellement
/// remplacé → comportement indéfini / exploit potentiel.
///
/// Les signaux sont débloqués par reset_signals_on_exec() qui suit.
pub fn block_all_except_kill(tcb: &ThreadControlBlock) {
    // SigMask::FULL = tous les signaux sauf SIGKILL (9) et SIGSTOP (19).
    // NON_BLOCKABLE est déjà masqué dans SigMask::FULL.
    tcb.signal_mask.store(SigMask::FULL.0, Ordering::Release);
}
