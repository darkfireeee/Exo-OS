//! Gestion des capabilities pour le contrôle d'accès fin

use crate::error::SecurityError;

/// Type de capability
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum CapabilityType {
    /// Lecture de fichiers
    FileRead = 0,
    /// Écriture de fichiers
    FileWrite = 1,
    /// Accès réseau
    NetworkAccess = 2,
    /// Création de processus
    ProcessCreate = 3,
    /// Allocation mémoire
    MemoryAllocate = 4,
    /// Accès aux périphériques
    DeviceAccess = 5,
    /// Administration système
    SystemAdmin = 6,
}

/// Capability représentant un droit d'accès
#[derive(Debug, Clone, Copy)]
pub struct Capability {
    /// ID unique de la capability
    pub id: u64,
    /// Type de capability
    pub cap_type: CapabilityType,
    /// Droits associés
    pub rights: Rights,
}

/// Droits d'accès
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rights {
    bits: u32,
}

impl Rights {
    /// Droit de lecture
    pub const READ: Self = Self { bits: 1 << 0 };
    /// Droit d'écriture
    pub const WRITE: Self = Self { bits: 1 << 1 };
    /// Droit d'exécution
    pub const EXECUTE: Self = Self { bits: 1 << 2 };
    /// Tous les droits
    pub const ALL: Self = Self { 
        bits: Self::READ.bits | Self::WRITE.bits | Self::EXECUTE.bits 
    };
    /// Aucun droit
    pub const NONE: Self = Self { bits: 0 };

    /// Crée des droits depuis des bits bruts
    pub const fn from_bits(bits: u32) -> Option<Self> {
        if bits & !Self::ALL.bits == 0 {
            Some(Self { bits })
        } else {
            None
        }
    }

    /// Retourne les bits bruts
    pub const fn bits(&self) -> u32 {
        self.bits
    }

    /// Vérifie si contient tous les droits spécifiés
    pub const fn contains(&self, other: Self) -> bool {
        (self.bits & other.bits) == other.bits
    }

    /// Intersection de droits
    pub const fn intersection(&self, other: Self) -> Self {
        Self {
            bits: self.bits & other.bits,
        }
    }

    /// Union de droits
    pub const fn union(&self, other: Self) -> Self {
        Self {
            bits: self.bits | other.bits,
        }
    }

    /// Différence de droits
    pub const fn difference(&self, other: Self) -> Self {
        Self {
            bits: self.bits & !other.bits,
        }
    }

    /// Vérifie si vide
    pub const fn is_empty(&self) -> bool {
        self.bits == 0
    }

    /// Vérifie si tous les droits
    pub const fn is_all(&self) -> bool {
        self.bits == Self::ALL.bits
    }
}

impl core::ops::BitOr for Rights {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self {
            bits: self.bits | rhs.bits,
        }
    }
}

impl core::ops::BitAnd for Rights {
    type Output = Self;

    fn bitand(self, rhs: Self) -> Self::Output {
        Self {
            bits: self.bits & rhs.bits,
        }
    }
}

impl core::ops::Not for Rights {
    type Output = Self;

    fn not(self) -> Self::Output {
        Self {
            bits: !self.bits & Self::ALL.bits,
        }
    }
}

/// Vérifie qu'une capability est valide
pub fn verify_capability(cap_id: u64) -> Result<(), SecurityError> {
    #[cfg(feature = "test_mode")]
    {
        let _ = cap_id;
        Ok(())
    }
    
    #[cfg(not(feature = "test_mode"))]
    unsafe {
        use crate::syscall::{syscall1, SyscallId};
        
        let result = syscall1(SyscallId::CapabilityVerify, cap_id as usize);
        
        if result == 0 {
            Ok(())
        } else {
            Err(SecurityError::InvalidCapability)
        }
    }
}

/// Vérifie que la capability a les droits requis
pub fn check_rights(cap: &Capability, required: Rights) -> Result<(), SecurityError> {
    if cap.rights.contains(required) {
        Ok(())
    } else {
        Err(SecurityError::InsufficientRights)
    }
}

/// Demande une nouvelle capability
pub unsafe fn request_capability(
    cap_type: CapabilityType,
    rights: Rights,
) -> Result<u64, SecurityError> {
    #[cfg(feature = "test_mode")]
    {
        let _ = (cap_type, rights);
        Ok(1234)
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        use crate::syscall::{syscall2, SyscallId};
        
        let result = syscall2(
            SyscallId::CapabilityRequest,
            cap_type as usize,
            rights.bits() as usize,
        );
        
        if result < 0 {
            Err(SecurityError::RequestDenied)
        } else {
            Ok(result as u64)
        }
    }
}

/// Révoque une capability
pub fn revoke_capability(cap_id: u64) -> Result<(), SecurityError> {
    #[cfg(feature = "test_mode")]
    {
        let _ = cap_id;
        Ok(())
    }
    
    #[cfg(not(feature = "test_mode"))]
    unsafe {
        use crate::syscall::{syscall1, SyscallId};
        
        let result = syscall1(SyscallId::CapabilityRevoke, cap_id as usize);
        
        if result == 0 {
            Ok(())
        } else {
            Err(SecurityError::RevokeFailed)
        }
    }
}

/// Délègue une capability à un autre processus
pub unsafe fn delegate_capability(
    cap_id: u64,
    target_pid: u32,
) -> Result<(), SecurityError> {
    #[cfg(feature = "test_mode")]
    {
        let _ = (cap_id, target_pid);
        Ok(())
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        use crate::syscall::{syscall2, SyscallId};
        
        let result = syscall2(
            SyscallId::CapabilityDelegate,
            cap_id as usize,
            target_pid as usize,
        );
        
        if result == 0 {
            Ok(())
        } else {
            Err(SecurityError::DelegateFailed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rights_operations() {
        let rw = Rights::READ | Rights::WRITE;
        
        assert!(rw.contains(Rights::READ));
        assert!(rw.contains(Rights::WRITE));
        assert!(!rw.contains(Rights::EXECUTE));

        let ro = rw & Rights::READ;
        assert_eq!(ro.bits(), Rights::READ.bits());

        assert!(!Rights::NONE.is_all());
        assert!(Rights::ALL.is_all());
    }

    #[test]
    fn test_capability_verification() {
        verify_capability(123).unwrap();
        
        let cap = Capability {
            id: 1,
            cap_type: CapabilityType::FileRead,
            rights: Rights::READ,
        };

        check_rights(&cap, Rights::READ).unwrap();
        assert!(check_rights(&cap, Rights::WRITE).is_err());
    }

    #[test]
    fn test_request_revoke() {
        unsafe {
            let cap_id = request_capability(
                CapabilityType::FileRead,
                Rights::READ,
            ).unwrap();

            revoke_capability(cap_id).unwrap();
        }
    }
}
