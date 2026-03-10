// kernel/src/security/isolation/domains.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Domaines de sécurité — Isolation par domaine d'exécution
// ═══════════════════════════════════════════════════════════════════════════════
//
// Architecture :
//   • Chaque thread/processus appartient à UN domaine de sécurité unique
//   • Les transitions sont strictement unidirectionnelles (Kernel→Driver→User→Sandbox)
//   • Les accès cross-domain sont filtrés par la zero_trust policy
//   • Instrumentation : compteurs atomiques par transition
//
// Domaines :
//   Kernel(0)  : code kernel — anneau 0, accès total
//   Driver(1)  : pilotes certifiés — anneau 0 isolé, accès MMIO/DMA limité
//   User(2)    : processus utilisateur normaux — anneau 3
//   Sandbox(3) : processus sandboxés — anneau 3, syscalls filtrés
// ═══════════════════════════════════════════════════════════════════════════════


use core::sync::atomic::{AtomicU64, AtomicU32, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// SecurityDomain — identifiant de domaine
// ─────────────────────────────────────────────────────────────────────────────

/// Domaine de sécurité d'un thread ou processus.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SecurityDomain {
    /// Kernel pur — anneau 0, aucune restriction.
    Kernel   = 0,
    /// Pilote certifié — anneau 0 avec politique IOMMU/MMIO.
    Driver   = 1,
    /// Processus utilisateur standard — anneau 3.
    User     = 2,
    /// Processus sandboxé — anneau 3, pledge() + syscall filter.
    Sandbox  = 3,
}

impl SecurityDomain {
    /// Retourne le niveau de confiance numérique du domaine.
    pub fn trust_level(&self) -> u32 {
        match self {
            SecurityDomain::Kernel  => 100,
            SecurityDomain::Driver  => 75,
            SecurityDomain::User    => 50,
            SecurityDomain::Sandbox => 10,
        }
    }

    /// Vérifie si ce domaine peut accéder au domaine `target`.
    ///
    /// Règle : un domaine de confiance inférieure ne peut PAS accéder
    /// à un domaine de confiance supérieure sans gate explicite.
    pub fn can_access(&self, target: SecurityDomain) -> bool {
        self.trust_level() >= target.trust_level()
    }

    /// Vérifie si une transition de `self` → `next` est autorisée.
    ///
    /// Les transitions autorisées via gate :
    ///   User → Kernel (syscall), User → Driver (ioctl via /dev)
    ///   Driver → Kernel (driver_call)
    ///   Sandbox → User est INTERDIT (isolation stricte)
    pub fn is_transition_allowed(&self, next: SecurityDomain) -> bool {
        match (self, next) {
            // Kernel peut aller partout
            (SecurityDomain::Kernel, _) => true,
            // Driver → Kernel via gate driver_call
            (SecurityDomain::Driver, SecurityDomain::Kernel) => true,
            // User → Kernel via syscall (normal)
            (SecurityDomain::User, SecurityDomain::Kernel) => true,
            // Sandbox → Kernel via syscall (filtré par sandbox policy)
            (SecurityDomain::Sandbox, SecurityDomain::Kernel) => true,
            // Toute autre transition est refusée
            _ => false,
        }
    }

    /// Encode le domaine sur un u8.
    pub fn as_u8(&self) -> u8 {
        *self as u8
    }

    /// Décode depuis un u8.
    pub fn from_u8(v: u8) -> Option<SecurityDomain> {
        match v {
            0 => Some(SecurityDomain::Kernel),
            1 => Some(SecurityDomain::Driver),
            2 => Some(SecurityDomain::User),
            3 => Some(SecurityDomain::Sandbox),
            _ => None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DomainContext — contexte d'exécution d'un thread dans un domaine
// ─────────────────────────────────────────────────────────────────────────────

/// Contexte de domaine par thread — stocké dans le TCB du thread.
#[repr(C)]
pub struct DomainContext {
    /// Domaine actuel.
    pub domain:          SecurityDomain,
    /// Domaine d'origine (avant la dernière transition).
    pub previous_domain: SecurityDomain,
    /// PID du thread propriétaire.
    pub owner_pid:       u32,
    /// TID du thread propriétaire.
    pub owner_tid:       u32,
    /// Nombre de transitions de domaine effectuées.
    pub transition_count: AtomicU32,
    /// Flags additionnels (bitmask).
    pub flags:           AtomicU64,
}

/// Flags de domaine context.
pub mod domain_flags {
    /// Thread en cours de transition de domaine.
    pub const TRANSITIONING:   u64 = 1 << 0;
    /// Thread est un thread kernel interne.
    pub const KERNEL_THREAD:   u64 = 1 << 1;
    /// Thread a été créé via fork() depuis un Sandbox.
    pub const SANDBOX_ORIGIN:  u64 = 1 << 2;
    /// Thread est en état de débogage.
    pub const UNDER_PTRACE:    u64 = 1 << 3;
    /// Isolation mémoire renforcée (PKRU activé).
    pub const PKRU_ISOLATED:   u64 = 1 << 4;
}

impl DomainContext {
    /// Crée un contexte de domaine pour un thread kernel.
    pub fn new_kernel(pid: u32, tid: u32) -> Self {
        Self {
            domain:          SecurityDomain::Kernel,
            previous_domain: SecurityDomain::Kernel,
            owner_pid:       pid,
            owner_tid:       tid,
            transition_count: AtomicU32::new(0),
            flags:           AtomicU64::new(domain_flags::KERNEL_THREAD),
        }
    }

    /// Crée un contexte de domaine pour un processus utilisateur.
    pub fn new_user(pid: u32, tid: u32) -> Self {
        Self {
            domain:          SecurityDomain::User,
            previous_domain: SecurityDomain::Kernel,
            owner_pid:       pid,
            owner_tid:       tid,
            transition_count: AtomicU32::new(0),
            flags:           AtomicU64::new(0),
        }
    }

    /// Crée un contexte de domaine pour un processus sandboxé.
    pub fn new_sandbox(pid: u32, tid: u32) -> Self {
        Self {
            domain:          SecurityDomain::Sandbox,
            previous_domain: SecurityDomain::User,
            owner_pid:       pid,
            owner_tid:       tid,
            transition_count: AtomicU32::new(0),
            flags:           AtomicU64::new(domain_flags::SANDBOX_ORIGIN),
        }
    }

    /// Effectue une transition vers `new_domain`.
    /// Retourne Err si la transition n'est pas autorisée.
    pub fn transition_to(&mut self, new_domain: SecurityDomain) -> Result<(), DomainError> {
        if !self.domain.is_transition_allowed(new_domain) {
            DOMAIN_STATS.forbidden_transitions.fetch_add(1, Ordering::Relaxed);
            return Err(DomainError::TransitionNotAllowed {
                from: self.domain,
                to:   new_domain,
            });
        }
        self.flags.fetch_or(domain_flags::TRANSITIONING, Ordering::Release);
        self.previous_domain = self.domain;
        self.domain = new_domain;
        self.transition_count.fetch_add(1, Ordering::Relaxed);
        self.flags.fetch_and(!domain_flags::TRANSITIONING, Ordering::Release);
        DOMAIN_STATS.successful_transitions.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Retourne au domaine précédent (return from syscall).
    pub fn return_to_previous(&mut self) {
        let prev = self.previous_domain;
        self.domain = prev;
        self.transition_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Vérifie si le contexte est dans le domaine Kernel.
    #[inline(always)]
    pub fn is_kernel(&self) -> bool {
        self.domain == SecurityDomain::Kernel
    }

    /// Vérifie si le contexte est sandboxé.
    #[inline(always)]
    pub fn is_sandboxed(&self) -> bool {
        self.domain == SecurityDomain::Sandbox
    }

    /// Ajoute un flag.
    pub fn set_flag(&self, flag: u64) {
        self.flags.fetch_or(flag, Ordering::Relaxed);
    }

    /// Retire un flag.
    pub fn clear_flag(&self, flag: u64) {
        self.flags.fetch_and(!flag, Ordering::Relaxed);
    }

    /// Vérifie un flag.
    pub fn has_flag(&self, flag: u64) -> bool {
        self.flags.load(Ordering::Relaxed) & flag != 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Erreurs de domaine
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub enum DomainError {
    /// Transition de domaine non autorisée.
    TransitionNotAllowed {
        from: SecurityDomain,
        to:   SecurityDomain,
    },
    /// Accès cross-domain refusé.
    CrossDomainAccessDenied,
    /// Domaine inconnu.
    UnknownDomain,
}

// ─────────────────────────────────────────────────────────────────────────────
// Statistiques globales des domaines
// ─────────────────────────────────────────────────────────────────────────────

struct DomainStats {
    successful_transitions:  AtomicU64,
    forbidden_transitions:   AtomicU64,
    cross_domain_denials:    AtomicU64,
}

static DOMAIN_STATS: DomainStats = DomainStats {
    successful_transitions: AtomicU64::new(0),
    forbidden_transitions:  AtomicU64::new(0),
    cross_domain_denials:   AtomicU64::new(0),
};

/// Retourne les statistiques de domaine sous forme de snapshot.
#[derive(Debug, Clone, Copy)]
pub struct DomainStatsSnapshot {
    pub successful_transitions: u64,
    pub forbidden_transitions:  u64,
    pub cross_domain_denials:   u64,
}

pub fn read_domain_stats() -> DomainStatsSnapshot {
    DomainStatsSnapshot {
        successful_transitions: DOMAIN_STATS.successful_transitions.load(Ordering::Relaxed),
        forbidden_transitions:  DOMAIN_STATS.forbidden_transitions.load(Ordering::Relaxed),
        cross_domain_denials:   DOMAIN_STATS.cross_domain_denials.load(Ordering::Relaxed),
    }
}
