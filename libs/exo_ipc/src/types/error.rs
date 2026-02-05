// libs/exo_ipc/src/types/error.rs
//! Gestion d'erreurs exhaustive pour exo_ipc

use core::fmt;

/// Résultat IPC standard
pub type IpcResult<T> = Result<T, IpcError>;

/// Erreurs IPC complètes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcError {
    /// Canal déconnecté
    Disconnected,
    
    /// Canal plein (pour try_send)
    WouldBlock,
    
    /// Timeout expiré
    Timeout,
    
    /// Message trop grand
    MessageTooLarge {
        size: usize,
        max_size: usize,
    },
    
    /// Message invalide (checksum, format, etc.)
    InvalidMessage,
    
    /// Checksum incorrect
    ChecksumMismatch {
        expected: u32,
        actual: u32,
    },
    
    /// Version de protocole incompatible
    IncompatibleVersion {
        local: u16,
        remote: u16,
    },
    
    /// Capacité invalide (doit être puissance de 2)
    InvalidCapacity(usize),
    
    /// Allocation mémoire échouée
    OutOfMemory,
    
    /// Endpoint introuvable
    EndpointNotFound(u64),
    
    /// Permission refusée (sécurité capability)
    PermissionDenied,
    
    /// Opération non supportée
    Unsupported,
    
    /// Erreur de sérialisation
    SerializationError,
    
    /// Erreur de désérialisation
    DeserializationError,
    
    /// État invalide
    InvalidState,
    
    /// Ressource déjà utilisée
    AlreadyInUse,
    
    /// Paramètre invalide
    InvalidParameter,
}

impl fmt::Display for IpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disconnected => write!(f, "Canal IPC déconnecté"),
            Self::WouldBlock => write!(f, "Opération bloquerait (canal plein/vide)"),
            Self::Timeout => write!(f, "Timeout expiré"),
            Self::MessageTooLarge { size, max_size } => {
                write!(f, "Message trop grand: {} bytes (max: {})", size, max_size)
            }
            Self::InvalidMessage => write!(f, "Message invalide"),
            Self::ChecksumMismatch { expected, actual } => {
                write!(f, "Checksum incorrect: attendu {:#x}, reçu {:#x}", expected, actual)
            }
            Self::IncompatibleVersion { local, remote } => {
                write!(f, "Version incompatible: locale v{}, distante v{}", local, remote)
            }
            Self::InvalidCapacity(cap) => {
                write!(f, "Capacité invalide: {} (doit être puissance de 2)", cap)
            }
            Self::OutOfMemory => write!(f, "Mémoire insuffisante"),
            Self::EndpointNotFound(id) => write!(f, "Endpoint {} introuvable", id),
            Self::PermissionDenied => write!(f, "Permission refusée"),
            Self::Unsupported => write!(f, "Opération non supportée"),
            Self::SerializationError => write!(f, "Erreur de sérialisation"),
            Self::DeserializationError => write!(f, "Erreur de désérialisation"),
            Self::InvalidState => write!(f, "État invalide"),
            Self::AlreadyInUse => write!(f, "Ressource déjà utilisée"),
            Self::InvalidParameter => write!(f, "Paramètre invalide"),
        }
    }
}

/// Erreurs d'envoi
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendError<T> {
    /// Canal déconnecté
    Disconnected(T),
    
    /// Canal plein
    Full(T),
    
    /// Timeout
    Timeout(T),
}

/// Erreurs de réception
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecvError {
    /// Canal déconnecté
    Disconnected,
    
    /// Canal vide
    Empty,
    
    /// Timeout
    Timeout,
}

impl<T> fmt::Display for SendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disconnected(_) => write!(f, "Canal déconnecté"),
            Self::Full(_) => write!(f, "Canal plein"),
            Self::Timeout(_) => write!(f, "Timeout expiré"),
        }
    }
}

impl fmt::Display for RecvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disconnected => write!(f, "Canal déconnecté"),
            Self::Empty => write!(f, "Canal vide"),
            Self::Timeout => write!(f, "Timeout expiré"),
        }
    }
}
