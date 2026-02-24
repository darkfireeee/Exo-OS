// kernel/src/process/resource/rlimit.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Limites de ressources POSIX (getrlimit/setrlimit) — Exo-OS Couche 1.5
// ═══════════════════════════════════════════════════════════════════════════════

#![allow(dead_code)]

/// Valeur infinie (illimité).
pub const RLIM_INFINITY: u64 = u64::MAX;

/// Ressources limitées (Linux x86_64).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum RLimitKind {
    CPU     =  0,  // Temps CPU en secondes
    FSIZE   =  1,  // Taille fichier max
    DATA    =  2,  // Segment data max
    STACK   =  3,  // Taille pile utilisateur
    CORE    =  4,  // Taille core dump
    RSS     =  5,  // Résidence mémoire
    NPROC   =  6,  // Nombre de processus
    NOFILE  =  7,  // Nombre de fd ouverts
    MEMLOCK =  8,  // Mémoire lockée
    AS      =  9,  // Espace d'adressage
    LOCKS   = 10,  // Verrous fichier
    SIGPENDING = 11,
    MSGQUEUE   = 12,
    NICE       = 13,
    RTPRIO     = 14,
    RTTIME     = 15,
}

impl RLimitKind {
    pub const COUNT: usize = 16;

    pub fn from_u8(n: u8) -> Option<Self> {
        if n < 16 {
            // SAFETY : n < COUNT, tous les discriminants de 0 à 15 sont valides.
            Some(unsafe { core::mem::transmute(n) })
        } else {
            None
        }
    }
}

/// Paire (soft, hard).
#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct RLimit {
    pub soft: u64,
    pub hard: u64,
}

impl RLimit {
    pub const UNLIMITED: Self = Self { soft: RLIM_INFINITY, hard: RLIM_INFINITY };

    /// Valides par défaut pour un process utilisateur.
    pub const DEFAULT_NOFILE: Self = Self { soft: 1024, hard: 4096 };
    pub const DEFAULT_STACK:  Self = Self { soft: 8 * 1024 * 1024, hard: RLIM_INFINITY };
    pub const DEFAULT_NPROC:  Self = Self { soft: 32768, hard: 32768 };
    pub const DEFAULT_AS:     Self = Self { soft: RLIM_INFINITY, hard: RLIM_INFINITY };

    pub fn is_exceeded(&self, usage: u64) -> bool {
        self.soft != RLIM_INFINITY && usage > self.soft
    }
}

/// Table des limites pour un processus.
#[derive(Clone)]
#[repr(C)]
pub struct RLimitTable {
    limits: [RLimit; RLimitKind::COUNT],
}

impl RLimitTable {
    pub fn new_default() -> Self {
        let mut limits = [RLimit::UNLIMITED; RLimitKind::COUNT];
        limits[RLimitKind::NOFILE as usize]  = RLimit::DEFAULT_NOFILE;
        limits[RLimitKind::STACK  as usize]  = RLimit::DEFAULT_STACK;
        limits[RLimitKind::NPROC  as usize]  = RLimit::DEFAULT_NPROC;
        limits[RLimitKind::AS     as usize]  = RLimit::DEFAULT_AS;
        // RLIMIT_CORE = 0 par défaut (pas de core dump).
        limits[RLimitKind::CORE   as usize]  = RLimit { soft: 0, hard: RLIM_INFINITY };
        Self { limits }
    }

    pub fn get(&self, kind: RLimitKind) -> RLimit {
        self.limits[kind as usize]
    }

    /// setrlimit(2) : modifie une limite.
    /// Règle POSIX : soft <= hard, et root peut élever hard.
    pub fn set(
        &mut self,
        kind:   RLimitKind,
        new:    RLimit,
        is_root: bool,
    ) -> Result<(), RlimitError> {
        let current = self.limits[kind as usize];
        // Vérifications POSIX
        if new.soft > new.hard { return Err(RlimitError::InvalidInput); }
        if !is_root && new.hard > current.hard { return Err(RlimitError::NotPermitted); }
        self.limits[kind as usize] = new;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RlimitError {
    InvalidInput,
    NotPermitted,
    InvalidKind,
}
