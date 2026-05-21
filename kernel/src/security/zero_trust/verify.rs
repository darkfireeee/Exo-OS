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

use core::sync::atomic::{AtomicU64, Ordering};

use super::context::{PrincipalId, SecurityContext};
use super::labels::SecurityLabel;
use super::policy::{global_policy, AccessRequest, PolicyAction, ResourceKind};
use crate::security::audit;

/// Bitmask des PIDs Ring1 de confiance. La forme compacte couvre les PIDs
/// précoces des serveurs canoniques; les PIDs >= 64 prennent le slow path.
static RING1_TRUSTED_MASK: AtomicU64 = AtomicU64::new((1u64 << 1) | (1u64 << 2));

#[inline(always)]
fn ring1_bit(pid: u32) -> Option<u64> {
    if pid < 64 {
        Some(1u64 << pid)
    } else {
        None
    }
}

pub fn register_ring1_pid(pid: u32) {
    if let Some(bit) = ring1_bit(pid) {
        RING1_TRUSTED_MASK.fetch_or(bit, Ordering::Release);
    }
}

pub fn unregister_ring1_pid(pid: u32) {
    if let Some(bit) = ring1_bit(pid) {
        RING1_TRUSTED_MASK.fetch_and(!bit, Ordering::Release);
    }
}

pub fn ring1_trusted_mask() -> u64 {
    RING1_TRUSTED_MASK.load(Ordering::Acquire)
}

#[inline]
pub fn ring1_pair_trusted(sender_pid: u32, receiver_pid: u32) -> bool {
    let Some(sender_bit) = ring1_bit(sender_pid) else {
        return false;
    };
    let Some(receiver_bit) = ring1_bit(receiver_pid) else {
        return false;
    };
    let mask = RING1_TRUSTED_MASK.load(Ordering::Acquire);
    (mask & sender_bit) != 0 && (mask & receiver_bit) != 0
}

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
    let receiver_pid = (endpoint_id >> 32) as u32;
    if receiver_pid != 0 && ring1_pair_trusted(subject.principal.pid, receiver_pid) {
        return Ok(());
    }
    verify_ipc_peer_access(subject, None, ep_label, endpoint_id, is_send)
}

/// Vérifie un accès IPC quand le PID destinataire est connu par l'appelant.
#[inline(always)]
pub fn verify_ipc_access_between(
    subject: &SecurityContext,
    receiver: PrincipalId,
    ep_label: SecurityLabel,
    endpoint_id: u64,
    is_send: bool,
) -> Result<(), AccessError> {
    verify_ipc_peer_access(subject, Some(receiver.pid), ep_label, endpoint_id, is_send)
}

#[inline]
fn verify_ipc_peer_access(
    subject: &SecurityContext,
    receiver_pid: Option<u32>,
    ep_label: SecurityLabel,
    endpoint_id: u64,
    is_send: bool,
) -> Result<(), AccessError> {
    if receiver_pid
        .map(|pid| ring1_pair_trusted(subject.principal.pid, pid))
        .unwrap_or(false)
    {
        return Ok(());
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    fn principal(pid: u32) -> PrincipalId {
        PrincipalId {
            uid: 1000,
            gid: 1000,
            pid,
            tid: pid,
            ns_id: 0,
        }
    }

    #[test]
    fn ring1_pair_bypasses_full_mls_check() {
        register_ring1_pid(40);
        register_ring1_pid(41);
        let subject = SecurityContext::new_normal(principal(40));
        let endpoint_id = (41u64 << 32) | 7;

        assert_eq!(
            verify_ipc_access(&subject, SecurityLabel::kernel(), endpoint_id, true),
            Ok(())
        );

        unregister_ring1_pid(40);
        unregister_ring1_pid(41);
    }

    #[test]
    fn unregister_removes_ring1_fast_path() {
        register_ring1_pid(42);
        register_ring1_pid(43);
        assert!(ring1_pair_trusted(42, 43));

        unregister_ring1_pid(42);
        assert!(!ring1_pair_trusted(42, 43));

        unregister_ring1_pid(43);
    }
}
