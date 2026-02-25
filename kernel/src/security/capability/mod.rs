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
pub use token::{CapToken, ObjectId, CapObjectType, TokenStats, read_stats as token_stats};
pub use rights::Rights;
pub use table::{CapTable, CapTableSnapshot, CAP_TABLE_CAPACITY};

// verify — v6 : extraits de revocation.rs
pub use verify::{
    CapError,
    verify,
    verify_and_get_rights,
    verify_typed,
    verify_read,
    verify_read_write,
    verify_ipc_send,
    verify_ipc_recv,
};

// revocation — v6 : uniquement révocation
pub use revocation::{
    revoke,
    revoke_token,
};

pub use delegation::{
    delegate,
    delegate_all,
    delegate_read_only,
    can_delegate,
    DelegationChain,
    DelegationEntry,
};
pub use namespace::{
    CapNamespace,
    NamespaceId,
    alloc_namespace_id,
    cross_namespace_verify,
};

// ─────────────────────────────────────────────────────────────────────────────
// init_capability_subsystem — appelé au boot (étape séquencée)
// ─────────────────────────────────────────────────────────────────────────────

use core::sync::atomic::{AtomicBool, Ordering};

static CAP_INIT_DONE: AtomicBool = AtomicBool::new(false);

/// Initialise le sous-système de capabilities.
///
/// # Boot sequence
/// Appelé par `security::init()` avant toute autre initialisation de couche 2b+.
/// Doit être appelé UNE SEULE fois — panique à la seconde invocation (build debug).
pub fn init_capability_subsystem() {
    if CAP_INIT_DONE.swap(true, Ordering::SeqCst) {
        // Seconde initialisation — erreur architecturale
        panic!("capability: double initialization");
    }
    // Vérifications statiques — compilées dans les assertions const des fichiers
    // Pas de runtime setup nécessaire car tout est statique/atomique
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
            Self::NotFound        => -2,   // ENOENT
            Self::PermissionDenied => -1,  // EPERM
            Self::InvalidArg      => -22,  // EINVAL
            Self::NotSupported    => -38,  // ENOSYS
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Wrappers syscall — appelés depuis syscall/table.rs
// ─────────────────────────────────────────────────────────────────────────────

/// Crée une capability pour un processus cible (syscall exo_cap_create).
///
/// `cap_type` : type d'objet (voir ObjectKind), `rights` : bitmask Rights,
/// `target_pid` : PID destinataire.
/// Retourne un handle opaque (ObjectId encodé en u32) ou une erreur.
pub fn create(cap_type: u32, rights: u32, _target_pid: u32) -> Result<u32, KernelCapError> {
    if !is_initialized() {
        return Err(KernelCapError::NotSupported);
    }
    // Validation du bitmask de droits
    if Rights::from_bits(rights).is_none() {
        return Err(KernelCapError::InvalidArg);
    }
    // Validation du type d'objet (0..=6 défini dans ObjectKind)
    if cap_type > 6 {
        return Err(KernelCapError::InvalidArg);
    }
    // Stub — l'implémentation complète alloue dans la CapTable du processus cible.
    // Pour l'instant : retourne un handle synthétique non nul basé sur cap_type.
    Ok(cap_type.wrapping_add(1))
}

/// Révoque une capability par handle opaque (syscall exo_cap_revoke).
///
/// Wrapper compatible avec l'ancienne ABI `revoke(handle: u32)`.
/// Traduit le handle en ObjectId puis appelle `revocation::revoke()`.
pub fn revoke_handle(_handle: u32) -> Result<(), KernelCapError> {
    if !is_initialized() {
        return Err(KernelCapError::NotSupported);
    }
    // Stub — l'implémentation complète résout le handle vers un ObjectId
    // dans la CapTable du processus courant, puis appelle revoke().
    Ok(())
}

/// Lecture des capabilities POSIX.1e (syscall capget — compat Linux).
///
/// Implémentation stub : retourne NotSupported car ExoOS utilise son propre
/// modèle de capabilities, non POSIX.1e.
#[inline]
pub fn capget(_hdrp: u64, _datap: u64) -> Result<(), KernelCapError> {
    Err(KernelCapError::NotSupported)
}

/// Écriture des capabilities POSIX.1e (syscall capset — compat Linux).
#[inline]
pub fn capset(_hdrp: u64, _datap: u64) -> Result<(), KernelCapError> {
    Err(KernelCapError::NotSupported)
}
