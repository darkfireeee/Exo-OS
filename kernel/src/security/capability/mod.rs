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

/// Générateur d'ObjectId unique, monotoniquement croissant.
static OBJ_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Alloue un ObjectId frais, garanti unique pour la durée de vie du noyau.
#[inline]
fn alloc_object_id() -> token::ObjectId {
    token::ObjectId::from_raw(OBJ_ID_COUNTER.fetch_add(1, Ordering::Relaxed))
}

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
    let rights_val = Rights::from_bits(rights).ok_or(KernelCapError::InvalidArg)?;
    // Validation et conversion du type d'objet
    if cap_type > u16::MAX as u32 {
        return Err(KernelCapError::InvalidArg);
    }
    let obj_type = token::CapObjectType::from_u16(cap_type as u16);
    if obj_type == token::CapObjectType::Invalid {
        return Err(KernelCapError::InvalidArg);
    }

    // Allouer un ObjectId frais et insérer dans la table kernel
    let oid = alloc_object_id();
    let mut guard = KERNEL_CAP_TABLE.lock();
    let table = guard.as_mut().ok_or(KernelCapError::NotSupported)?;
    match table.grant(oid, rights_val, obj_type) {
        Ok(_token) => {
            // Retourner les 32 bits bas de l'ObjectId comme handle opaque.
            Ok(oid.as_u64() as u32)
        }
        Err(_) => Err(KernelCapError::InvalidArg),
    }
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
