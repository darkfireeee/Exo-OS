// libs/exo_types/src/error.rs
use core::fmt;

/// Type de résultat standard pour Exo-OS
pub type Result<T> = core::result::Result<T, ExoError>;

/// Codes d'erreur systèmes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// Opération réussie
    Ok = 0,

    /// Ressource non trouvée
    NotFound = 1,

    /// Permission refusée
    PermissionDenied = 2,

    /// Mémoire insuffisante
    OutOfMemory = 3,

    /// Argument invalide
    InvalidArgument = 4,

    /// Ressource occupée
    ResourceBusy = 5,

    /// Opération non supportée
    NotSupported = 6,

    /// Dépassement de capacité
    Overflow = 7,

    /// Timeout dépassé
    Timeout = 8,

    /// État invalide
    InvalidState = 9,

    /// Erreur d'E/S
    IoError = 10,

    /// Erreur de communication inter-processus
    IpcError = 11,

    /// Erreur cryptographique
    CryptoError = 12,
}

/// Structure d'erreur pour Exo-OS
#[derive(Debug, Clone)]
pub struct ExoError {
    /// Code d'erreur
    code: ErrorCode,

    /// Message descriptif
    message: Option<&'static str>,

    /// Identifiant optionnel pour le contexte
    context_id: Option<u64>,
}

impl ExoError {
    /// Crée une nouvelle erreur avec un code
    pub fn new(code: ErrorCode) -> Self {
        ExoError {
            code,
            message: None,
            context_id: None,
        }
    }

    /// Crée une erreur avec un message
    pub fn with_message(code: ErrorCode, message: &'static str) -> Self {
        ExoError {
            code,
            message: Some(message),
            context_id: None,
        }
    }

    /// Définit un identifiant de contexte
    pub fn with_context(mut self, id: u64) -> Self {
        self.context_id = Some(id);
        self
    }

    /// Retourne le code d'erreur
    pub fn code(&self) -> ErrorCode {
        self.code
    }

    /// Retourne le message d'erreur
    pub fn message(&self) -> Option<&'static str> {
        self.message
    }
}

impl fmt::Display for ExoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.code {
            ErrorCode::Ok => write!(f, "Success"),
            ErrorCode::NotFound => write!(f, "Resource not found"),
            ErrorCode::PermissionDenied => write!(f, "Permission denied"),
            ErrorCode::OutOfMemory => write!(f, "Out of memory"),
            ErrorCode::InvalidArgument => write!(f, "Invalid argument"),
            ErrorCode::ResourceBusy => write!(f, "Resource busy"),
            ErrorCode::NotSupported => write!(f, "Operation not supported"),
            ErrorCode::Overflow => write!(f, "Overflow occurred"),
            ErrorCode::Timeout => write!(f, "Operation timed out"),
            ErrorCode::InvalidState => write!(f, "Invalid state"),
            ErrorCode::IoError => write!(f, "I/O error"),
            ErrorCode::IpcError => write!(f, "IPC error"),
            ErrorCode::CryptoError => write!(f, "Cryptographic error"),
        }?;

        if let Some(msg) = self.message {
            write!(f, ": {}", msg)?;
        }

        if let Some(id) = self.context_id {
            write!(f, " (context: {})", id)?;
        }

        Ok(())
    }
}

impl From<ErrorCode> for ExoError {
    fn from(code: ErrorCode) -> Self {
        ExoError::new(code)
    }
}

