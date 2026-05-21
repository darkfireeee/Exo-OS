// kernel/src/security/capability/mod.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// CAPABILITY — Module racine (Exo-OS Security · Couche 2b) — v6
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLE CAP-01 : security/capability/ est l'UNIQUE source de vérité pour les capabilities.
// RÈGLE CAP-02 (v6) : ipc/, fs/, process/ accèdent via security::access_control::check_access()
//                     — appel direct à verify() hors de security/ est INTERDIT.
// RÈGLE CAP-03 : Délégation = toujours avec sous-ensemble de droits (invariant CAP-03).
//
// Sous-modules :
//   token      — CapToken (24 bytes, inforgeable, Copy)
//   rights     — Rights bitmask (u32)
//   table      — CapTable par processus (512 slots, lock-free lectures)
//   verify     — verify() UNIQUE point d'entrée + CapError (v6)
//   revocation — revoke() O(1) uniquement (v6)
//   delegation — subdélégation (invariant CAP-03)
//   namespace  — domaines d'ObjectId indépendants
// ═══════════════════════════════════════════════════════════════════════════════

pub mod delegation;
pub mod namespace;
pub mod revocation;
pub mod rights;
pub mod table;
pub mod token;
pub mod verify;

// ── Re-exports publics ────────────────────────────────────────────────────────

// Types fondamentaux
pub use rights::Rights;
pub use table::{CapTable, CapTableSnapshot, CAP_TABLE_CAPACITY};
pub use token::{
    read_stats as token_stats, CapObjectType, CapToken, ObjectId, TokenStats, CAP_TOKEN_WIRE_SIZE,
};

// verify — v6 : extraits de revocation.rs
pub use verify::{
    verify, verify_and_get_rights, verify_ipc_recv, verify_ipc_send, verify_read,
    verify_read_write, verify_typed, CapError,
};

// revocation — v6 : uniquement révocation
pub use revocation::{revoke, revoke_token};

pub use delegation::{
    can_delegate, delegate, delegate_all, delegate_read_only, DelegationChain, DelegationEntry,
};
pub use namespace::{alloc_namespace_id, cross_namespace_verify, CapNamespace, NamespaceId};

// ─────────────────────────────────────────────────────────────────────────────
// init_capability_subsystem — appelé au boot (étape séquencée)
// ─────────────────────────────────────────────────────────────────────────────

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static CAP_INIT_DONE: AtomicBool = AtomicBool::new(false);

// ─────────────────────────────────────────────────────────────────────────────
// Table de capabilities globale (kernel-context)
// ─────────────────────────────────────────────────────────────────────────────

use crate::scheduler::sync::spinlock::SpinLock;

/// Table de capabilities du noyau — utilisée pour les allocations syscall
/// avant que le registre de processus soit disponible.
/// Initialisée par `init_capability_subsystem()`.
static KERNEL_CAP_TABLE: SpinLock<Option<table::CapTable>> = SpinLock::new(None);

/// Métadonnées de capabilities de service Ring 1.
///
/// Les tokens créés via `exo_cap_create()` pour les endpoints IPC sont
/// enregistrés ici pour permettre au noyau de vérifier le couple
/// `{service appelant -> service cible}` lorsqu'un serveur valide un token
/// reçu sur le fil.
const SERVICE_CAP_META_CAPACITY: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ServiceCapMeta {
    object_id: token::ObjectId,
    owner_pid: u32,
    target_pid: u32,
    type_tag: token::CapObjectType,
}

impl ServiceCapMeta {
    const fn empty() -> Self {
        Self {
            object_id: token::ObjectId::INVALID,
            owner_pid: 0,
            target_pid: 0,
            type_tag: token::CapObjectType::Invalid,
        }
    }

    #[inline(always)]
    fn is_free(self) -> bool {
        self.object_id == token::ObjectId::INVALID
    }
}

static SERVICE_CAP_META: SpinLock<[ServiceCapMeta; SERVICE_CAP_META_CAPACITY]> =
    SpinLock::new([ServiceCapMeta::empty(); SERVICE_CAP_META_CAPACITY]);

/// Générateur d'ObjectId unique, monotoniquement croissant.
static OBJ_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Alloue un ObjectId frais, garanti unique pour la durée de vie du noyau.
#[inline]
fn alloc_object_id() -> token::ObjectId {
    token::ObjectId::from_raw(OBJ_ID_COUNTER.fetch_add(1, Ordering::Relaxed))
}

fn insert_service_cap_meta(
    object_id: token::ObjectId,
    owner_pid: u32,
    target_pid: u32,
    type_tag: token::CapObjectType,
) -> Result<(), KernelCapError> {
    let mut metas = SERVICE_CAP_META.lock();
    for slot in metas.iter_mut() {
        if slot.is_free() {
            *slot = ServiceCapMeta {
                object_id,
                owner_pid,
                target_pid,
                type_tag,
            };
            return Ok(());
        }
    }
    Err(KernelCapError::InvalidArg)
}

fn lookup_service_cap_meta(object_id: token::ObjectId) -> Option<ServiceCapMeta> {
    let metas = SERVICE_CAP_META.lock();
    for slot in metas.iter() {
        if slot.object_id == object_id {
            return Some(*slot);
        }
    }
    None
}

fn remove_service_cap_meta(object_id: token::ObjectId) {
    let mut metas = SERVICE_CAP_META.lock();
    for slot in metas.iter_mut() {
        if slot.object_id == object_id {
            *slot = ServiceCapMeta::empty();
            return;
        }
    }
}

/// Initialise le sous-système de capabilities.
///
/// # Boot sequence
/// Appelé par `security::init()` avant toute autre initialisation de couche 2b+.
/// Doit être appelé UNE SEULE fois ; les appels suivants sont idempotents.
pub fn init_capability_subsystem() {
    if CAP_INIT_DONE.swap(true, Ordering::SeqCst) {
        return;
    }
    // Initialiser la table kernel globale
    *KERNEL_CAP_TABLE.lock() = Some(table::CapTable::new());
}

/// Retourne vrai si le sous-système est initialisé.
#[inline(always)]
pub fn is_initialized() -> bool {
    CAP_INIT_DONE.load(Ordering::Acquire)
}

// ─────────────────────────────────────────────────────────────────────────────
// KernelCapError — erreur syscall (to_kernel_errno compatible)
// ─────────────────────────────────────────────────────────────────────────────

/// Erreur retournée par les wrappers syscall de capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelCapError {
    NotFound,
    PermissionDenied,
    InvalidArg,
    NotSupported,
}

impl KernelCapError {
    /// Convertit en code errno négatif (compatible Linux ABI).
    #[inline]
    pub const fn to_kernel_errno(self) -> i32 {
        match self {
            Self::NotFound => -2,         // ENOENT
            Self::PermissionDenied => -1, // EPERM
            Self::InvalidArg => -22,      // EINVAL
            Self::NotSupported => -38,    // ENOSYS
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Wrappers syscall — appelés depuis syscall/table.rs
// ─────────────────────────────────────────────────────────────────────────────

/// Crée une capability de service IPC pour un endpoint cible.
///
/// `cap_type` : type d'objet (seul `IpcEndpoint` est accepté sur ce chemin),
/// `rights` : bitmask Rights, `target_pid` : PID du service cible,
/// `owner_pid` : PID du service qui recevra le token.
///
/// Retourne le `CapToken` émis par le noyau ou une erreur.
pub fn create(
    cap_type: u32,
    rights: u32,
    target_pid: u32,
    owner_pid: u32,
) -> Result<token::CapToken, KernelCapError> {
    if !is_initialized() {
        return Err(KernelCapError::NotSupported);
    }
    if owner_pid == 0 || target_pid == 0 {
        return Err(KernelCapError::InvalidArg);
    }
    // Validation du bitmask de droits
    let rights_val = Rights::from_bits(rights).ok_or(KernelCapError::InvalidArg)?;
    // Validation et conversion du type d'objet
    if cap_type > u16::MAX as u32 {
        return Err(KernelCapError::InvalidArg);
    }
    let obj_type = token::CapObjectType::from_u16(cap_type as u16);
    if obj_type == token::CapObjectType::Invalid {
        return Err(KernelCapError::InvalidArg);
    }
    if obj_type != token::CapObjectType::IpcEndpoint {
        return Err(KernelCapError::NotSupported);
    }
    if !rights_val.contains(Rights::IPC_SEND) {
        return Err(KernelCapError::InvalidArg);
    }

    let verdict = crate::security::check_direct_ipc(
        crate::process::core::pid::Pid(owner_pid),
        crate::process::core::pid::Pid(target_pid),
    );
    match verdict {
        crate::security::IpcPolicyResult::Allowed => {}
        crate::security::IpcPolicyResult::Denied
        | crate::security::IpcPolicyResult::UnknownService => {
            return Err(KernelCapError::PermissionDenied);
        }
    }

    // Allouer un ObjectId frais et insérer dans la table kernel
    let oid = alloc_object_id();
    let mut guard = KERNEL_CAP_TABLE.lock();
    let table = guard.as_mut().ok_or(KernelCapError::NotSupported)?;
    let token = table
        .grant(oid, rights_val, obj_type)
        .map_err(|_| KernelCapError::InvalidArg)?;
    drop(guard);

    if let Err(err) = insert_service_cap_meta(oid, owner_pid, target_pid, obj_type) {
        let guard = KERNEL_CAP_TABLE.lock();
        if let Some(table) = guard.as_ref() {
            revocation::revoke(table, oid);
        }
        return Err(err);
    }

    Ok(token)
}

/// Vérifie un token de service IPC sérialisé émis via `create()`.
///
/// Utilisé par les serveurs Ring 1 qui souhaitent valider qu'une requête IPC
/// porte bien une capability noyau correspondant au service attendu.
pub fn check_token(
    token: token::CapToken,
    required_rights: u32,
    expected_target_pid: u32,
    expected_type: u32,
) -> Result<token::ObjectId, KernelCapError> {
    if !is_initialized() {
        return Err(KernelCapError::NotSupported);
    }
    if expected_target_pid == 0 || expected_type > u16::MAX as u32 {
        return Err(KernelCapError::InvalidArg);
    }

    let rights = Rights::from_bits(required_rights).ok_or(KernelCapError::InvalidArg)?;
    let expected_type = token::CapObjectType::from_u16(expected_type as u16);
    if expected_type == token::CapObjectType::Invalid {
        return Err(KernelCapError::InvalidArg);
    }

    let guard = KERNEL_CAP_TABLE.lock();
    let table = guard.as_ref().ok_or(KernelCapError::NotSupported)?;
    verify::verify_typed(table, token, rights, expected_type)
        .map_err(|_| KernelCapError::PermissionDenied)?;
    drop(guard);

    let meta = lookup_service_cap_meta(token.object_id()).ok_or(KernelCapError::NotFound)?;
    if meta.target_pid != expected_target_pid || meta.type_tag != expected_type {
        return Err(KernelCapError::PermissionDenied);
    }

    Ok(meta.object_id)
}

/// Vérifie un token de service IPC et son propriétaire attendu.
pub fn check_token_owner(
    token: token::CapToken,
    required_rights: u32,
    expected_owner_pid: u32,
    expected_target_pid: u32,
    expected_type: u32,
) -> Result<token::ObjectId, KernelCapError> {
    if expected_owner_pid == 0 {
        return Err(KernelCapError::InvalidArg);
    }
    let object_id = check_token(token, required_rights, expected_target_pid, expected_type)?;
    let meta = lookup_service_cap_meta(token.object_id()).ok_or(KernelCapError::NotFound)?;
    if meta.owner_pid != expected_owner_pid {
        return Err(KernelCapError::PermissionDenied);
    }
    Ok(object_id)
}

/// Révoque une capability par handle opaque (syscall exo_cap_revoke).
///
/// Traduit le handle (u32 = 32 bits bas de l'ObjectId) en ObjectId, puis
/// incrémente atomiquement la génération dans la table kernel — tous les
/// tokens capturant l'ancienne génération retourneront `Err(Revoked)`.
///
/// # Complexité : O(1) (incrément atomique Release, aucun parcours de liste).
pub fn revoke_handle(handle: u32) -> Result<(), KernelCapError> {
    if !is_initialized() {
        return Err(KernelCapError::NotSupported);
    }
    let object_id = token::ObjectId::from_raw(handle as u64);
    let guard = KERNEL_CAP_TABLE.lock();
    let tbl = guard.as_ref().ok_or(KernelCapError::NotSupported)?;
    revocation::revoke(tbl, object_id);
    drop(guard);
    remove_service_cap_meta(object_id);
    Ok(())
}

/// Lecture des capabilities POSIX.1e (syscall capget — compat Linux).
///
/// ExoOS utilise son propre modèle de capabilities (CapToken/Rights/CapTable),
/// incompatible avec POSIX.1e capget/capset. Le retour ENOSYS est
/// le comportement correct et documenté — les applications ExoOS
/// doivent utiliser les syscalls exo_cap_create/exo_cap_revoke à la place.
#[inline]
pub fn capget(_hdrp: u64, _datap: u64) -> Result<(), KernelCapError> {
    Err(KernelCapError::NotSupported)
}

/// Écriture des capabilities POSIX.1e (syscall capset — compat Linux).
#[inline]
pub fn capset(_hdrp: u64, _datap: u64) -> Result<(), KernelCapError> {
    Err(KernelCapError::NotSupported)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::core::pid::Pid;
    use crate::security::ipc_policy::{register_service_class, unregister_service, ServiceClass};

    static INIT: std::sync::Once = std::sync::Once::new();

    fn ensure_capability_init() {
        INIT.call_once(init_capability_subsystem);
    }

    fn register_test_service(pid: u32, class: ServiceClass) {
        let pid = Pid(pid);
        let _ = unregister_service(pid);
        assert!(register_service_class(pid, class));
    }

    fn unregister_test_service(pid: u32) {
        let _ = unregister_service(Pid(pid));
    }

    #[test]
    fn service_token_roundtrip_for_allowed_route() {
        ensure_capability_init();
        const CRYPTO_PID: u32 = 1101;
        const EXO_SHIELD_PID: u32 = 1102;

        register_test_service(CRYPTO_PID, ServiceClass::CryptoServer);
        register_test_service(EXO_SHIELD_PID, ServiceClass::ExoShield);

        let token = create(
            CapObjectType::IpcEndpoint as u32,
            Rights::IPC_SEND.bits(),
            CRYPTO_PID,
            EXO_SHIELD_PID,
        )
        .expect("exo_shield -> crypto_server token");

        let verified = check_token(
            token,
            Rights::IPC_SEND.bits(),
            CRYPTO_PID,
            CapObjectType::IpcEndpoint as u32,
        )
        .expect("token accepted");

        assert_eq!(verified, token.object_id());
        let owner_verified = check_token_owner(
            token,
            Rights::IPC_SEND.bits(),
            EXO_SHIELD_PID,
            CRYPTO_PID,
            CapObjectType::IpcEndpoint as u32,
        )
        .expect("owner token accepted");
        assert_eq!(owner_verified, token.object_id());
        assert_eq!(
            check_token_owner(
                token,
                Rights::IPC_SEND.bits(),
                9999,
                CRYPTO_PID,
                CapObjectType::IpcEndpoint as u32,
            ),
            Err(KernelCapError::PermissionDenied)
        );
        let _ = revoke_handle(token.object_id().as_u64() as u32);
        unregister_test_service(CRYPTO_PID);
        unregister_test_service(EXO_SHIELD_PID);
    }

    #[test]
    fn service_token_creation_respects_ipc_policy() {
        ensure_capability_init();
        const CRYPTO_PID: u32 = 1111;
        const NETWORK_PID: u32 = 1112;

        register_test_service(CRYPTO_PID, ServiceClass::CryptoServer);
        register_test_service(NETWORK_PID, ServiceClass::NetworkServer);

        let result = create(
            CapObjectType::IpcEndpoint as u32,
            Rights::IPC_SEND.bits(),
            NETWORK_PID,
            CRYPTO_PID,
        );

        assert_eq!(result, Err(KernelCapError::PermissionDenied));
        unregister_test_service(CRYPTO_PID);
        unregister_test_service(NETWORK_PID);
    }
}
