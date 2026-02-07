// libs/exo_std/src/error.rs
//! Gestion d'erreurs unifiée pour exo_std
//!
//! Ce module fournit une hiérarchie d'erreurs complète et type-safe pour
//! toutes les opérations de la bibliothèque standard.

use core::fmt;

/// Type Result avec erreur ExoStdError
pub type Result<T> = core::result::Result<T, ExoStdError>;

/// Type IoError comme alias vers IoErrorKind pour compatibilité
pub type IoError = IoErrorKind;

/// Erreur principale de exo_std
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExoStdError {
    /// Erreur d'I/O
    Io(IoErrorKind),
    /// Erreur de processus
    Process(ProcessError),
    /// Erreur de thread
    Thread(ThreadError),
    /// Erreur de synchronisation
    Sync(SyncError),
    /// Erreur de collection
    Collection(CollectionError),
    /// Erreur de sécurité
    Security(SecurityError),
    /// Erreur IPC
    Ipc(IpcError),
    /// Erreur système
    System(SystemError),
    /// Autre erreur
    Other,
}

/// Type d'erreur I/O
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoErrorKind {
    /// Opération interrompue
    Interrupted,
    /// Fin de fichier inattendue
    UnexpectedEof,
    /// Ressource temporairement indisponible
    WouldBlock,
    /// Permission refusée
    PermissionDenied,
    /// Entité non trouvée
    NotFound,
    /// Entité existe déjà
    AlreadyExists,
    /// Argument invalide
    InvalidInput,
    /// Données invalides
    InvalidData,
    /// Timeout expiré
    TimedOut,
    /// Écriture vers pipe fermé
    BrokenPipe,
    /// Plus d'espace disponible
    StorageFull,
    /// Opération non supportée
    Unsupported,
    /// Tentative d'écriture de 0 octets
    WriteZero,
    /// Autre erreur I/O
    Other,
}

/// Erreur de processus
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessError {
    /// Processus non trouvé
    NotFound,
    /// Permission refusée
    PermissionDenied,
    /// Ressources système épuisées
    ResourceExhausted,
    /// Code de sortie invalide
    InvalidExitStatus,
    /// Échec de fork
    ForkFailed,
    /// Échec de exec
    ExecFailed,
    /// Échec de wait
    WaitFailed,
    /// Autre erreur processus
    Other,
}

/// Erreur de thread
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadError {
    /// Échec de création
    CreationFailed,
    /// Échec de join
    JoinFailed,
    /// Thread en panic
    Panicked,
    /// Ressources épuisées
    ResourceExhausted,
    /// Nom invalide
    InvalidName,
    /// TLS non initialisé
    TlsNotInitialized,
    /// Template TLS invalide
    TlsInvalid,
    /// Échec d'allocation TLS
    TlsAllocationFailed,
    /// Échec de configuration TLS
    TlsSetupFailed,
    /// Autre erreur thread
    Other,
}

/// Erreur de synchronisation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncError {
    /// Mutex empoisonné
    Poisoned,
    /// Timeout expiré
    Timeout,
    /// Opération serait bloquante
    WouldBlock,
    /// Deadlock détecté
    Deadlock,
    /// Échec d'attente (futex wait failed)
    WaitFailed,
    /// Échec de verrouillage
    LockFailed,
    /// Échec de déverrouillage
    UnlockFailed,
    /// Autre erreur sync
    Other,
}

/// Erreur de collection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionError {
    /// Capacité dépassée
    CapacityExceeded,
    /// Index hors limites
    IndexOutOfBounds,
    /// Collection vide
    Empty,
    /// Collection pleine
    Full,
    /// Autre erreur collection
    Other,
}

/// Erreur de sécurité
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityError {
    /// Capability invalide
    InvalidCapability,
    /// Permission refusée
    PermissionDenied,
    /// Opération non autorisée
    Forbidden,
    /// Autre erreur sécurité
    Other,
}

/// Erreur IPC
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcError {
    /// Channel fermé
    ChannelClosed,
    /// Message trop grand
    MessageTooLarge,
    /// Destination invalide
    InvalidDestination,
    /// Timeout expiré
    Timeout,
    /// Autre erreur IPC
    Other,
}

/// Erreur système
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemError {
    /// Appel système invalide
    InvalidSyscall,
    /// Argument invalide
    InvalidArgument,
    /// Fonctionnalité non implémentée
    NotImplemented,
    /// Ressources système insuffisantes
    ResourceExhausted,
    /// Autre erreur système
    Other,
}

// Implémentations Display pour messages d'erreur lisibles

impl fmt::Display for ExoStdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExoStdError::Io(e) => write!(f, "I/O error: {}", e),
            ExoStdError::Process(e) => write!(f, "Process error: {}", e),
            ExoStdError::Thread(e) => write!(f, "Thread error: {}", e),
            ExoStdError::Sync(e) => write!(f, "Sync error: {}", e),
            ExoStdError::Collection(e) => write!(f, "Collection error: {}", e),
            ExoStdError::Security(e) => write!(f, "Security error: {}", e),
            ExoStdError::Ipc(e) => write!(f, "IPC error: {}", e),
            ExoStdError::System(e) => write!(f, "System error: {}", e),
            ExoStdError::Other => write!(f, "Unknown error"),
        }
    }
}

impl fmt::Display for IoErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IoErrorKind::Interrupted => write!(f, "operation interrupted"),
            IoErrorKind::UnexpectedEof => write!(f, "unexpected end of file"),
            IoErrorKind::WouldBlock => write!(f, "operation would block"),
            IoErrorKind::PermissionDenied => write!(f, "permission denied"),
            IoErrorKind::NotFound => write!(f, "entity not found"),
            IoErrorKind::AlreadyExists => write!(f, "entity already exists"),
            IoErrorKind::InvalidInput => write!(f, "invalid input"),
            IoErrorKind::InvalidData => write!(f, "invalid data"),
            IoErrorKind::TimedOut => write!(f, "operation timed out"),
            IoErrorKind::BrokenPipe => write!(f, "broken pipe"),
            IoErrorKind::StorageFull => write!(f, "no storage space"),
            IoErrorKind::Unsupported => write!(f, "operation not supported"),
            IoErrorKind::WriteZero => write!(f, "write zero"),
            IoErrorKind::Other => write!(f, "other I/O error"),
        }
    }
}

impl fmt::Display for ProcessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProcessError::NotFound => write!(f, "process not found"),
            ProcessError::PermissionDenied => write!(f, "permission denied"),
            ProcessError::ResourceExhausted => write!(f, "system resources exhausted"),
            ProcessError::InvalidExitStatus => write!(f, "invalid exit status"),
            ProcessError::ForkFailed => write!(f, "fork failed"),
            ProcessError::ExecFailed => write!(f, "exec failed"),
            ProcessError::WaitFailed => write!(f, "wait failed"),
            ProcessError::Other => write!(f, "other process error"),
        }
    }
}

impl fmt::Display for ThreadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ThreadError::CreationFailed => write!(f, "thread creation failed"),
            ThreadError::JoinFailed => write!(f, "thread join failed"),
            ThreadError::Panicked => write!(f, "thread panicked"),
            ThreadError::ResourceExhausted => write!(f, "resource exhausted"),
            ThreadError::InvalidName => write!(f, "invalid thread name"),
            ThreadError::TlsNotInitialized => write!(f, "TLS not initialized"),
            ThreadError::TlsInvalid => write!(f, "invalid TLS template"),
            ThreadError::TlsAllocationFailed => write!(f, "TLS allocation failed"),
            ThreadError::TlsSetupFailed => write!(f, "TLS setup failed"),
            ThreadError::Other => write!(f, "other thread error"),
        }
    }
}

impl fmt::Display for SyncError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SyncError::Poisoned => write!(f, "lock poisoned"),
            SyncError::Timeout => write!(f, "operation timed out"),
            SyncError::WouldBlock => write!(f, "operation would block"),
            SyncError::Deadlock => write!(f, "deadlock detected"),
            SyncError::WaitFailed => write!(f, "wait operation failed"),
            SyncError::LockFailed => write!(f, "lock operation failed"),
            SyncError::UnlockFailed => write!(f, "unlock operation failed"),
            SyncError::Other => write!(f, "other sync error"),
        }
    }
}

impl fmt::Display for CollectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CollectionError::CapacityExceeded => write!(f, "capacity exceeded"),
            CollectionError::IndexOutOfBounds => write!(f, "index out of bounds"),
            CollectionError::Empty => write!(f, "collection is empty"),
            CollectionError::Full => write!(f, "collection is full"),
            CollectionError::Other => write!(f, "other collection error"),
        }
    }
}

impl fmt::Display for SecurityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SecurityError::InvalidCapability => write!(f, "invalid capability"),
            SecurityError::PermissionDenied => write!(f, "permission denied"),
            SecurityError::Forbidden => write!(f, "operation forbidden"),
            SecurityError::Other => write!(f, "other security error"),
        }
    }
}

impl fmt::Display for IpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IpcError::ChannelClosed => write!(f, "channel closed"),
            IpcError::MessageTooLarge => write!(f, "message too large"),
            IpcError::InvalidDestination => write!(f, "invalid destination"),
            IpcError::Timeout => write!(f, "operation timed out"),
            IpcError::Other => write!(f, "other IPC error"),
        }
    }
}

impl fmt::Display for SystemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SystemError::InvalidSyscall => write!(f, "invalid syscall"),
            SystemError::InvalidArgument => write!(f, "invalid argument"),
            SystemError::NotImplemented => write!(f, "not implemented"),
            SystemError::ResourceExhausted => write!(f, "resource exhausted"),
            SystemError::Other => write!(f, "other system error"),
        }
    }
}

// Conversions pratiques

impl From<IoErrorKind> for ExoStdError {
    #[inline]
    fn from(e: IoErrorKind) -> Self {
        ExoStdError::Io(e)
    }
}

impl From<ProcessError> for ExoStdError {
    #[inline]
    fn from(e: ProcessError) -> Self {
        ExoStdError::Process(e)
    }
}

impl From<ThreadError> for ExoStdError {
    #[inline]
    fn from(e: ThreadError) -> Self {
        ExoStdError::Thread(e)
    }
}

impl From<SyncError> for ExoStdError {
    #[inline]
    fn from(e: SyncError) -> Self {
        ExoStdError::Sync(e)
    }
}

impl From<CollectionError> for ExoStdError {
    #[inline]
    fn from(e: CollectionError) -> Self {
        ExoStdError::Collection(e)
    }
}

impl From<SecurityError> for ExoStdError {
    #[inline]
    fn from(e: SecurityError) -> Self {
        ExoStdError::Security(e)
    }
}

impl From<IpcError> for ExoStdError {
    #[inline]
    fn from(e: IpcError) -> Self {
        ExoStdError::Ipc(e)
    }
}

impl From<SystemError> for ExoStdError {
    #[inline]
    fn from(e: SystemError) -> Self {
        ExoStdError::System(e)
    }
}

// Reverse conversions for extracting specific errors from ExoStdError
impl From<ExoStdError> for ProcessError {
    #[inline]
    fn from(e: ExoStdError) -> Self {
        match e {
            ExoStdError::Process(p) => p,
            _ => ProcessError::Other,
        }
    }
}

impl From<ExoStdError> for ThreadError {
    #[inline]
    fn from(e: ExoStdError) -> Self {
        match e {
            ExoStdError::Thread(t) => t,
            _ => ThreadError::Other,
        }
    }
}

impl From<ExoStdError> for IoErrorKind {
    #[inline]
    fn from(e: ExoStdError) -> Self {
        match e {
            ExoStdError::Io(io) => io,
            _ => IoErrorKind::Other,
        }
    }
}

/// Extension pour Result avec méthodes utilitaires
pub trait ResultExt<T> {
    /// Convertit en Result I/O
    fn io_context(self, kind: IoErrorKind) -> Result<T>;
}

impl<T, E> ResultExt<T> for core::result::Result<T, E> {
    #[inline]
    fn io_context(self, kind: IoErrorKind) -> Result<T> {
        self.map_err(|_| ExoStdError::Io(kind))
    }
}
