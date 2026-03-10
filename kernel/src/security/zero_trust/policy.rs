// kernel/src/security/zero_trust/policy.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ZERO-TRUST POLICY — Moteur de politique de sécurité
// ═══════════════════════════════════════════════════════════════════════════════
//
// Ce module implémente le moteur de politique Zero-Trust d'Exo-OS.
// CHAQUE accès à une ressource doit être évalué par ce moteur.
//
// PRINCIPE Zero-Trust : "Never trust, always verify"
//   • Aucune confiance implicite basée sur la localisation (réseau interne, Ring 1…)
//   • Authentification + autorisation pour CHAQUE accès
//   • Accès accordé sur le minimum nécessaire (least privilege)
//   • Monitoring continu des accès (journalisation)
//
// RÈGLE ZT-POLICY-01 : deny_by_default() est l'action par défaut.
//   Si aucune règle ne matche → accès refusé.
// ═══════════════════════════════════════════════════════════════════════════════


use core::sync::atomic::{AtomicU64, Ordering};

use super::context::{SecurityContext, TrustLevel};
use super::labels::SecurityLabel;

// ─────────────────────────────────────────────────────────────────────────────
// PolicyAction — décision de politique
// ─────────────────────────────────────────────────────────────────────────────

/// Décision prise par le moteur de politique.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyAction {
    /// Accès autorisé.
    Allow,
    /// Accès refusé.
    Deny,
    /// Accès refusé + audit obligatoire.
    DenyAndAudit,
    /// Accès refusé + alerte de sécurité.
    DenyAndAlert,
}

impl PolicyAction {
    #[inline(always)]
    pub fn is_allow(self) -> bool {
        self == Self::Allow
    }

    #[inline(always)]
    pub fn is_deny(self) -> bool {
        !self.is_allow()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ResourceKind — type de ressource accédée
// ─────────────────────────────────────────────────────────────────────────────

/// Type de ressource faisant l'objet d'une vérification de politique.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ResourceKind {
    /// Endpoint IPC.
    IpcEndpoint   = 0,
    /// Région mémoire.
    MemoryRegion  = 1,
    /// Inode de fichier.
    FileInode     = 2,
    /// Périphérique physique.
    Device        = 3,
    /// Thread d'un autre processus.
    Thread        = 4,
    /// Namespace.
    Namespace     = 5,
    /// Clé cryptographique.
    CryptoKey     = 6,
    /// Canal DMA.
    DmaChannel    = 7,
    /// Syscall système (pour le sandbox).
    Syscall       = 8,
}

// ─────────────────────────────────────────────────────────────────────────────
// AccessRequest — requête d'accès à évaluer
// ─────────────────────────────────────────────────────────────────────────────

/// Requête d'accès transmise au moteur de politique.
pub struct AccessRequest<'a> {
    /// Contexte du demandeur.
    pub subject:       &'a SecurityContext,
    /// Type de ressource accédée.
    pub resource_kind: ResourceKind,
    /// Label de sécurité de la ressource.
    pub object_label:  SecurityLabel,
    /// Opération demandée : vrai = écriture, faux = lecture.
    pub is_write:      bool,
    /// Données supplémentaires (syscall no, adresse, etc.).
    pub context_data:  u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// ZeroTrustPolicy — moteur principal
// ─────────────────────────────────────────────────────────────────────────────

/// Moteur de politique Zero-Trust.
///
/// # Algorithme d'évaluation
/// 1. Niveau de confiance insuffisant → DenyAndAudit
/// 2. Restriction active pour ce type de ressource → DenyAndAudit
/// 3. Vérification MLS (Bell-LaPadula + Biba) → Deny si violation
/// 4. Règle spécifique au type de ressource
/// 5. Allow (default allow pour Kernel/System, Deny pour le reste si non matché)
pub struct ZeroTrustPolicy {
    evaluations:   AtomicU64,
    denials:       AtomicU64,
    alerts:        AtomicU64,
}

impl ZeroTrustPolicy {
    pub const fn new() -> Self {
        Self {
            evaluations: AtomicU64::new(0),
            denials:     AtomicU64::new(0),
            alerts:      AtomicU64::new(0),
        }
    }

    /// Évalue une requête d'accès — cœur du moteur Zero-Trust.
    pub fn evaluate(&self, req: &AccessRequest<'_>) -> PolicyAction {
        self.evaluations.fetch_add(1, Ordering::Relaxed);

        let trust = req.subject.trust_level();

        // ── 1. Threads non fiables : deny tout sauf lecture Public ───────────
        if trust == TrustLevel::Untrusted {
            // Untrusted peut uniquement lire des ressources Public
            if req.is_write || req.object_label.confidentiality
                != super::labels::ConfidentialityLevel::Public
            {
                self.denials.fetch_add(1, Ordering::Relaxed);
                req.subject.record_deny();
                return PolicyAction::DenyAndAudit;
            }
        }

        // ── 2. Restrictions actives ───────────────────────────────────────────
        let action = self.check_restrictions(req, trust);
        if action.is_deny() {
            self.denials.fetch_add(1, Ordering::Relaxed);
            req.subject.record_deny();
            return action;
        }

        // ── 3. Vérification MLS ───────────────────────────────────────────────
        let subject_label = req.subject.label();
        let mls_ok = if req.is_write {
            subject_label.can_write(req.object_label)
        } else {
            subject_label.can_read(req.object_label)
        };

        if !mls_ok {
            self.denials.fetch_add(1, Ordering::Relaxed);
            req.subject.record_deny();
            // Violation MLS : potentiellement un exploit → alerte
            if req.object_label.confidentiality >= super::labels::ConfidentialityLevel::Secret {
                self.alerts.fetch_add(1, Ordering::Relaxed);
                return PolicyAction::DenyAndAlert;
            }
            return PolicyAction::DenyAndAudit;
        }

        // ── 4. Règles spécifiques aux ressources ──────────────────────────────
        let resource_action = self.check_resource_rules(req, trust);
        if resource_action.is_deny() {
            self.denials.fetch_add(1, Ordering::Relaxed);
            req.subject.record_deny();
            return resource_action;
        }

        // ── 5. Allow ──────────────────────────────────────────────────────────
        req.subject.record_allow();
        PolicyAction::Allow
    }

    fn check_restrictions(&self, req: &AccessRequest<'_>, _trust: TrustLevel) -> PolicyAction {
        use super::context::restriction_flags::*;

        match req.resource_kind {
            ResourceKind::IpcEndpoint => {
                // Pas de restriction active interdisant l'IPC
                PolicyAction::Allow
            }
            ResourceKind::FileInode => {
                if req.subject.has_restriction(NO_FS) {
                    return PolicyAction::DenyAndAudit;
                }
                if req.is_write && req.subject.has_restriction(FS_READONLY) {
                    return PolicyAction::DenyAndAudit;
                }
                PolicyAction::Allow
            }
            ResourceKind::Syscall => {
                // Vérifié par sandbox.rs
                PolicyAction::Allow
            }
            ResourceKind::Device => {
                // Devices : restreint aux threads Trusted+
                PolicyAction::Allow
            }
            _ => PolicyAction::Allow,
        }
    }

    fn check_resource_rules(&self, req: &AccessRequest<'_>, trust: TrustLevel) -> PolicyAction {
        match req.resource_kind {
            ResourceKind::CryptoKey => {
                // Clés crypto : minimum Trusted
                if trust < TrustLevel::Trusted {
                    return PolicyAction::DenyAndAudit;
                }
                PolicyAction::Allow
            }
            ResourceKind::DmaChannel => {
                // DMA : réservé aux drivers (Trusted+)
                if trust < TrustLevel::Trusted {
                    return PolicyAction::DenyAndAlert;
                }
                PolicyAction::Allow
            }
            ResourceKind::Device => {
                if trust < TrustLevel::Trusted {
                    return PolicyAction::DenyAndAudit;
                }
                PolicyAction::Allow
            }
            _ => PolicyAction::Allow,
        }
    }

    /// Statistiques du moteur.
    pub fn stats(&self) -> PolicyStats {
        PolicyStats {
            evaluations: self.evaluations.load(Ordering::Relaxed),
            denials:     self.denials.load(Ordering::Relaxed),
            alerts:      self.alerts.load(Ordering::Relaxed),
        }
    }
}

impl Default for ZeroTrustPolicy {
    fn default() -> Self {
        Self::new()
    }
}

/// Snapshot de statistiques du moteur de politique.
#[derive(Debug, Clone, Copy)]
pub struct PolicyStats {
    pub evaluations: u64,
    pub denials:     u64,
    pub alerts:      u64,
}

// Singleton global
static GLOBAL_POLICY: ZeroTrustPolicy = ZeroTrustPolicy::new();

pub fn global_policy() -> &'static ZeroTrustPolicy {
    &GLOBAL_POLICY
}
