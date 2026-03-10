// kernel/src/security/zero_trust/context.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// SECURITY CONTEXT — Contexte de sécurité par thread/processus (Zero-Trust)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Chaque thread possède un SecurityContext décrivant :
//   • Son niveau de confiance (TrustLevel)
//   • Les labels de sécurité MLS-like (confidentialité, intégrité)
//   • L'identité du principal (UID, GID, namespace)
//   • Les restrictions actives (sandbox, pledge)
//
// RÈGLE ZT-01 : Tout accès vérifié DOIT passer par ce contexte.
// RÈGLE ZT-02 : TrustLevel ne peut que DIMINUER — jamais augmenter sans re-authentification.
// RÈGLE ZT-03 : Les contexts fils héritent d'un sous-ensemble du contexte parent.
// ═══════════════════════════════════════════════════════════════════════════════


use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use super::labels::SecurityLabel;

// ─────────────────────────────────────────────────────────────────────────────
// TrustLevel — niveau de confiance
// ─────────────────────────────────────────────────────────────────────────────

/// Niveau de confiance d'un thread.
/// Ordre : System > Kernel > Trusted > Normal > Restricted > Untrusted
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum TrustLevel {
    /// Thread complètement non fiable (sandbox strict).
    Untrusted   = 0,
    /// Thread restreint (pledge actif, syscalls limités).
    Restricted  = 1,
    /// Thread normal (application utilisateur standard).
    Normal      = 2,
    /// Thread de confiance (service système vérifié).
    Trusted     = 3,
    /// Thread noyau (kernel thread).
    Kernel      = 4,
    /// Thread système (init, driver critique).
    System      = 5,
}

impl TrustLevel {
    /// Retourne le niveau maximal accordé à un thread fils (héritage).
    pub fn inherit(self) -> Self {
        // Un fils ne peut pas être plus de confiance que son parent
        match self {
            Self::System    => Self::Trusted,   // fils d'un System = max Trusted
            Self::Kernel    => Self::Trusted,
            Self::Trusted   => Self::Normal,
            Self::Normal    => Self::Normal,
            Self::Restricted => Self::Restricted,
            Self::Untrusted => Self::Untrusted,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PrincipalId — identité d'un principal
// ─────────────────────────────────────────────────────────────────────────────

/// Identité d'un principal (thread ou processus).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrincipalId {
    /// UID POSIX.
    pub uid:   u32,
    /// GID POSIX.
    pub gid:   u32,
    /// PID du processus.
    pub pid:   u32,
    /// TID du thread.
    pub tid:   u32,
    /// Namespace PID (namespace isolation).
    pub ns_id: u32,
}

impl PrincipalId {
    pub const ROOT: Self = Self {
        uid: 0, gid: 0, pid: 0, tid: 0, ns_id: 0
    };

    pub fn is_root(&self) -> bool {
        self.uid == 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SecurityContext — contexte de sécurité d'un thread
// ─────────────────────────────────────────────────────────────────────────────

/// Contexte de sécurité Zero-Trust d'un thread.
///
/// # Invariants (RÈGLE ZT-02)
/// - `trust_level` ne peut que diminuer via `downgrade()`.
/// - `label` ne peut pas être élevé sans passage par une re-auth.
#[repr(C)]
pub struct SecurityContext {
    /// Identité du principal.
    pub principal:    PrincipalId,
    /// Niveau de confiance courant.
    trust_level:      AtomicU32,
    /// Label de sécurité MLS.
    label:            SecurityLabel,
    /// Flags de restrictions actives.
    restrictions:     AtomicU64,
    /// Nombre d'accès refusés depuis la création.
    denied_count:     AtomicU64,
    /// Nombre d'accès accordés.
    allowed_count:    AtomicU64,
}

/// Flags de restrictions.
pub mod restriction_flags {
    /// Restrict : syscalls limités (pledge actif).
    pub const PLEDGE_ACTIVE:    u64 = 1 << 0;
    /// Restrict : aucun réseau.
    pub const NO_NETWORK:       u64 = 1 << 1;
    /// Restrict : aucun accès fichier hors tmpfs.
    pub const NO_FS:            u64 = 1 << 2;
    /// Restrict : aucun fork.
    pub const NO_FORK:          u64 = 1 << 3;
    /// Restrict : aucun exec.
    pub const NO_EXEC:          u64 = 1 << 4;
    /// Restrict : sandbox complet (seccomp-like).
    pub const SANDBOX_FULL:     u64 = 1 << 5;
    /// Restrict : lecture seule FS.
    pub const FS_READONLY:      u64 = 1 << 6;
    /// Restrict : pas de création de processus.
    pub const NO_PROCESS_CREATE: u64 = 1 << 7;
}

// SAFETY: SecurityContext contient uniquement des primitives atomiques.
unsafe impl Send for SecurityContext {}
unsafe impl Sync for SecurityContext {}

impl SecurityContext {
    /// Crée un contexte pour un thread normal.
    pub fn new_normal(principal: PrincipalId) -> Self {
        Self {
            principal,
            trust_level:   AtomicU32::new(TrustLevel::Normal as u32),
            label:         SecurityLabel::user_default(),
            restrictions:  AtomicU64::new(0),
            denied_count:  AtomicU64::new(0),
            allowed_count: AtomicU64::new(0),
        }
    }

    /// Crée un contexte pour un thread noyau.
    pub fn new_kernel(principal: PrincipalId) -> Self {
        Self {
            principal,
            trust_level:   AtomicU32::new(TrustLevel::Kernel as u32),
            label:         SecurityLabel::kernel(),
            restrictions:  AtomicU64::new(0),
            denied_count:  AtomicU64::new(0),
            allowed_count: AtomicU64::new(0),
        }
    }

    /// Retourne le niveau de confiance courant.
    #[inline(always)]
    pub fn trust_level(&self) -> TrustLevel {
        match self.trust_level.load(Ordering::Acquire) {
            0 => TrustLevel::Untrusted,
            1 => TrustLevel::Restricted,
            2 => TrustLevel::Normal,
            3 => TrustLevel::Trusted,
            4 => TrustLevel::Kernel,
            5 => TrustLevel::System,
            _ => TrustLevel::Untrusted,
        }
    }

    /// Dégrade le niveau de confiance — RÈGLE ZT-02.
    /// Ne peut PAS augmenter le niveau.
    pub fn downgrade(&self, new_level: TrustLevel) {
        let current = self.trust_level.load(Ordering::Acquire);
        if (new_level as u32) < current {
            self.trust_level.store(new_level as u32, Ordering::Release);
        }
    }

    /// Retourne le label de sécurité.
    #[inline(always)]
    pub fn label(&self) -> SecurityLabel {
        self.label
    }

    /// Active une restriction.
    pub fn add_restriction(&self, flag: u64) {
        self.restrictions.fetch_or(flag, Ordering::Release);
    }

    /// Retire une restriction (réservé aux opérations d'escalade autorisée).
    pub fn remove_restriction(&self, flag: u64) {
        self.restrictions.fetch_and(!flag, Ordering::Release);
    }

    /// Vérifie si une restriction est active.
    #[inline(always)]
    pub fn has_restriction(&self, flag: u64) -> bool {
        (self.restrictions.load(Ordering::Acquire) & flag) != 0
    }

    /// Incrémente le compteur de refus — pour monitoring ZT.
    #[inline(always)]
    pub(super) fn record_deny(&self) {
        self.denied_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Incrémente le compteur d'accès accordés.
    #[inline(always)]
    pub(super) fn record_allow(&self) {
        self.allowed_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Crée un contexte fils avec niveau hérité (RÈGLE ZT-03).
    pub fn derive_child(&self, child_principal: PrincipalId) -> SecurityContext {
        let child_trust = self.trust_level().inherit();
        SecurityContext {
            principal:    child_principal,
            trust_level:  AtomicU32::new(child_trust as u32),
            label:        self.label.inherit(),
            restrictions: AtomicU64::new(self.restrictions.load(Ordering::Relaxed)),
            denied_count:  AtomicU64::new(0),
            allowed_count: AtomicU64::new(0),
        }
    }

    /// Snapshot de statistiques.
    pub fn stats(&self) -> ContextStats {
        ContextStats {
            denied:   self.denied_count.load(Ordering::Relaxed),
            allowed:  self.allowed_count.load(Ordering::Relaxed),
        }
    }
}

/// Snapshot de stats d'un contexte.
#[derive(Debug, Clone, Copy)]
pub struct ContextStats {
    pub denied:  u64,
    pub allowed: u64,
}
