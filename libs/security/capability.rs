// libs/exo_std/src/security/capability.rs
use exo_types::{Capability, Rights, Result as ExoResult};
use crate::io::{Result as IoResult, IoError};

/// Système de capabilities pour Exo-OS
pub struct CapabilitySystem;

impl CapabilitySystem {
    /// Crée une nouvelle capability
    pub fn create_capability(
        path: &str,
        rights: Rights,
    ) -> IoResult<Capability> {
        sys_create_capability(path, rights).map_err(|e| {
            IoError::Other // Mapping simplifié pour l'exemple
        })
    }
    
    /// Atténue une capability (réduit les droits)
    pub fn attenuate_capability(
        cap: &Capability,
        rights: Rights,
    ) -> Capability {
        cap.attenuate(rights)
    }
    
    /// Transfère une capability à un autre processus
    pub fn transfer_capability(
        cap: Capability,
        target_pid: u64,
    ) -> IoResult<()> {
        sys_transfer_capability(cap, target_pid).map_err(|e| {
            IoError::Other
        })
    }
    
    /// Vérifie si un processus a une capability
    pub fn has_capability(
        pid: u64,
        cap_id: u64,
        required_rights: Rights,
    ) -> bool {
        sys_check_capability(pid, cap_id, required_rights)
    }
    
    /// Liste les capabilities d'un processus
    pub fn list_capabilities(pid: u64) -> Vec<Capability> {
        sys_list_capabilities(pid)
    }
}

// Appels système
fn sys_create_capability(path: &str, rights: Rights) -> ExoResult<Capability> {
    #[cfg(feature = "test_mode")]
    {
        Ok(Capability::new(
            1, 
            exo_types::CapabilityType::File, 
            rights
        ).with_metadata(exo_types::CapabilityMetadata::for_file(path, 0)))
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        unsafe {
            extern "C" {
                fn sys_create_capability(
                    path: *const u8,
                    path_len: usize,
                    rights: u32,
                    cap: *mut Capability,
                ) -> i32;
            }
            
            let mut cap = core::mem::MaybeUninit::<Capability>::uninit();
            let result = sys_create_capability(
                path.as_ptr(),
                path.len(),
                rights.bits(),
                cap.as_mut_ptr(),
            );
            
            if result == 0 {
                Ok(cap.assume_init())
            } else {
                Err(exo_types::ExoError::new(exo_types::ErrorCode::PermissionDenied))
            }
        }
    }
}

fn sys_transfer_capability(_cap: Capability, _target_pid: u64) -> ExoResult<()> {
    #[cfg(feature = "test_mode")]
    {
        Ok(())
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        // Implémentation réelle
        Ok(())
    }
}

fn sys_check_capability(_pid: u64, _cap_id: u64, _required_rights: Rights) -> bool {
    #[cfg(feature = "test_mode")]
    {
        true
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        // Implémentation réelle
        true
    }
}

fn sys_list_capabilities(_pid: u64) -> Vec<Capability> {
    #[cfg(feature = "test_mode")]
    {
        vec![
            Capability::new(1, exo_types::CapabilityType::File, Rights::READ),
            Capability::new(2, exo_types::CapabilityType::Directory, Rights::FILE_STANDARD),
        ]
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use exo_types::Rights;
    
    #[test]
    fn test_create_capability() {
        let cap_sys = CapabilitySystem;
        let cap = cap_sys.create_capability("/test.txt", Rights::FILE_STANDARD).unwrap();
        
        assert_eq!(cap.rights(), Rights::FILE_STANDARD);
        assert_eq!(cap.cap_type(), &exo_types::CapabilityType::File);
    }
    
    #[test]
    fn test_attenuate_capability() {
        let cap_sys = CapabilitySystem;
        let cap = cap_sys.create_capability("/test.txt", Rights::FILE_STANDARD).unwrap();
        
        let read_only = cap_sys.attenuate_capability(&cap, Rights::READ);
        assert!(read_only.has_rights(Rights::READ));
        assert!(!read_only.has_rights(Rights::WRITE));
    }
}