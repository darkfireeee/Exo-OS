// kernel/src/security/zero_trust/verify.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ZERO-TRUST VERIFY — Vérification de chaque accès (Zero-Trust)
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLE ZT-VERIFY-01 : verify_access() est appelé pour CHAQUE accès à une
//   ressource sensible — syscall, IPC, device, clé crypto.
//
// RÈGLE ZT-VERIFY-02 : Un refus est TOUJOURS journalisé dans l'audit log TCB.
//
// RÈGLE ZT-VERIFY-03 : En mode strict (SECURITY_STRICT), un DenyAndAlert
//   déclenche une alerte immédiate vers le security monitor (Ring 1).
// ═══════════════════════════════════════════════════════════════════════════════

use super::context::SecurityContext;
use super::labels::SecurityLabel;
use super::policy::{global_policy, AccessRequest, PolicyAction, ResourceKind};
use crate::security::audit;

// ─────────────────────────────────────────────────────────────────────────────
// AccessError — erreur de vérification d'accès
// ─────────────────────────────────────────────────────────────────────────────

/// Erreur retournée par `verify_access()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessError {
    /// Accès refusé (politique).
    Denied,
    /// Accès refusé avec alerte de sécurité levée.
    DeniedAlert,
    /// Contexte de sécurité invalide.
    InvalidContext,
}

impl core::fmt::Display for AccessError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Denied => write!(f, "zero-trust: access denied"),
            Self::DeniedAlert => write!(f, "zero-trust: access denied (security alert raised)"),
            Self::InvalidContext => write!(f, "zero-trust: invalid security context"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// verify_access — point d'entrée principal
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie un accès à une ressource selon la politique Zero-Trust.
///
/// # Paramètres
/// * `subject` — contexte de sécurité du thread demandeur
/// * `resource_kind` — type de la ressource accédée
/// * `object_label` — label de sécurité de la ressource (confidentialité + intégrité)
/// * `is_write` — vrai si écriture, faux si lecture
/// * `context_data` — données supplémentaires pour l'audit (ObjectId, syscall no…)
///
/// # Retourne
/// * `Ok(())` si l'accès est autorisé
/// * `Err(AccessError::Denied)` si refusé
/// * `Err(AccessError::DeniedAlert)` si refusé avec alerte
#[inline]
pub fn verify_access(
    subject: &SecurityContext,
    resource_kind: ResourceKind,
    object_label: SecurityLabel,
    is_write: bool,
    context_data: u64,
) -> Result<(), AccessError> {
    let req = AccessRequest {
        subject,
        resource_kind,
        object_label,
        is_write,
        context_data,
    };

    let decision = global_policy().evaluate(&req);

    match decision {
        PolicyAction::Allow => Ok(()),
        PolicyAction::Deny => {
            audit::log_security_violation(
                subject.principal.pid,
                subject.principal.tid,
                0,
                1,
                context_data.to_le_bytes(),
            );
            Err(AccessError::Denied)
        }
        PolicyAction::DenyAndAudit => {
            audit::log_security_violation(
                subject.principal.pid,
                subject.principal.tid,
                0,
                1,
                context_data.to_le_bytes(),
            );
            Err(AccessError::Denied)
        }
        PolicyAction::DenyAndAlert => {
            audit::log_security_violation(
                subject.principal.pid,
                subject.principal.tid,
                0,
                1,
                context_data.to_le_bytes(),
            );
            // Dans une implémentation complète : notify_security_monitor()
            Err(AccessError::DeniedAlert)
        }
    }
}

/// Raccourci : vérifie un accès en lecture à un fichier.
#[inline(always)]
pub fn verify_file_read(
    subject: &SecurityContext,
    object_label: SecurityLabel,
    inode_id: u64,
) -> Result<(), AccessError> {
    verify_access(
        subject,
        ResourceKind::FileInode,
        object_label,
        false,
        inode_id,
    )
}

/// Raccourci : vérifie un accès en écriture à un fichier.
#[inline(always)]
pub fn verify_file_write(
    subject: &SecurityContext,
    object_label: SecurityLabel,
    inode_id: u64,
) -> Result<(), AccessError> {
    verify_access(
        subject,
        ResourceKind::FileInode,
        object_label,
        true,
        inode_id,
    )
}

/// Raccourci : vérifie un accès IPC.
#[inline(always)]
pub fn verify_ipc_access(
    subject: &SecurityContext,
    ep_label: SecurityLabel,
    endpoint_id: u64,
    is_send: bool,
) -> Result<(), AccessError> {
    verify_access(
        subject,
        ResourceKind::IpcEndpoint,
        ep_label,
        is_send,
        endpoint_id,
    )
}

/// Raccourci : vérifie un accès à une clé cryptographique.
#[inline(always)]
pub fn verify_crypto_key_access(
    subject: &SecurityContext,
    key_label: SecurityLabel,
    key_id: u64,
) -> Result<(), AccessError> {
    verify_access(subject, ResourceKind::CryptoKey, key_label, false, key_id)
}

/// Raccourci : vérifie un accès DMA.
#[inline(always)]
pub fn verify_dma_access(
    subject: &SecurityContext,
    chan_label: SecurityLabel,
    channel_id: u64,
) -> Result<(), AccessError> {
    verify_access(
        subject,
        ResourceKind::DmaChannel,
        chan_label,
        true,
        channel_id,
    )
}

/// Vérifie un appel syscall dans un contexte sandboxé.
#[inline]
pub fn verify_syscall(subject: &SecurityContext, syscall_no: u64) -> Result<(), AccessError> {
    // Syscalls ont le label kernel — vérification de la restriction sandbox
    let kernel_label = SecurityLabel::kernel();
    verify_access(
        subject,
        ResourceKind::Syscall,
        kernel_label,
        false,
        syscall_no,
    )
}
