// kernel/src/security/isolation/pledge.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Pledge — Restrictions de capacités style OpenBSD
// ═══════════════════════════════════════════════════════════════════════════════
//
// Architecture :
//   • PledgeSet : bitmask de "promesses" (pledges) que le processus s'engage à respecter
//   • Une fois activé, les pledges ne peuvent QUE se réduire
//   • Intégration avec SandboxPolicy : les pledges génèrent une SandboxPolicy
//   • Pledges disponibles : stdio, rpath, wpath, cpath, tmppath, 
//     inet, unix, dns, getpw, proc, exec, id, route, etc.
//
// RÈGLE PLEDGE-01 : Un processus ne peut QUE retirer des pledges, jamais en ajouter.
// RÈGLE PLEDGE-02 : La violation d'un pledge → SIGKILL immédiat.
// RÈGLE PLEDGE-03 : Le processus init ne peut pas appeler pledge().
// ═══════════════════════════════════════════════════════════════════════════════


use core::sync::atomic::{AtomicU64, Ordering};
use super::sandbox::{SandboxPolicy, syscall_nr};

// ─────────────────────────────────────────────────────────────────────────────
// Pledges disponibles
// ─────────────────────────────────────────────────────────────────────────────

/// Flags de pledge (bitmask u64).
pub mod pledge_flags {
    /// Opérations I/O de base : read, write, recvfrom, sendto, etc.
    pub const STDIO:    u64 = 1 << 0;
    /// Lecture de fichiers (open en mode lecture, stat, etc.).
    pub const RPATH:    u64 = 1 << 1;
    /// Écriture de fichiers (open en écriture).
    pub const WPATH:    u64 = 1 << 2;
    /// Création de fichiers (creat, unlink, rename, etc.).
    pub const CPATH:    u64 = 1 << 3;
    /// Accès /tmp.
    pub const TMPPATH:  u64 = 1 << 4;
    /// Sockets TCP/UDP.
    pub const INET:     u64 = 1 << 5;
    /// Sockets Unix Domain.
    pub const UNIX:     u64 = 1 << 6;
    /// Résolution DNS.
    pub const DNS:      u64 = 1 << 7;
    /// Lecture /etc/passwd et /etc/group.
    pub const GETPW:    u64 = 1 << 8;
    /// Création de processus enfants.
    pub const PROC:     u64 = 1 << 9;
    /// execve().
    pub const EXEC:     u64 = 1 << 10;
    /// setuid/setgid/getuid/getgid/etc.
    pub const ID:       u64 = 1 << 11;
    /// Gestion des tables de routage.
    pub const ROUTE:    u64 = 1 << 12;
    /// Mémoire partagée.
    pub const SHM:      u64 = 1 << 13;
    /// Gestion des signaux (sigaction, sigprocmask, etc.).
    pub const SIGNAL:   u64 = 1 << 14;
    /// Appels ioctl limités aux terminaux.
    pub const TTY:      u64 = 1 << 15;
    /// Opérations sur les futex (mutex userspace).
    pub const FUTEX:    u64 = 1 << 16;
    /// Toutes les privilèges (aucune restriction).
    pub const UNRESTRICTED: u64 = !0u64;
}

// ─────────────────────────────────────────────────────────────────────────────
// PledgeSet — état des pledges d'un processus
// ─────────────────────────────────────────────────────────────────────────────

/// État des pledges d'un processus.
#[derive(Clone, Copy)]
pub struct PledgeSet {
    /// Pledges actifs (bitmask).
    active:  u64,
    /// Pledges actifs lors de l'activation initiale (non modifiable).
    initial: u64,
    /// Pledge aktivé (true = pledge() a été appelé).
    enabled: bool,
    /// Compteur de violations.
    violations: u64,
}

impl PledgeSet {
    /// État initial : aucun pledge (non activé = UNRESTRICTED).
    pub const fn new() -> Self {
        Self {
            active:     pledge_flags::UNRESTRICTED,
            initial:    pledge_flags::UNRESTRICTED,
            enabled:    false,
            violations: 0,
        }
    }

    /// État initial avec pledges restreints.
    pub const fn restricted(flags: u64) -> Self {
        Self {
            active:     flags,
            initial:    flags,
            enabled:    true,
            violations: 0,
        }
    }

    /// Active les pledges. Une fois activé, ne peut que se restreindre.
    ///
    /// RÈGLE PLEDGE-01 : `flags` doit être ⊆ `self.active`.
    pub fn pledge(&mut self, flags: u64) -> Result<(), PledgeError> {
        if self.enabled {
            // Vérifier que les nouveaux flags sont un sous-ensemble des actifs
            if flags & !self.active != 0 {
                return Err(PledgeError::CannotExpand);
            }
        }
        self.active  = flags;
        self.enabled = true;
        Ok(())
    }

    /// Vérifie si un pledge flag est actif.
    pub fn has(&self, flag: u64) -> bool {
        if !self.enabled { return true; } // Non activé = tout autorisé
        self.active & flag != 0
    }

    /// Génère la SandboxPolicy correspondant aux pledges actifs.
    pub fn to_sandbox_policy(&self) -> SandboxPolicy {
        if !self.enabled {
            return SandboxPolicy::allow_all();
        }
        let mut policy = SandboxPolicy::deny_all();

        // STDIO : opérations I/O de base
        if self.has(pledge_flags::STDIO) {
            for nr in [
                syscall_nr::READ, syscall_nr::WRITE, syscall_nr::CLOSE,
                syscall_nr::FSTAT, syscall_nr::LSEEK, syscall_nr::MMAP,
                syscall_nr::MUNMAP, syscall_nr::MPROTECT, syscall_nr::MADVISE,
                syscall_nr::GETPID, syscall_nr::GETTID,
                syscall_nr::GETTIMEOFDAY, syscall_nr::NANOSLEEP,
                syscall_nr::FUTEX, syscall_nr::PIPE, syscall_nr::DUP, syscall_nr::DUP2,
            ] {
                policy.allow_syscall(nr);
            }
        }

        // RPATH : lecture fichiers
        if self.has(pledge_flags::RPATH) {
            for nr in [syscall_nr::OPEN, syscall_nr::STAT, syscall_nr::LSTAT] {
                policy.allow_syscall(nr);
            }
        }

        // WPATH : écriture fichiers
        if self.has(pledge_flags::WPATH) {
            policy.allow_syscall(syscall_nr::OPEN);
        }

        // PROC : création processus
        if self.has(pledge_flags::PROC) {
            for nr in [syscall_nr::FORK, syscall_nr::CLONE, syscall_nr::WAIT4] {
                policy.allow_syscall(nr);
            }
        }

        // EXEC : execve
        if self.has(pledge_flags::EXEC) {
            policy.allow_syscall(syscall_nr::EXECVE);
        }

        // INET : sockets réseau
        if self.has(pledge_flags::INET) {
            for nr in [
                syscall_nr::SOCKET, syscall_nr::CONNECT, syscall_nr::BIND,
                syscall_nr::LISTEN, syscall_nr::ACCEPT, syscall_nr::SEND, syscall_nr::RECV,
            ] {
                policy.allow_syscall(nr);
            }
        }

        // ID : uid/gid
        if self.has(pledge_flags::ID) {
            for nr in [syscall_nr::GETUID, syscall_nr::GETGID] {
                policy.allow_syscall(nr);
            }
        }

        // EXIT est toujours autorisé
        policy.allow_syscall(syscall_nr::EXIT);

        // SIGNAL
        if self.has(pledge_flags::SIGNAL) {
            policy.allow_syscall(syscall_nr::RT_SIGACTION);
        }

        // TTY : ioctl terminal limité
        if self.has(pledge_flags::TTY) {
            policy.allow_syscall(syscall_nr::IOCTL);
        }

        // FUTEX : mutex userspace
        if self.has(pledge_flags::FUTEX) {
            policy.allow_syscall(syscall_nr::FUTEX);
        }

        // PTRACE est toujours interdit en mode pledge (RÈGLE PLEDGE-02)
        policy.kill_on_syscall(syscall_nr::PTRACE);

        policy
    }

    /// Vérifie si le pledge est activé.
    pub fn is_enabled(&self) -> bool { self.enabled }

    /// Retourne les flags actifs.
    pub fn active_flags(&self) -> u64 { self.active }

    /// Enregistre une violation de pledge.
    pub fn record_violation(&mut self) {
        self.violations += 1;
        GLOBAL_PLEDGE_VIOLATIONS.fetch_add(1, Ordering::Relaxed);
    }

    /// Retourne le nombre de violations.
    pub fn violations(&self) -> u64 { self.violations }

    /// Dérive un PledgeSet enfant ⊆ self.
    pub fn derive_child(&self) -> PledgeSet {
        PledgeSet {
            active:     self.active,
            initial:    self.initial,
            enabled:    self.enabled,
            violations: 0,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Erreurs pledge
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub enum PledgeError {
    /// Tentative d'élargir les pledges (RÈGLE PLEDGE-01).
    CannotExpand,
    /// Pledge appelé par le processus init (RÈGLE PLEDGE-03).
    InitProcessCannotPledge,
    /// Flags de pledge invalides.
    InvalidFlags,
}

// ─────────────────────────────────────────────────────────────────────────────
// Statistiques globales
// ─────────────────────────────────────────────────────────────────────────────

static GLOBAL_PLEDGE_VIOLATIONS: AtomicU64 = AtomicU64::new(0);

pub fn global_pledge_violations() -> u64 {
    GLOBAL_PLEDGE_VIOLATIONS.load(Ordering::Relaxed)
}
