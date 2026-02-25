// kernel/src/security/isolation/sandbox.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Sandbox — Politique de filtrage syscall par processus (style Seccomp)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Architecture :
//   • SandboxPolicy : bitmap de syscalls autorisés (256 syscalls, 4 × u64)
//   • DEFAULT_DENY : toute politique commence en deny-all
//   • Actions par syscall : Allow, Deny, Errno(n), Kill
//   • Intégration : le handler syscall vérifie la politique avant dispatch
//   • Instrumentation : compteurs refus/autorisations par processus
//
// RÈGLE SAND-01 : SandboxPolicy ne peut QUE réduire les droits (jamais augmenter).
// RÈGLE SAND-02 : Un processus Sandbox ne peut pas modifier sa propre policy.
// RÈGLE SAND-03 : Héritage : le fils reçoit une policy ⊆ père.
// ═══════════════════════════════════════════════════════════════════════════════

#![allow(dead_code)]

use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Numéros de syscalls Exo-OS (sous-ensemble représentatif)
// ─────────────────────────────────────────────────────────────────────────────

pub mod syscall_nr {
    pub const READ:        usize = 0;
    pub const WRITE:       usize = 1;
    pub const OPEN:        usize = 2;
    pub const CLOSE:       usize = 3;
    pub const STAT:        usize = 4;
    pub const FSTAT:       usize = 5;
    pub const LSTAT:       usize = 6;
    pub const POLL:        usize = 7;
    pub const LSEEK:       usize = 8;
    pub const MMAP:        usize = 9;
    pub const MPROTECT:    usize = 10;
    pub const MUNMAP:      usize = 11;
    pub const BRK:         usize = 12;
    pub const RT_SIGACTION:usize = 13;
    pub const FORK:        usize = 57;
    pub const EXECVE:      usize = 59;
    pub const EXIT:        usize = 60;
    pub const WAIT4:       usize = 61;
    pub const KILL:        usize = 62;
    pub const SOCKET:      usize = 41;
    pub const CONNECT:     usize = 42;
    pub const BIND:        usize = 49;
    pub const LISTEN:      usize = 50;
    pub const ACCEPT:      usize = 43;
    pub const SEND:        usize = 44;
    pub const RECV:        usize = 45;
    pub const IOCTL:       usize = 16;
    pub const PRCTL:       usize = 157;
    pub const CLONE:       usize = 56;
    pub const PTRACE:      usize = 101;
    pub const MADVISE:     usize = 28;
    pub const FUTEX:       usize = 202;
    pub const GETPID:      usize = 39;
    pub const GETTID:      usize = 186;
    pub const GETTIMEOFDAY:usize = 96;
    pub const NANOSLEEP:   usize = 35;
    pub const PIPE:        usize = 22;
    pub const DUP:         usize = 32;
    pub const DUP2:        usize = 33;
    pub const GETUID:      usize = 102;
    pub const GETGID:      usize = 104;

    pub const MAX_SYSCALL: usize = 256;
}

// ─────────────────────────────────────────────────────────────────────────────
// SandboxAction — action prise pour un syscall filtré
// ─────────────────────────────────────────────────────────────────────────────

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxAction {
    /// Laisser passer le syscall.
    Allow  = 0,
    /// Retourner EPERM (1).
    DenyEperm = 1,
    /// Retourner ENOSYS (2).
    DenyEnosys = 2,
    /// Tuer le processus (SIGKILL).
    Kill   = 3,
    /// Logger et retourner EPERM.
    LogAndDeny = 4,
}

// ─────────────────────────────────────────────────────────────────────────────
// SandboxPolicy — bitmap + actions pour 256 syscalls
// ─────────────────────────────────────────────────────────────────────────────

/// Politique de sandbox pour un processus.
///
/// Représentation compacte :
///   - `allowed_bitmap` : 4 × u64 = 256 bits — 1 si syscall autorisé
///   - `kill_bitmap`    : 4 × u64 — 1 si syscall → SIGKILL (priorité sur allowed)
///   - `log_bitmap`     : 4 × u64 — 1 si syscall → log + deny
#[derive(Clone)]
pub struct SandboxPolicy {
    allowed_bitmap: [u64; 4],
    kill_bitmap:    [u64; 4],
    log_bitmap:     [u64; 4],
    /// Nombre de refus enregistrés (instrumentation).
    denials:        u64,
    /// Nombre d'autorisations enregistrées.
    allows:         u64,
}

impl SandboxPolicy {
    /// Crée une politique deny-all (aucun syscall autorisé).
    pub const fn deny_all() -> Self {
        Self {
            allowed_bitmap: [0u64; 4],
            kill_bitmap:    [0u64; 4],
            log_bitmap:     [0u64; 4],
            denials:        0,
            allows:         0,
        }
    }

    /// Crée une politique allow-all (tous les syscalls autorisés).
    pub const fn allow_all() -> Self {
        Self {
            allowed_bitmap: [!0u64; 4],
            kill_bitmap:    [0u64; 4],
            log_bitmap:     [0u64; 4],
            denials:        0,
            allows:         0,
        }
    }

    /// Crée une politique minimale pour un processus lecture-seule.
    pub fn read_only_minimal() -> Self {
        let mut p = Self::deny_all();
        for nr in [
            syscall_nr::READ, syscall_nr::FSTAT, syscall_nr::STAT,
            syscall_nr::LSEEK, syscall_nr::CLOSE, syscall_nr::MMAP,
            syscall_nr::MUNMAP, syscall_nr::EXIT, syscall_nr::GETPID,
            syscall_nr::GETTID, syscall_nr::GETTIMEOFDAY, syscall_nr::FUTEX,
            syscall_nr::NANOSLEEP, syscall_nr::MADVISE,
        ] {
            p.allow_syscall(nr);
        }
        p
    }

    /// Crée une politique pour les processus réseau (allow I/O + socket).
    pub fn network_io() -> Self {
        let mut p = Self::read_only_minimal();
        for nr in [
            syscall_nr::WRITE, syscall_nr::SOCKET, syscall_nr::CONNECT,
            syscall_nr::SEND, syscall_nr::RECV, syscall_nr::POLL,
            syscall_nr::PIPE, syscall_nr::DUP, syscall_nr::DUP2,
        ] {
            p.allow_syscall(nr);
        }
        p
    }

    /// Autorise un syscall.
    pub fn allow_syscall(&mut self, nr: usize) {
        if nr < syscall_nr::MAX_SYSCALL {
            self.allowed_bitmap[nr / 64] |= 1u64 << (nr % 64);
        }
    }

    /// Interdit un syscall (retire de la liste allowed).
    pub fn deny_syscall(&mut self, nr: usize) {
        if nr < syscall_nr::MAX_SYSCALL {
            self.allowed_bitmap[nr / 64] &= !(1u64 << (nr % 64));
        }
    }

    /// Configure un syscall pour tuer le processus s'il est appelé.
    pub fn kill_on_syscall(&mut self, nr: usize) {
        if nr < syscall_nr::MAX_SYSCALL {
            self.deny_syscall(nr);
            self.kill_bitmap[nr / 64] |= 1u64 << (nr % 64);
        }
    }

    /// Configure un syscall pour logger + deny.
    pub fn log_and_deny_syscall(&mut self, nr: usize) {
        if nr < syscall_nr::MAX_SYSCALL {
            self.deny_syscall(nr);
            self.log_bitmap[nr / 64] |= 1u64 << (nr % 64);
        }
    }

    /// Évalue l'action pour un numéro de syscall.
    pub fn evaluate(&mut self, nr: usize) -> SandboxAction {
        if nr >= syscall_nr::MAX_SYSCALL {
            self.denials += 1;
            return SandboxAction::DenyEnosys;
        }
        let word = nr / 64;
        let bit  = 1u64 << (nr % 64);

        if self.kill_bitmap[word] & bit != 0 {
            self.denials += 1;
            return SandboxAction::Kill;
        }
        if self.log_bitmap[word] & bit != 0 {
            self.denials += 1;
            return SandboxAction::LogAndDeny;
        }
        if self.allowed_bitmap[word] & bit != 0 {
            self.allows += 1;
            return SandboxAction::Allow;
        }
        self.denials += 1;
        SandboxAction::DenyEperm
    }

    /// Évalue sans modifier les compteurs (utile pour les checks préalables).
    pub fn check(&self, nr: usize) -> SandboxAction {
        if nr >= syscall_nr::MAX_SYSCALL { return SandboxAction::DenyEnosys; }
        let word = nr / 64;
        let bit  = 1u64 << (nr % 64);
        if self.kill_bitmap[word] & bit != 0 { return SandboxAction::Kill; }
        if self.log_bitmap[word] & bit != 0  { return SandboxAction::LogAndDeny; }
        if self.allowed_bitmap[word] & bit != 0 { return SandboxAction::Allow; }
        SandboxAction::DenyEperm
    }

    /// RÈGLE SAND-03 : Retourne une politique enfant ⊆ self (AND des bitmaps).
    pub fn derive_child(&self) -> SandboxPolicy {
        // L'enfant hérite exactement les mêmes droits (sous-ensemble garanti par AND)
        Self {
            allowed_bitmap: self.allowed_bitmap,
            kill_bitmap:    self.kill_bitmap,
            log_bitmap:     self.log_bitmap,
            denials:        0,
            allows:         0,
        }
    }

    /// Restreint la politique actuelle avec `other` (intersection des allowed).
    pub fn intersect_with(&mut self, other: &SandboxPolicy) {
        for i in 0..4 {
            self.allowed_bitmap[i] &= other.allowed_bitmap[i];
            self.kill_bitmap[i]    |= other.kill_bitmap[i];
            self.log_bitmap[i]     |= other.log_bitmap[i];
        }
    }

    /// Statistiques de la politique.
    pub fn stats(&self) -> (u64, u64) {
        (self.allows, self.denials)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Statistiques globales sandbox
// ─────────────────────────────────────────────────────────────────────────────

static GLOBAL_SANDBOX_DENIALS: AtomicU64 = AtomicU64::new(0);
static GLOBAL_SANDBOX_ALLOWS:  AtomicU64 = AtomicU64::new(0);
static GLOBAL_SANDBOX_KILLS:   AtomicU64 = AtomicU64::new(0);

/// Enregistre une décision sandbox dans les compteurs globaux.
pub fn record_sandbox_decision(action: SandboxAction) {
    match action {
        SandboxAction::Allow => { GLOBAL_SANDBOX_ALLOWS.fetch_add(1, Ordering::Relaxed); }
        SandboxAction::Kill  => { GLOBAL_SANDBOX_KILLS.fetch_add(1, Ordering::Relaxed);
                                  GLOBAL_SANDBOX_DENIALS.fetch_add(1, Ordering::Relaxed); }
        _ =>                   { GLOBAL_SANDBOX_DENIALS.fetch_add(1, Ordering::Relaxed); }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SandboxGlobalStats {
    pub global_allows:  u64,
    pub global_denials: u64,
    pub global_kills:   u64,
}

pub fn sandbox_global_stats() -> SandboxGlobalStats {
    SandboxGlobalStats {
        global_allows:  GLOBAL_SANDBOX_ALLOWS.load(Ordering::Relaxed),
        global_denials: GLOBAL_SANDBOX_DENIALS.load(Ordering::Relaxed),
        global_kills:   GLOBAL_SANDBOX_KILLS.load(Ordering::Relaxed),
    }
}
