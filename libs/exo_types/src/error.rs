// libs/exo-types/src/error.rs
//
// Fichier : libs/exo_types/src/error.rs
// Rôle    : ExoError — codes d'erreur unifiés tous rings.
//
// SOURCE DE VÉRITÉ : ExoOS_Architecture_v7.md, ExoOS_Kernel_Types_v10.md

/// Codes d'erreur unifiés ExoOS — valides Ring 0, Ring 1 et Ring 3.
///
/// Compatible POSIX partiel : les codes ≥ 1024 sont des extensions ExoOS.
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExoError {
    // ─── Succès ──────────────────────────────────────────────────────────
    Ok              =    0,

    // ─── POSIX standard ──────────────────────────────────────────────────
    /// Opération non permise.
    PermissionDenied =   1,  // EPERM
    /// Fichier ou répertoire inexistant.
    NotFound         =   2,  // ENOENT
    /// Entrée/sortie.
    Io               =   5,  // EIO
    /// Argument invalide.
    InvalidArg       =  22,  // EINVAL
    /// Espace disque épuisé.
    NoSpace          =  28,  // ENOSPC
    /// Dépassement d'intervalle.
    Overflow         =  75,  // EOVERFLOW
    /// Non implémenté.
    NotImplemented   =  38,  // ENOSYS
    /// Temporairement non disponible.
    Again            =  11,  // EAGAIN
    /// Mémoire insuffisante.
    OutOfMemory      =  12,  // ENOMEM
    /// Trop de fichiers ouverts.
    TooManyFiles     =  24,  // EMFILE
    /// Fichier trop grand.
    FileTooLarge     =  27,  // EFBIG
    /// Table de fichiers pleine.
    TableFull        =  23,  // ENFILE

    // ─── Extensions ExoOS (≥ 1024) ───────────────────────────────────────
    /// CapToken invalide ou révoqué.
    CapInvalid       = 1024,
    /// CapToken type mismatch.
    CapTypeMismatch  = 1025,
    /// ObjectId inconnu dans ExoFS.
    ObjectNotFound   = 1026,
    /// Syscall ExoFS inconnu (numéro hors plage 500-519).
    InvalidSyscall   = 1027,
    /// Offset en dehors des bornes de l'objet ExoFS.
    OffsetOverflow   = 1028,
    /// BDF PCI déjà claimé (CORR-32).
    AlreadyClaimed   = 1029,
    /// Region physique non dans la whitelist MMIO.
    NotInHardwareRegion = 1030,
    /// Adresse physique est de la RAM (interdit pour MMIO claim).
    PhysIsRam        = 1031,
    /// Table de claims drivers pleine.
    ClaimTableFull   = 1032,
    /// Server Ring 1 non encore disponible (en cours de démarrage).
    ServiceNotReady  = 1033,
    /// Timeout expiré (ExoPhoenix freeze timeout — CORR-37).
    Timeout          = 1034,
    /// Quota dépassé (ExoFS — CORR-47).
    QuotaExceeded    = 1035,
    /// ObjectId secret — opération get_content_hash refusée (S-09).
    SecretObject     = 1036,
}

impl ExoError {
    /// Retourne `true` si c'est un succès.
    #[inline(always)]
    pub fn is_ok(self) -> bool {
        self == ExoError::Ok
    }
}
