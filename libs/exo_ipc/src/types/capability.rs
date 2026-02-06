// libs/exo_ipc/src/types/capability.rs
//! Système de sécurité capability-based pour IPC
//!
//! Ce module fournit IpcDescriptor qui wrape exo_types::Capability
//! avec des métadonnées temporelles pour la gestion d'expiration.

use core::fmt;

// Utilise le type canonical de exo_types
pub use exo_types::capability::{Capability, Rights};

/// Identifiant de capability unique
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct CapabilityId(pub u64);

impl CapabilityId {
    /// Crée une nouvelle capability ID
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    /// Capability invalide
    pub const INVALID: Self = Self(0);

    /// Capability système (privilèges complets)
    pub const SYSTEM: Self = Self(1);

    /// Vérifie si la capability est valide
    pub fn is_valid(&self) -> bool {
        self.0 != 0
    }
}

impl fmt::Display for CapabilityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Cap({})", self.0)
    }
}

/// Descripteur IPC avec capability et métadonnées temporelles
///
/// Ce type wrape une `Capability` (depuis exo_types) avec des timestamps
/// pour gérer l'expiration et la révocation temporelle.
#[derive(Debug, Clone, Copy)]
pub struct IpcDescriptor {
    /// Capability sous-jacente (type canonical depuis exo_types)
    pub capability: Capability,

    /// Timestamp de création (pour révocation)
    pub created_at: u64,

    /// Timestamp d'expiration (0 = pas d'expiration)
    pub expires_at: u64,
}

impl IpcDescriptor {
    /// Crée un nouveau descripteur IPC
    pub const fn new(capability: Capability) -> Self {
        Self {
            capability,
            created_at: 0,
            expires_at: 0,
        }
    }

    /// Crée depuis une capability existante avec timestamps
    pub const fn with_times(capability: Capability, created_at: u64, expires_at: u64) -> Self {
        Self {
            capability,
            created_at,
            expires_at,
        }
    }

    /// Descripteur système avec permissions complètes
    pub fn system() -> Self {
        Self::new(Capability::system())
    }

    /// Vérifie si le descripteur est valide
    pub fn is_valid(&self) -> bool {
        self.capability.is_valid()
    }

    /// Vérifie si le descripteur a expiré
    pub fn is_expired(&self, current_time: u64) -> bool {
        self.expires_at != 0 && current_time >= self.expires_at
    }

    /// Vérifie si le descripteur autorise une opération
    pub fn allows(&self, required: Rights, current_time: u64) -> bool {
        self.is_valid()
            && !self.is_expired(current_time)
            && self.capability.has_rights(required)
    }

    /// Définit l'expiration
    pub fn with_expiration(mut self, expires_at: u64) -> Self {
        self.expires_at = expires_at;
        self
    }

    /// Définit le timestamp de création
    pub fn with_creation_time(mut self, created_at: u64) -> Self {
        self.created_at = created_at;
        self
    }

    /// Récupère l'ID de la capability
    pub fn id(&self) -> u64 {
        self.capability.id()
    }

    /// Récupère les droits
    pub fn rights(&self) -> Rights {
        self.capability.rights()
    }
}

impl fmt::Display for IpcDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "IpcDescriptor {{ cap: {:?}, created: {}, expires: {} }}",
            self.capability, self.created_at, self.expires_at
        )
    }
}

impl From<Capability> for IpcDescriptor {
    fn from(capability: Capability) -> Self {
        Self::new(capability)
    }
}

// Alias de compatibilité (deprecated)
#[deprecated(since = "0.2.0", note = "Use exo_types::Rights directly")]
pub type Permissions = Rights;
