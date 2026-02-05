// libs/exo_std/src/security.rs
//! Primitives de sécurité basées sur capabilities
//!
//! Exo-OS utilise un système de sécurité basé sur capabilities pour
//! contrôler l'accès aux ressources système.

use crate::Result;
use crate::error::{SecurityError, ExoStdError};

/// ID de capability
pub type CapabilityId = u64;

/// Type de capability
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityType {
    /// Lecture de fichier
    FileRead = 1,
    /// Écriture de fichier
    FileWrite = 2,
    /// Exécution de programme
    Execute = 3,
    /// Opérations réseau
    Network = 4,
    /// Gestion de processus
    ProcessManagement = 5,
    /// Gestion de mémoire
    MemoryManagement = 6,
    /// Opérations système privilégiées
    SystemAdmin = 7,
}

/// Representation d'une capability
#[derive(Debug, Clone, Copy)]
pub struct Capability {
    /// ID unique de la capability
    pub id: CapabilityId,
    /// Type de capability
    pub cap_type: CapabilityType,
    /// Droits associés
    pub rights: Rights,
}

/// Droits d'accès
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rights(u32);

impl Rights {
    /// Aucun droit
    pub const NONE: Self = Self(0);
    /// Droit de lecture
    pub const READ: Self = Self(1 << 0);
    /// Droit d'écriture
    pub const WRITE: Self = Self(1 << 1);
    /// Droit d'exécution
    pub const EXECUTE: Self = Self(1 << 2);
    /// Tous les droits
    pub const ALL: Self = Self(0xFFFFFFFF);
    
    /// Crée des Rights depuis un mask
    #[inline]
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }
    
    /// Retourne le mask de bits
    #[inline]
    pub const fn bits(&self) -> u32 {
        self.0
    }
    
    /// Vérifie si contient un droit spécifique
    #[inline]
    pub const fn contains(&self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
    
    /// Combine deux ensembles de droits
    #[inline]
    pub const fn union(&self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
    
    /// Intersection de deux ensembles de droits
    #[inline]
    pub const fn intersection(&self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
}

impl core::ops::BitOr for Rights {
    type Output = Self;
    
    #[inline]
    fn bitor(self, rhs: Self) -> Self {
        self.union(rhs)
    }
}

impl core::ops::BitAnd for Rights {
    type Output = Self;
    
    #[inline]
    fn bitand(self, rhs: Self) -> Self {
        self.intersection(rhs)
    }
}

/// Vérifie qu'une capability est valide et accessible
///
/// # Exemple
/// ```no_run
/// use exo_std::security;
///
/// let cap_id = 12345;
/// if security::verify_capability(cap_id).is_ok() {
///     // Capability valide, procéder
/// }
/// ```
pub fn verify_capability(cap_id: CapabilityId) -> Result<()> {
    #[cfg(feature = "test_mode")]
    {
        // En mode test, accepte toutes les capabilities
        let _ = cap_id;
        Ok(())
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        // TODO: Appel système pour vérifier la capability
        // Pour l'instant, stub
        let _ = cap_id;
        Ok(())
    }
}

/// Vérifie que le processus possède les droits donnés
pub fn check_rights(cap_id: CapabilityId, required: Rights) -> Result<()> {
    verify_capability(cap_id)?;
    
    // TODO: Vérifier que la capability a les droits requis
    let _ = required;
    Ok(())
}

/// Demande une nouvelle capability au système
///
/// # Safety
/// Le processus doit avoir les permissions pour demander ce type de capability
pub unsafe fn request_capability(cap_type: CapabilityType, rights: Rights) -> Result<CapabilityId> {
    #[cfg(feature = "test_mode")]
    {
        let _ = (cap_type, rights);
        Ok(1) // Retourne un ID simulé
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        // TODO: Syscall pour demander capability
        let _ = (cap_type, rights);
        Err(ExoStdError::Security(SecurityError::PermissionDenied))
    }
}

/// Révoque une capability
///
/// # Safety
/// Le processus doit posséder cette capability
pub unsafe fn revoke_capability(cap_id: CapabilityId) -> Result<()> {
    #[cfg(feature = "test_mode")]
    {
        let _ = cap_id;
        Ok(())
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        // TODO: Syscall pour révoquer
        let _ = cap_id;
        Ok(())
    }
}

/// Délégue une capability à un autre processus
///
/// # Safety  
/// - Le processus doit posséder la capability
/// - Le processus cible doit exister et être accessible
pub unsafe fn delegate_capability(
    cap_id: CapabilityId,
    target_pid: crate::process::Pid,
) -> Result<()> {
    #[cfg(feature = "test_mode")]
    {
        let _ = (cap_id, target_pid);
        Ok(())
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        // TODO: Syscall pour déléguer
        let _ = (cap_id, target_pid);
        Err(ExoStdError::Security(SecurityError::PermissionDenied))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_rights() {
        let read_write = Rights::READ | Rights::WRITE;
        
        assert!(read_write.contains(Rights::READ));
        assert!(read_write.contains(Rights::WRITE));
        assert!(!read_write.contains(Rights::EXECUTE));
        
        let read_only = read_write & Rights::READ;
        assert!(read_only.contains(Rights::READ));
        assert!(!read_only.contains(Rights::WRITE));
    }
    
    #[test]
    fn test_verify_capability() {
        // En mode test, devrait toujours passer
        assert!(verify_capability(123).is_ok());
    }
}
