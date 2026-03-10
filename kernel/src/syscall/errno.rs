//! # syscall/errno.rs — Mapping KernelError / ExofsError → errno POSIX
//!
//! ## Règles (ERRNO-01 à ERRNO-03)
//!
//! - **ERRNO-01** : `rax = -(errno as i64) as u64`. Jamais errno positif dans rax.
//! - **ERRNO-02** : INTERDIT de retourner -1 avec errno=0.
//! - **ERRNO-03** : Ce module DOIT couvrir TOUS les variants de KernelError et ExofsError.
//!
//! ## BUG ERRNO-MISSING — corrigé ici
//! L'ancien code ne listait que 3 cas (NoSpace, NotFound, Denied).
//! Ce module couvre les 15+ variants requis.
//!
//! ## Utilisation
//! ```rust
//! use crate::syscall::errno::kernel_err_to_errno;
//! let retval: i64 = kernel_err_to_errno(KernelError::NotFound);  // → -2 (ENOENT)
//! ```


// ─────────────────────────────────────────────────────────────────────────────
// Import des types d'erreur kernel
// ─────────────────────────────────────────────────────────────────────────────

use crate::fs::exofs::core::ExofsError;

// ─────────────────────────────────────────────────────────────────────────────
// Codes errno POSIX — valeurs NÉGATIVES (rax = -errno)
// ─────────────────────────────────────────────────────────────────────────────

/// Opération non permise (EPERM)
pub const EPERM:       i64 = -1;
/// Objet ou chemin absent (ENOENT)
pub const ENOENT:      i64 = -2;
/// Processus cible inexistant (ESRCH)
pub const ESRCH:       i64 = -3;
/// Signal reçu pendant sleep (EINTR)
pub const EINTR:       i64 = -4;
/// Pas de processus fils (ECHILD)
pub const ECHILD:      i64 = -10;
/// Erreur I/O physique (EIO)
pub const EIO:         i64 = -5;
/// Argument trop grand — len > MAX (E2BIG)
pub const E2BIG:       i64 = -7;
/// Mauvais descripteur de fichier (EBADF)
pub const EBADF:       i64 = -9;
/// Ressource temporairement indisponible / non-bloquant (EAGAIN)
pub const EAGAIN:      i64 = -11;
/// Mémoire insuffisante (ENOMEM)
pub const ENOMEM:      i64 = -12;
/// Permission refusée — capability Denied (EACCES)
pub const EACCES:      i64 = -13;
/// Mauvaise adresse userspace (EFAULT)
pub const EFAULT:      i64 = -14;
/// Ressource occupée (EBUSY)
pub const EBUSY:       i64 = -16;
/// Fichier/objet existe déjà (EEXIST)
pub const EEXIST:      i64 = -17;
/// N'est pas un répertoire (ENOTDIR)
pub const ENOTDIR:     i64 = -20;
/// Est un répertoire (EISDIR)
pub const EISDIR:      i64 = -21;
/// Argument invalide — len=0, ptr null, flags invalides (EINVAL)
pub const EINVAL:      i64 = -22;
/// Trop de fichiers ouverts (EMFILE)
pub const EMFILE:      i64 = -24;
/// Espace disque épuisé (ENOSPC)
pub const ENOSPC:      i64 = -28;
/// Résultat hors plage — overflow (ERANGE)
pub const ERANGE:      i64 = -34;
/// Syscall non implémenté (ENOSYS)
pub const ENOSYS:      i64 = -38;
/// Format de données invalide (EBADMSG)
pub const EBADMSG:     i64 = -74;
/// Débordement arithmétique (EOVERFLOW)
pub const EOVERFLOW:   i64 = -75;
/// Opération non supportée (ENOTSUP / EOPNOTSUPP)
pub const ENOTSUP:     i64 = -95;
/// Attente expirée (ETIMEDOUT)
pub const ETIMEDOUT:   i64 = -110;
/// Quota capability dépassé (EDQUOT)
pub const EDQUOT:      i64 = -122;
/// Clé révoquée (EKEYREV — custom ExoOS)
pub const EKEYREV:     i64 = -126;
/// Format disque incompatible (EPROTO)
pub const EPROTO:      i64 = -71;
/// Epoch inexistante (ENOEPOCH — custom ExoOS)
pub const ENOEPOCH:    i64 = -130;
/// GC table pleine (EGCFULL — custom ExoOS)
pub const EGCFULL:     i64 = -131;
/// Epoch table pleine (EEPOCHFULL — custom ExoOS)
pub const EEPOCHFULL:  i64 = -133;
/// Erreur de commit NVMe (ECOMMIT — custom ExoOS)
pub const ECOMMIT:     i64 = -134;

// ─────────────────────────────────────────────────────────────────────────────
// KernelError — défini localement (miroir de l'enum kernel/process/error)
// ─────────────────────────────────────────────────────────────────────────────

/// Erreurs génériques du kernel — couvre les 15 variants requis par ERRNO-03.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelError {
    /// Objet ou chemin absent.
    NotFound,
    /// Mémoire insuffisante — jamais panic, retourner ENOMEM.
    NoMemory,
    /// Argument invalide (len=0, ptr null, flags invalides).
    InvalidArg,
    /// Espace disque épuisé.
    NoSpace,
    /// Permission refusée (capability Denied).
    AccessDenied,
    /// Corruption détectée (magic, checksum).
    Corrupt,
    /// Objet non exécutable (ObjectKind != Code).
    NotExecutable,
    /// Attente expirée.
    TimedOut,
    /// Non-bloquant + pas de données (O_NONBLOCK).
    WouldBlock,
    /// len > MAX (E2BIG).
    TooBig,
    /// Signal reçu pendant une attente.
    Interrupted,
    /// Syscall non implémenté.
    NotSupported,
    /// Quota capability dépassé.
    QuotaExceeded,
    /// Format de version incompatible.
    VersionMismatch,
    /// Ressource déjà existante.
    AlreadyExists,
    /// Mauvais descripteur de fichier.
    BadFd,
    /// Erreur I/O physique.
    IoError,
    /// Ressource occupée.
    Busy,
}

// ─────────────────────────────────────────────────────────────────────────────
// Mapping KernelError → errno POSIX (ERRNO-03 : couverture COMPLÈTE)
// ─────────────────────────────────────────────────────────────────────────────

/// Convertit une `KernelError` en code errno POSIX négatif.
///
/// RÈGLE ERRNO-01 : la valeur retournée est TOUJOURS négative.
/// RÈGLE ERRNO-03 : TOUS les variants sont couverts.
#[inline]
pub fn kernel_err_to_errno(e: KernelError) -> i64 {
    match e {
        KernelError::NotFound        => ENOENT,
        KernelError::NoMemory        => ENOMEM,
        KernelError::InvalidArg      => EINVAL,
        KernelError::NoSpace         => ENOSPC,
        KernelError::AccessDenied    => EACCES,
        KernelError::Corrupt         => EIO,
        KernelError::NotExecutable   => -8,    // ENOEXEC = 8
        KernelError::TimedOut        => ETIMEDOUT,
        KernelError::WouldBlock      => EAGAIN,
        KernelError::TooBig          => E2BIG,
        KernelError::Interrupted     => EINTR,
        KernelError::NotSupported    => ENOSYS,
        KernelError::QuotaExceeded   => EDQUOT,
        KernelError::VersionMismatch => EPROTO,
        KernelError::AlreadyExists   => EEXIST,
        KernelError::BadFd           => EBADF,
        KernelError::IoError         => EIO,
        KernelError::Busy            => EBUSY,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Mapping ExofsError → errno POSIX (ERRNO-03 : couverture COMPLÈTE)
// ─────────────────────────────────────────────────────────────────────────────

/// Convertit une `ExofsError` en code errno POSIX négatif.
///
/// RÈGLE ERRNO-01 : la valeur retournée est TOUJOURS négative.
/// RÈGLE ERRNO-03 : TOUS les variants ExofsError sont mappés.
#[inline]
pub fn exofs_err_to_errno(e: ExofsError) -> i64 {
    match e {
        ExofsError::NoMemory               => ENOMEM,
        ExofsError::NoSpace                => ENOSPC,
        ExofsError::IoError                => EIO,
        ExofsError::PartialWrite           => EIO,
        ExofsError::OffsetOverflow         => EOVERFLOW,
        ExofsError::InvalidMagic           => EBADMSG,
        ExofsError::ChecksumMismatch       => EBADMSG,
        ExofsError::IncompatibleVersion    => EPROTO,
        ExofsError::CorruptedStructure     => EBADMSG,
        ExofsError::CorruptedChain         => EBADMSG,
        ExofsError::ObjectNotFound         => ENOENT,
        ExofsError::BlobNotFound           => ENOENT,
        ExofsError::ObjectAlreadyExists    => EEXIST,
        ExofsError::WrongObjectKind        => EINVAL,
        ExofsError::WrongObjectClass       => EINVAL,
        ExofsError::InvalidArgument        => EINVAL,
        // FIX: anciens noms remplacés par les variants corrects de ExofsError
        ExofsError::PermissionDenied       => EACCES,
        ExofsError::SecretBlobIdLeakPrevented => EACCES,
        ExofsError::QuotaExceeded          => EDQUOT,
        ExofsError::NoValidEpoch           => ENOEPOCH,
        ExofsError::EpochFull              => EEPOCHFULL,
        ExofsError::GcQueueFull            => EGCFULL,
        ExofsError::CommitInProgress       => ECOMMIT,
        ExofsError::Concurrency            => EBUSY,
        ExofsError::NotSupported           => ENOTSUP,
        ExofsError::PathTooLong            => ERANGE,
        ExofsError::TooManySymlinks        => ERANGE,
        ExofsError::NotADirectory          => ENOTDIR,
        ExofsError::DirectoryNotEmpty      => ENOTSUP,
        ExofsError::InvalidPathComponent   => EINVAL,
        ExofsError::InvalidObjectKind      => EINVAL,
        ExofsError::InvalidObjectClass     => EINVAL,
        ExofsError::ObjectTooLarge         => EOVERFLOW,
        ExofsError::InternalError          => EIO,
        ExofsError::AlreadyMounted         => EEXIST,
        ExofsError::RecoveryFailed         => EIO,
        ExofsError::Corrupt                => EBADMSG,
        ExofsError::CorruptFilesystem      => EBADMSG,
        ExofsError::BadMagic               => EBADMSG,
        ExofsError::MagicMismatch          => EBADMSG,
        ExofsError::BlobIdMismatch         => EBADMSG,
        ExofsError::DataHashMismatch       => EBADMSG,
        ExofsError::Overflow               => EOVERFLOW,
        ExofsError::Underflow              => EOVERFLOW,
        ExofsError::EndOfFile              => EIO,
        ExofsError::UnexpectedEof          => EIO,
        ExofsError::InlineTooLarge         => EOVERFLOW,
        ExofsError::InvalidSize            => EINVAL,
        ExofsError::ShortWrite             => EIO,
        ExofsError::AlreadyExists          => EEXIST,
        ExofsError::InvalidState           => EINVAL,
        ExofsError::AlreadyInitialized     => EBUSY,
        ExofsError::Resource               => ENOMEM,
        ExofsError::NotFound               => ENOENT,
        ExofsError::BufferFull             => ENOSPC,
        ExofsError::IoFailed               => EIO,
        ExofsError::DecompressError        => EIO,
        ExofsError::Logic                  => EIO,
        ExofsError::Shutdown               => EBUSY,
        ExofsError::TooManyPins            => ERANGE,
        ExofsError::InvalidPin             => EINVAL,
        ExofsError::InvalidEpochId         => EINVAL,
        ExofsError::EpochOverflow          => EOVERFLOW,
        ExofsError::EpochSequenceViolation => EINVAL,
        ExofsError::FutureEpoch            => EINVAL,
        ExofsError::NvmeFlushFailed        => EIO,
        ExofsError::RefCountUnderflow      => EIO,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helper générique pour les handlers — retourne i64 depuis Result<i64, E>
// ─────────────────────────────────────────────────────────────────────────────

/// Utilitaire — convertit `Result<i64, KernelError>` en `i64` (0/errno).
#[inline]
pub fn result_to_retval(r: Result<i64, KernelError>) -> i64 {
    match r {
        Ok(v) => v,
        Err(e) => kernel_err_to_errno(e),
    }
}

/// Utilitaire — convertit `Result<i64, ExofsError>` en `i64` (0/errno).
#[inline]
pub fn exofs_result_to_retval(r: Result<i64, ExofsError>) -> i64 {
    match r {
        Ok(v) => v,
        Err(e) => exofs_err_to_errno(e),
    }
}
