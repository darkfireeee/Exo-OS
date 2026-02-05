// libs/exo_ipc/src/types/capability.rs
//! Système de sécurité capability-based pour IPC

use core::fmt;

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

/// Permissions pour les opérations IPC
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permissions(u32);

impl Permissions {
    /// Aucune permission
    pub const NONE: Self = Self(0);
    
    /// Permission de lecture
    pub const READ: Self = Self(1 << 0);
    
    /// Permission d'écriture
    pub const WRITE: Self = Self(1 << 1);
    
    /// Permission d'exécution (pour RPC)
    pub const EXECUTE: Self = Self(1 << 2);
    
    /// Permission de création de canaux
    pub const CREATE: Self = Self(1 << 3);
    
    /// Permission de destruction
    pub const DESTROY: Self = Self(1 << 4);
    
    /// Permission de délégation (transférer des capabilities)
    pub const DELEGATE: Self = Self(1 << 5);
    
    /// Toutes les permissions
    pub const ALL: Self = Self(0xFFFFFFFF);
    
    /// Crée un ensemble de permissions
    pub const fn new() -> Self {
        Self::NONE
    }
    
    /// Ajoute une permission
    pub const fn with(mut self, perm: Self) -> Self {
        self.0 |= perm.0;
        self
    }
    
    /// Vérifie si une permission est présente
    pub const fn has(&self, perm: Self) -> bool {
        (self.0 & perm.0) == perm.0
    }
    
    /// Vérifie si l'ensemble est vide
    pub const fn is_empty(&self) -> bool {
        self.0 == 0
    }
    
    /// Intersection de permissions
    pub const fn intersect(&self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
    
    /// Union de permissions
    pub const fn union(&self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl Default for Permissions {
    fn default() -> Self {
        Self::NONE
    }
}

/// Capability complète avec permissions
#[derive(Debug, Clone, Copy)]
pub struct Capability {
    /// Identifiant unique
    pub id: CapabilityId,
    
    /// Permissions associées
    pub permissions: Permissions,
    
    /// Timestamp de création (pour révocation)
    pub created_at: u64,
    
    /// Timestamp d'expiration (0 = pas d'expiration)
    pub expires_at: u64,
}

impl Capability {
    /// Crée une nouvelle capability
    pub const fn new(id: CapabilityId, permissions: Permissions) -> Self {
        Self {
            id,
            permissions,
            created_at: 0,
            expires_at: 0,
        }
    }
    
    /// Capability système avec permissions complètes
    pub const fn system() -> Self {
        Self::new(CapabilityId::SYSTEM, Permissions::ALL)
    }
    
    /// Vérifie si la capability est valide
    pub fn is_valid(&self) -> bool {
        self.id.is_valid() && !self.permissions.is_empty()
    }
    
    /// Vérifie si la capability a expiré
    pub fn is_expired(&self, current_time: u64) -> bool {
        self.expires_at != 0 && current_time >= self.expires_at
    }
    
    /// Vérifie si la capability autorise une opération
    pub fn allows(&self, required: Permissions, current_time: u64) -> bool {
        self.is_valid()
            && !self.is_expired(current_time)
            && self.permissions.has(required)
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
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Capability {{ id: {}, perms: {:#x}, created: {}, expires: {} }}",
            self.id, self.permissions.0, self.created_at, self.expires_at
        )
    }
}
