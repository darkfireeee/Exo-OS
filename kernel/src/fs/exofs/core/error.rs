// kernel/src/fs/exofs/core/error.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ExofsError — erreurs internes ExoFS + mapping vers FsError VFS
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════

use core::fmt;
use crate::fs::core::types::FsError;

// ─────────────────────────────────────────────────────────────────────────────
// ExofsError
// ─────────────────────────────────────────────────────────────────────────────

/// Erreurs internes du module ExoFS.
///
/// Toutes les fonctions kernel ExoFS retournent `Result<T, ExofsError>`.
/// La conversion vers `FsError` (VFS) est assurée par `From<ExofsError> for FsError`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExofsError {
    // ── Mémoire ──────────────────────────────────────────────────────────────
    /// Allocation mémoire échouée (allocateur fallible).
    NoMemory,

    // ── Disque ───────────────────────────────────────────────────────────────
    /// Espace disque insuffisant.
    NoSpace,
    /// Erreur I/O disque (lecture ou écriture échouée).
    IoError,
    /// Écriture partielle : bytes_written ≠ expected (règle WRITE-01).
    PartialWrite,
    /// Overflow d'adresse disque (checked_add → None, règle ARITH-01).
    OffsetOverflow,

    // ── Format / Corruption ──────────────────────────────────────────────────
    /// Magic number incorrect en tête de structure on-disk.
    InvalidMagic,
    /// Checksum Blake3 invalide.
    ChecksumMismatch,
    /// Version de format incompatible.
    IncompatibleVersion,
    /// Structure on-disk corrompue (champ hors limite).
    CorruptedStructure,
    /// Page chainée EpochRoot : magic ou checksum invalide (règle CHAIN-01).
    CorruptedChain,

    // ── Objets ───────────────────────────────────────────────────────────────
    /// ObjectId introuvable dans la table.
    ObjectNotFound,
    /// BlobId introuvable dans le registry.
    BlobNotFound,
    /// L'objet existe déjà (doublon).
    ObjectAlreadyExists,
    /// Type d'objet incompatible avec l'opération demandée.
    WrongObjectKind,
    /// Classe d'objet incompatible (ex : write sur Class1 immuable).
    WrongObjectClass,

    // ── Chemins ──────────────────────────────────────────────────────────────
    /// Composant de chemin invalide (trop long, caractère interdit).
    InvalidPathComponent,
    /// Chemin trop long (> PATH_MAX).
    PathTooLong,
    /// Profondeur de symlinks dépassée (> SYMLINK_MAX_DEPTH).
    TooManySymlinks,
    /// Répertoire non vide.
    DirectoryNotEmpty,
    /// La cible n'est pas un répertoire.
    NotADirectory,

    // ── Sécurité / Capabilities ──────────────────────────────────────────────
    /// CapToken invalide ou révoqué.
    PermissionDenied,
    /// Quota capability dépassé.
    QuotaExceeded,
    /// Tentative d'exposition du BlobId d'un Secret (règle SEC-07).
    SecretBlobIdLeakPrevented,

    // ── Epoch ────────────────────────────────────────────────────────────────
    /// Aucun Epoch valide trouvé au recovery.
    NoValidEpoch,
    /// Epoch plein (> EPOCH_MAX_OBJECTS).
    EpochFull,
    /// Conflit de commit (EPOCH_COMMIT_LOCK pris).
    CommitInProgress,

    // ── GC ───────────────────────────────────────────────────────────────────
    /// File grise GC pleine (> GC_MAX_GREY_QUEUE).
    GcQueueFull,
    /// ref_count underflow détecté (règle REFCNT-01 — panic normalement).
    RefCountUnderflow,

    // ── Générique ────────────────────────────────────────────────────────────
    /// Opération non supportée.
    NotSupported,
    /// Paramètre invalide passé à la fonction.
    InvalidArgument,
    /// Erreur interne inattendue (bug kernel).
    InternalError,
}

impl fmt::Display for ExofsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoMemory                  => write!(f, "exofs: no memory"),
            Self::NoSpace                   => write!(f, "exofs: no space"),
            Self::IoError                   => write!(f, "exofs: I/O error"),
            Self::PartialWrite              => write!(f, "exofs: partial write"),
            Self::OffsetOverflow            => write!(f, "exofs: disk offset overflow"),
            Self::InvalidMagic              => write!(f, "exofs: invalid magic"),
            Self::ChecksumMismatch          => write!(f, "exofs: checksum mismatch"),
            Self::IncompatibleVersion       => write!(f, "exofs: incompatible version"),
            Self::CorruptedStructure        => write!(f, "exofs: corrupted structure"),
            Self::CorruptedChain            => write!(f, "exofs: corrupted page chain"),
            Self::ObjectNotFound            => write!(f, "exofs: object not found"),
            Self::BlobNotFound              => write!(f, "exofs: blob not found"),
            Self::ObjectAlreadyExists       => write!(f, "exofs: object already exists"),
            Self::WrongObjectKind           => write!(f, "exofs: wrong object kind"),
            Self::WrongObjectClass          => write!(f, "exofs: wrong object class"),
            Self::InvalidPathComponent      => write!(f, "exofs: invalid path component"),
            Self::PathTooLong               => write!(f, "exofs: path too long"),
            Self::TooManySymlinks           => write!(f, "exofs: too many symlinks"),
            Self::DirectoryNotEmpty         => write!(f, "exofs: directory not empty"),
            Self::NotADirectory             => write!(f, "exofs: not a directory"),
            Self::PermissionDenied          => write!(f, "exofs: permission denied"),
            Self::QuotaExceeded             => write!(f, "exofs: quota exceeded"),
            Self::SecretBlobIdLeakPrevented => write!(f, "exofs: secret blob-id protected"),
            Self::NoValidEpoch              => write!(f, "exofs: no valid epoch found"),
            Self::EpochFull                 => write!(f, "exofs: epoch full"),
            Self::CommitInProgress          => write!(f, "exofs: commit in progress"),
            Self::GcQueueFull               => write!(f, "exofs: GC grey queue full"),
            Self::RefCountUnderflow         => write!(f, "exofs: ref_count underflow (BUG)"),
            Self::NotSupported              => write!(f, "exofs: not supported"),
            Self::InvalidArgument           => write!(f, "exofs: invalid argument"),
            Self::InternalError             => write!(f, "exofs: internal error"),
        }
    }
}

/// Conversion ExofsError → FsError (interface VFS).
impl From<ExofsError> for FsError {
    fn from(e: ExofsError) -> Self {
        match e {
            ExofsError::NoMemory                  => FsError::NoMemory,
            ExofsError::NoSpace                   => FsError::NoSpace,
            ExofsError::IoError                   => FsError::IoError,
            ExofsError::PartialWrite              => FsError::IoError,
            ExofsError::OffsetOverflow            => FsError::IoError,
            ExofsError::InvalidMagic              => FsError::Corrupt,
            ExofsError::ChecksumMismatch          => FsError::Corrupt,
            ExofsError::IncompatibleVersion       => FsError::NotSupported,
            ExofsError::CorruptedStructure        => FsError::Corrupt,
            ExofsError::CorruptedChain            => FsError::Corrupt,
            ExofsError::ObjectNotFound            => FsError::NotFound,
            ExofsError::BlobNotFound              => FsError::NotFound,
            ExofsError::ObjectAlreadyExists       => FsError::AlreadyExists,
            ExofsError::WrongObjectKind           => FsError::InvalidArgument,
            ExofsError::WrongObjectClass          => FsError::InvalidArgument,
            ExofsError::InvalidPathComponent      => FsError::InvalidArgument,
            ExofsError::PathTooLong               => FsError::NameTooLong,
            ExofsError::TooManySymlinks           => FsError::Loop,
            ExofsError::DirectoryNotEmpty         => FsError::NotEmpty,
            ExofsError::NotADirectory             => FsError::NotDir,
            ExofsError::PermissionDenied          => FsError::PermissionDenied,
            ExofsError::QuotaExceeded             => FsError::NoSpace,
            ExofsError::SecretBlobIdLeakPrevented => FsError::PermissionDenied,
            ExofsError::NoValidEpoch              => FsError::Corrupt,
            ExofsError::EpochFull                 => FsError::NoSpace,
            ExofsError::CommitInProgress          => FsError::Busy,
            ExofsError::GcQueueFull               => FsError::NoMemory,
            ExofsError::RefCountUnderflow         => FsError::InternalError,
            ExofsError::NotSupported              => FsError::NotSupported,
            ExofsError::InvalidArgument           => FsError::InvalidArgument,
            ExofsError::InternalError             => FsError::InternalError,
        }
    }
}

/// Type résultat standard pour toutes les fonctions ExoFS.
pub type ExofsResult<T> = Result<T, ExofsError>;

// ─────────────────────────────────────────────────────────────────────────────
// Catégories et sévérité
// ─────────────────────────────────────────────────────────────────────────────

/// Catégorie d'erreur pour routage / log / alerting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    /// Erreur de ressource (mémoire, espace disque).
    Resource,
    /// Erreur I/O matérielle ou driver.
    Io,
    /// Corruption on-disk détectée.
    Corruption,
    /// Erreur de logique / paramètre invalide.
    Logic,
    /// Erreur de sécurité / accès.
    Security,
    /// Conflit de concurrence (réessayable).
    Concurrency,
    /// Erreur interne / bug kernel.
    Internal,
}

/// Sévérité d'une erreur pour le log système.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ErrorSeverity {
    /// Informatif — non bloquant.
    Info,
    /// Avertissement — opération dégradée mais continuable.
    Warning,
    /// Erreur — opération échouée, filesystem intact.
    Error,
    /// Fatal — corruption ou bug ; le filesystem doit être démonté.
    Fatal,
}

impl ExofsError {
    /// Catégorie sémantique de cette erreur.
    pub fn category(self) -> ErrorCategory {
        match self {
            Self::NoMemory | Self::NoSpace | Self::EpochFull
            | Self::GcQueueFull => ErrorCategory::Resource,

            Self::IoError | Self::PartialWrite | Self::OffsetOverflow
                => ErrorCategory::Io,

            Self::InvalidMagic | Self::ChecksumMismatch
            | Self::CorruptedStructure | Self::CorruptedChain
            | Self::RefCountUnderflow
                => ErrorCategory::Corruption,

            Self::ObjectNotFound | Self::BlobNotFound
            | Self::ObjectAlreadyExists | Self::WrongObjectKind
            | Self::WrongObjectClass | Self::InvalidPathComponent
            | Self::PathTooLong | Self::TooManySymlinks
            | Self::DirectoryNotEmpty | Self::NotADirectory
            | Self::InvalidArgument | Self::NotSupported
            | Self::IncompatibleVersion
                => ErrorCategory::Logic,

            Self::PermissionDenied | Self::QuotaExceeded
            | Self::SecretBlobIdLeakPrevented
                => ErrorCategory::Security,

            Self::CommitInProgress
                => ErrorCategory::Concurrency,

            Self::NoValidEpoch | Self::InternalError
                => ErrorCategory::Internal,
        }
    }

    /// Sévérité de cette erreur.
    pub fn severity(self) -> ErrorSeverity {
        match self.category() {
            ErrorCategory::Corruption | ErrorCategory::Internal
                => ErrorSeverity::Fatal,
            ErrorCategory::Io
                => ErrorSeverity::Error,
            ErrorCategory::Concurrency | ErrorCategory::Resource
                => ErrorSeverity::Warning,
            ErrorCategory::Logic | ErrorCategory::Security
                => ErrorSeverity::Info,
        }
    }

    /// Vrai si l'erreur est fatale : le filesystem doit être démonté.
    ///
    /// Corruption on-disk, bug kernel, ref_count underflow.
    #[inline]
    pub fn is_fatal(self) -> bool {
        self.severity() == ErrorSeverity::Fatal
    }

    /// Vrai si l'erreur est transitoire et que l'opération peut être réessayée.
    #[inline]
    pub fn is_transient(self) -> bool {
        matches!(self,
            Self::CommitInProgress | Self::GcQueueFull | Self::NoMemory
        )
    }

    /// Vrai si l'erreur suggère un retry (sous-ensemble de is_transient).
    #[inline]
    pub fn suggests_retry(self) -> bool {
        self.is_transient()
    }

    /// Vrai si l'erreur est due à une corruption on-disk.
    #[inline]
    pub fn is_corruption(self) -> bool {
        self.category() == ErrorCategory::Corruption
    }

    /// Vrai si l'erreur est une erreur de sécurité / capabilities.
    #[inline]
    pub fn is_security(self) -> bool {
        self.category() == ErrorCategory::Security
    }

    /// Vrai si l'erreur indique un manque de ressource (espace, mémoire).
    #[inline]
    pub fn is_resource_exhausted(self) -> bool {
        self.category() == ErrorCategory::Resource
    }

    /// Log level recommandé pour cette erreur (kernel log).
    ///
    /// 0=debug, 1=info, 2=warn, 3=error, 4=crit
    pub fn log_level(self) -> u8 {
        match self.severity() {
            ErrorSeverity::Info    => 1,
            ErrorSeverity::Warning => 2,
            ErrorSeverity::Error   => 3,
            ErrorSeverity::Fatal   => 4,
        }
    }

    /// Retourne un code POSIX approximatif (négatif) correspondant.
    pub fn to_posix_errno(self) -> i32 {
        match self {
            Self::NoMemory                  => -12, // ENOMEM
            Self::NoSpace                   => -28, // ENOSPC
            Self::IoError                   => -5,  // EIO
            Self::PartialWrite              => -5,  // EIO
            Self::OffsetOverflow            => -75, // EOVERFLOW
            Self::InvalidMagic              => -5,  // EIO
            Self::ChecksumMismatch          => -5,  // EIO
            Self::IncompatibleVersion       => -95, // EOPNOTSUPP
            Self::CorruptedStructure        => -5,  // EIO
            Self::CorruptedChain            => -5,  // EIO
            Self::ObjectNotFound            => -2,  // ENOENT
            Self::BlobNotFound              => -2,  // ENOENT
            Self::ObjectAlreadyExists       => -17, // EEXIST
            Self::WrongObjectKind           => -22, // EINVAL
            Self::WrongObjectClass          => -22, // EINVAL
            Self::InvalidPathComponent      => -22, // EINVAL
            Self::PathTooLong               => -36, // ENAMETOOLONG
            Self::TooManySymlinks           => -40, // ELOOP
            Self::DirectoryNotEmpty         => -39, // ENOTEMPTY
            Self::NotADirectory             => -20, // ENOTDIR
            Self::PermissionDenied          => -13, // EACCES
            Self::QuotaExceeded             => -122, // EDQUOT
            Self::SecretBlobIdLeakPrevented => -13, // EACCES
            Self::NoValidEpoch              => -5,  // EIO
            Self::EpochFull                 => -28, // ENOSPC
            Self::CommitInProgress          => -11, // EAGAIN
            Self::GcQueueFull               => -12, // ENOMEM
            Self::RefCountUnderflow         => -5,  // EIO (BUG)
            Self::NotSupported              => -95, // EOPNOTSUPP
            Self::InvalidArgument           => -22, // EINVAL
            Self::InternalError             => -5,  // EIO (BUG)
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ExofsError — méthodes utilitaires additionnelles
// ─────────────────────────────────────────────────────────────────────────────

impl ExofsError {
    /// Nom textuel court machine-readable pour le logging et les métriques.
    ///
    /// Couverture exhaustive de tous les variants — doit rester synchronisé
    /// avec l'enum.
    pub fn name(self) -> &'static str {
        match self {
            Self::NoMemory                  => "no_memory",
            Self::NoSpace                   => "no_space",
            Self::IoError                   => "io_error",
            Self::PartialWrite              => "partial_write",
            Self::OffsetOverflow            => "offset_overflow",
            Self::InvalidMagic              => "invalid_magic",
            Self::ChecksumMismatch          => "checksum_mismatch",
            Self::IncompatibleVersion       => "incompatible_version",
            Self::CorruptedStructure        => "corrupted_structure",
            Self::CorruptedChain            => "corrupted_chain",
            Self::ObjectNotFound            => "object_not_found",
            Self::BlobNotFound              => "blob_not_found",
            Self::ObjectAlreadyExists       => "object_already_exists",
            Self::WrongObjectKind           => "wrong_object_kind",
            Self::WrongObjectClass          => "wrong_object_class",
            Self::InvalidPathComponent      => "invalid_path_component",
            Self::PathTooLong               => "path_too_long",
            Self::TooManySymlinks           => "too_many_symlinks",
            Self::DirectoryNotEmpty         => "directory_not_empty",
            Self::NotADirectory             => "not_a_directory",
            Self::PermissionDenied          => "permission_denied",
            Self::QuotaExceeded             => "quota_exceeded",
            Self::SecretBlobIdLeakPrevented => "secret_blob_id_leak",
            Self::NoValidEpoch              => "no_valid_epoch",
            Self::EpochFull                 => "epoch_full",
            Self::CommitInProgress          => "commit_in_progress",
            Self::GcQueueFull               => "gc_queue_full",
            Self::RefCountUnderflow         => "ref_count_underflow",
            Self::NotSupported              => "not_supported",
            Self::InvalidArgument           => "invalid_argument",
            Self::InternalError             => "internal_error",
        }
    }

    /// Vrai si l'opération peut être tentée à nouveau après un court délai.
    ///
    /// Alias sémantique de `is_transient()` orienté vers l'appelant externe.
    #[inline]
    pub fn is_retryable(self) -> bool { self.is_transient() }
}

// ─────────────────────────────────────────────────────────────────────────────
// ExofsErrorContext — erreur augmentée d'informations de contexte
// ─────────────────────────────────────────────────────────────────────────────

/// Erreur ExoFS enrichie d'un contexte minimal (numéro de ligne source + info libre).
///
/// Utilisé pour les rapports d'erreurs détaillés transmis vers l'espace utilisateur
/// via les mécanismes de logging du kernel. La struct est intentionnellement petite
/// pour minimiser la pression sur la pile.
#[derive(Copy, Clone, Debug)]
pub struct ExofsErrorContext {
    /// Erreur de base.
    pub error:     ExofsError,
    /// Ligne source (`line!()` macro) pour localiser rapidement l'appelant.
    pub file_line: u32,
    /// Information libre : par exemple l'ID objet impliqué, ou une valeur de tag.
    pub extra:     u64,
}

impl ExofsErrorContext {
    /// Construit un contexte d'erreur.
    #[inline]
    pub const fn new(error: ExofsError, file_line: u32, extra: u64) -> Self {
        Self { error, file_line, extra }
    }

    /// Construction rapide sans extra.
    #[inline]
    pub const fn simple(error: ExofsError, file_line: u32) -> Self {
        Self { error, file_line, extra: 0 }
    }

    /// Retourne l'erreur de base.
    #[inline]
    pub fn error(&self) -> ExofsError { self.error }

    /// Convertit en errno POSIX.
    #[inline]
    pub fn to_posix_errno(&self) -> i32 { self.error.to_posix_errno() }

    /// Vrai si l'opération peut être retentée.
    #[inline]
    pub fn is_retryable(&self) -> bool { self.error.is_retryable() }

    /// Catégorie sémantique de l'erreur.
    #[inline]
    pub fn category(&self) -> ErrorCategory { self.error.category() }
}

/// Macro de commodité pour créer un `ExofsErrorContext` avec la ligne courante.
#[macro_export]
macro_rules! exofs_err {
    ($e:expr) => {
        $crate::fs::exofs::core::error::ExofsErrorContext::simple($e, line!())
    };
    ($e:expr, $extra:expr) => {
        $crate::fs::exofs::core::error::ExofsErrorContext::new($e, line!(), $extra as u64)
    };
}
