// libs/exo_ipc/src/shm/region.rs
//! Régions de mémoire partagée pour zero-copy IPC

use core::ptr::NonNull;
use core::marker::PhantomData;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use alloc::sync::Arc;

use crate::types::{IpcError, IpcResult};
use crate::util::atomic::AtomicRefCount;

/// ID unique de région de mémoire partagée
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct RegionId(pub u64);

impl RegionId {
    /// Crée un nouvel ID
    pub const fn new(id: u64) -> Self {
        Self(id)
    }
    
    /// ID invalide
    pub const INVALID: Self = Self(0);
    
    /// Génère un nouvel ID unique
    pub fn generate() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

/// Permissions pour une région de mémoire partagée
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionPermissions {
    /// Lecture autorisée
    pub read: bool,
    
    /// Écriture autorisée
    pub write: bool,
    
    /// Exécution autorisée (pour JIT code)
    pub execute: bool,
}

impl RegionPermissions {
    /// Lecture seule
    pub const READ_ONLY: Self = Self {
        read: true,
        write: false,
        execute: false,
    };
    
    /// Lecture/écriture
    pub const READ_WRITE: Self = Self {
        read: true,
        write: true,
        execute: false,
    };
    
    /// Toutes les permissions
    pub const ALL: Self = Self {
        read: true,
        write: true,
        execute: true,
    };
}

/// Métadata d'une région de mémoire partagée
struct RegionMetadata {
    /// ID unique de la région
    id: RegionId,
    
    /// Taille en bytes
    size: usize,
    
    /// Permissions
    permissions: RegionPermissions,
    
    /// Compteur de références
    ref_count: AtomicRefCount,
    
    /// Nombre de mappages actifs
    mappings: AtomicUsize,
}

/// Région de mémoire partagée
pub struct SharedRegion {
    /// Pointeur vers la mémoire
    ptr: NonNull<u8>,
    
    /// Métadata  
    metadata: Arc<RegionMetadata>,
}

impl SharedRegion {
    /// Crée une nouvelle région de mémoire partagée
    ///
    /// # Arguments
    /// * `size` - Taille en bytes (alignée sur page)
    /// * `permissions` - Permissions d'accès
    ///
    /// # Returns
    /// Nouvelle région ou erreur
    pub fn new(size: usize, permissions: RegionPermissions) -> IpcResult<Self> {
        if size == 0 {
            return Err(IpcError::InvalidParameter);
        }
        
        // Aligner sur page (4KB)
        const PAGE_SIZE: usize = 4096;
        let aligned_size = (size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        
        // Allouer la mémoire
        let layout = alloc::alloc::Layout::from_size_align(aligned_size, PAGE_SIZE)
            .map_err(|_| IpcError::InvalidParameter)?;
        
        let ptr = unsafe {
            let raw_ptr = alloc::alloc::alloc_zeroed(layout);
            if raw_ptr.is_null() {
                return Err(IpcError::OutOfMemory);
            }
            NonNull::new_unchecked(raw_ptr)
        };
        
        let metadata = Arc::new(RegionMetadata {
            id: RegionId::generate(),
            size: aligned_size,
            permissions,
            ref_count: AtomicRefCount::new(1),
            mappings: AtomicUsize::new(1),
        });
        
        Ok(Self { ptr, metadata })
    }
    
    /// Récupère l'ID de la région
    pub fn id(&self) -> RegionId {
        self.metadata.id
    }
    
    /// Récupère la taille de la région
    pub fn size(&self) -> usize {
        self.metadata.size
    }
    
    /// Récupère les permissions
    pub fn permissions(&self) -> RegionPermissions {
        self.metadata.permissions
    }
    
    /// Récupère un slice en lecture seule
    pub fn as_slice(&self) -> Option<&[u8]> {
        if self.metadata.permissions.read {
            Some(unsafe {
                core::slice::from_raw_parts(self.ptr.as_ptr(), self.metadata.size)
            })
        } else {
            None
        }
    }
    
    /// Récupère un slice mutable
    pub fn as_slice_mut(&mut self) -> Option<&mut [u8]> {
        if self.metadata.permissions.write {
            Some(unsafe {
                core::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.metadata.size)
            })
        } else {
            None
        }
    }
    
    /// Crée un mapping en lecture seule (partagé entre processus)
    pub fn map_readonly(&self) -> IpcResult<SharedMapping> {
        if !self.metadata.permissions.read {
            return Err(IpcError::PermissionDenied);
        }
        
        self.metadata.ref_count.inc();
        self.metadata.mappings.fetch_add(1, Ordering::Relaxed);
        
        Ok(SharedMapping {
            ptr: self.ptr,
            size: self.metadata.size,
            metadata: self.metadata.clone(),
            _phantom: PhantomData,
        })
    }
    
    /// Nombre de mappages actifs
    pub fn mapping_count(&self) -> usize {
        self.metadata.mappings.load(Ordering::Relaxed)
    }
    
    /// Adresse brute (pour messages zero-copy)
    pub fn as_ptr(&self) -> *const u8 {
        self.ptr.as_ptr()
    }
    
    /// Adresse mutable (pour messages zero-copy)
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.ptr.as_ptr()
    }
}

impl Drop for SharedRegion {
    fn drop(&mut self) {
        if self.metadata.ref_count.dec() == 0 {
            // Dernière référence - libérer la mémoire
            unsafe {
                const PAGE_SIZE: usize = 4096;
                let layout = alloc::alloc::Layout::from_size_align_unchecked(
                    self.metadata.size,
                    PAGE_SIZE,
                );
                alloc::alloc::dealloc(self.ptr.as_ptr(), layout);
            }
        }
    }
}

unsafe impl Send for SharedRegion {}
unsafe impl Sync for SharedRegion {}

/// Mapping en lecture seule d'une région partagée
pub struct SharedMapping<'a> {
    ptr: NonNull<u8>,
    size: usize,
    metadata: Arc<RegionMetadata>,
    _phantom: PhantomData<&'a [u8]>,
}

impl<'a> SharedMapping<'a> {
    /// Récupère l'ID de la région
    pub fn id(&self) -> RegionId {
        self.metadata.id
    }
    
    /// Taille de la région
    pub fn size(&self) -> usize {
        self.size
    }
    
    /// Accès en lecture seule
    pub fn as_slice(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.ptr.as_ptr(), self.size) }
    }
}

impl<'a> Drop for SharedMapping<'a> {
    fn drop(&mut self) {
        self.metadata.ref_count.dec();
        self.metadata.mappings.fetch_sub(1, Ordering::Relaxed);
    }
}

unsafe impl<'a> Send for SharedMapping<'a> {}
unsafe impl<'a> Sync for SharedMapping<'a> {}

/*
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_region_create() {
        let region = SharedRegion::new(4096, RegionPermissions::READ_WRITE).unwrap();
        assert_eq!(region.size(), 4096);
        assert!(region.id() != RegionId::INVALID);
    }
    
    #[test]
    fn test_region_read_write() {
        let mut region = SharedRegion::new(1024, RegionPermissions::READ_WRITE).unwrap();
        
        let slice = region.as_slice_mut().unwrap();
        slice[0] = 42;
        slice[1] = 43;
        
        let read_slice = region.as_slice().unwrap();
        assert_eq!(read_slice[0], 42);
        assert_eq!(read_slice[1], 43);
    }
    
    #[test]
    fn test_region_readonly_mapping() {
        let mut region = SharedRegion::new(1024, RegionPermissions::READ_WRITE).unwrap();
        
        // Écrire des données
        region.as_slice_mut().unwrap()[0] = 99;
        
        // Créer un mapping readonly
        let mapping = region.map_readonly().unwrap();
        assert_eq!(mapping.as_slice()[0], 99);
        assert_eq!(region.mapping_count(), 2);
    }
    
    #[test]
    fn test_region_id_uniqueness() {
        let region1 = SharedRegion::new(1024, RegionPermissions::READ_WRITE).unwrap();
        let region2 = SharedRegion::new(1024, RegionPermissions::READ_WRITE).unwrap();
        
        assert_ne!(region1.id(), region2.id());
    }
}
*/
