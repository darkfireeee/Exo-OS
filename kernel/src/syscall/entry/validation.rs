//! # Validation des Arguments des Syscalls
//!
//! Ce module fournit des utilitaires pour valider les pointeurs et les tailles
//! provenant de l'espace utilisateur. C'est une barrière de sécurité essentielle
//! pour empêcher le noyau de lire ou d'écrire dans des zones mémoire non autorisées.

use crate::memory::address::{UserVirtAddr, VirtAddr};
use crate::memory::vm::VmArea;
use crate::task::current;

/// Erreur de validation d'un argument utilisateur.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationError {
    /// L'adresse est nulle.
    NullPointer,
    /// L'adresse n'est pas alignée.
    Misaligned,
    /// La plage mémoire [addr, addr+size) n'est pas entièrement dans l'espace utilisateur.
    InvalidRange,
    /// Les permissions de la plage mémoire ne correspondent pas (ex: écriture dans une zone en lecture seule).
    PermissionDenied,
}

/// Type de permission requise pour une validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    Read,
    Write,
    ReadWrite,
    Execute,
}

/// Valide un pointeur brut depuis l'espace utilisateur.
///
/// # Arguments
///
/// * `ptr` - Le pointeur à valider.
/// * `size` - La taille de la zone mémoire pointée.
/// * `perm` - Les permissions requises pour cette zone.
///
/// # Retourne
///
/// `Ok(UserVirtAddr)` si le pointeur est valide, `Err(ValidationError)` sinon.
pub fn validate_user_ptr(ptr: usize, size: usize, perm: Permission) -> Result<UserVirtAddr, ValidationError> {
    if ptr == 0 && size > 0 {
        return Err(ValidationError::NullPointer);
    }

    let addr = UserVirtAddr::new(ptr);

    // TODO: Implémenter une vérification d'alignement si nécessaire pour certaines architectures.
    // if addr.as_usize() % core::mem::align_of::<u8>() != 0 { return Err(ValidationError::Misaligned); }

    // Vérifier que la plage ne déborde pas de l'espace d'adressage utilisateur.
    if addr.as_usize().checked_add(size).is_none() {
        return Err(ValidationError::InvalidRange);
    }
    if !addr.is_user() || !addr.add(size - 1).is_user() {
        return Err(ValidationError::InvalidRange);
    }

    // Vérifier que la plage est couverte par des VmArea (Virtual Memory Areas)
    // et que les permissions sont suffisantes.
    let current_task = current();
    let vm_space = current_task.vm_space();
    
    let (write, execute) = match perm {
        Permission::Read => (false, false),
        Permission::Write | Permission::ReadWrite => (true, false),
        Permission::Execute => (false, true),
    };

    if !vm_space.is_range_accessible(addr, size, write, execute) {
        return Err(ValidationError::PermissionDenied);
    }

    Ok(addr)
}

/// Wrapper sûr pour une tranche de mémoire en espace utilisateur.
///
/// Ce type garantit que la mémoire sous-jacente a été validée.
/// Il ne peut être construit que via les fonctions `validate_*`.
pub struct UserSlice {
    addr: UserVirtAddr,
    size: usize,
}

impl UserSlice {
    /// Crée un `UserSlice` en validant une tranche en lecture seule.
    pub fn validate_read(ptr: usize, size: usize) -> Result<Self, ValidationError> {
        let addr = validate_user_ptr(ptr, size, Permission::Read)?;
        Ok(Self { addr, size })
    }

    /// Crée un `UserSlice` en validant une tranche en lecture/écriture.
    pub fn validate_write(ptr: usize, size: usize) -> Result<Self, ValidationError> {
        let addr = validate_user_ptr(ptr, size, Permission::ReadWrite)?;
        Ok(Self { addr, size })
    }

    /// Retourne l'adresse de départ de la tranche.
    pub fn addr(&self) -> UserVirtAddr {
        self.addr
    }

    /// Retourne la taille de la tranche.
    pub fn size(&self) -> usize {
        self.size
    }
}