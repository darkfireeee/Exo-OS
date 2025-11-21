
### libs/exo_std/src/ipc/shared_mem.rs
```rust
// libs/exo_std/src/ipc/shared_mem.rs
use core::sync::atomic::{AtomicU64, Ordering};
use alloc::sync::Arc;
use crate::io::{Result as IoResult, IoError};
use exo_types::{PhysAddr, VirtAddr, Capability, Rights};

/// Builder pour créer une mémoire partagée
pub struct SharedMemoryBuilder {
    name: Option<String>,
    size: usize,
    executable: bool,
    persist: bool,
}

/// Représente une région de mémoire partagée
pub struct SharedMemory {
    handle: u64,
    ptr: VirtAddr,
    size: usize,
    capability: Capability,
    persist: bool,
}

impl SharedMemoryBuilder {
    /// Crée un nouveau builder
    pub fn new() -> Self {
        SharedMemoryBuilder {
            name: None,
            size: 4096, // Page size par défaut
            executable: false,
            persist: false,
        }
    }
    
    /// Définit le nom de la mémoire partagée
    pub fn name<S: Into<String>>(&mut self, name: S) -> &mut Self {
        self.name = Some(name.into());
        self
    }
    
    /// Définit la taille en octets
    pub fn size(&mut self, size: usize) -> &mut Self {
        self.size = size;
        self
    }
    
    /// Rend la mémoire exécutable
    pub fn executable(&mut self, executable: bool) -> &mut Self {
        self.executable = executable;
        self
    }
    
    /// Définit si la mémoire persiste après la fermeture
    pub fn persist(&mut self, persist: bool) -> &mut Self {
        self.persist = persist;
        self
    }
    
    /// Crée la mémoire partagée
    pub fn create(self) -> IoResult<SharedMemory> {
        sys_create_shared_memory(
            self.name.as_deref(),
            self.size,
            self.executable,
            self.persist,
        )
    }
}

impl SharedMemory {
    /// Ouvre une mémoire partagée existante par nom
    pub fn open(name: &str) -> IoResult<Self> {
        sys_open_shared_memory(name)
    }
    
    /// Ouvre une mémoire partagée existante par handle
    pub fn open_from_handle(handle: u64) -> IoResult<Self> {
        sys_open_shared_memory_by_handle(handle)
    }
    
    /// Retourne le handle de la mémoire partagée
    pub fn handle(&self) -> u64 {
        self.handle
    }
    
    /// Retourne un pointeur vers la mémoire
    pub fn as_ptr(&self) -> *mut u8 {
        self.ptr.as_mut_ptr()
    }
    
    /// Retourne la taille de la mémoire
    pub fn size(&self) -> usize {
        self.size
    }
    
    /// Écrit des données dans la mémoire partagée
    pub fn write(&self, offset: usize,  &[u8]) -> IoResult<()> {
        if offset + data.len() > self.size {
            return Err(IoError::InvalidInput);
        }
        
        unsafe {
            let dst = self.ptr.offset(offset as isize).as_mut_ptr::<u8>();
            core::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
        }
        
        Ok(())
    }
    
    /// Lit des données depuis la mémoire partagée
    pub fn read(&self, offset: usize, buf: &mut [u8]) -> IoResult<()> {
        if offset + buf.len() > self.size {
            return Err(IoError::InvalidInput);
        }
        
        unsafe {
            let src = self.ptr.offset(offset as isize).as_ptr::<u8>();
            core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), buf.len());
        }
        
        Ok(())
    }
    
    /// Mappe la mémoire partagée dans l'espace d'adressage
    pub fn map(&self, virt_addr: Option<VirtAddr>) -> IoResult<VirtAddr> {
        sys_map_shared_memory(self.handle, virt_addr)
    }
    
    /// Démapper la mémoire partagée
    pub fn unmap(&self) -> IoResult<()> {
        sys_unmap_shared_memory(self.handle)
    }
}

impl Drop for SharedMemory {
    fn drop(&mut self) {
        if !self.persist {
            let _ = sys_close_shared_memory(self.handle);
        }
    }
}

// Appels système pour la mémoire partagée
fn sys_create_shared_memory(
    name: Option<&str>,
    size: usize,
    executable: bool,
    persist: bool,
) -> IoResult<SharedMemory> {
    #[cfg(feature = "test_mode")]
    {
        // Mode test: créer une mémoire factice
        let handle = static SHM_HANDLE_COUNTER: AtomicU64 = AtomicU64::new(1000);
        let handle = SHM_HANDLE_COUNTER.fetch_add(1, Ordering::SeqCst);
        
        // Allouer de la mémoire simulée
        let data = vec![0u8; size];
        let ptr = data.as_ptr() as usize;
        core::mem::forget(data); // Ne pas libérer la mémoire
        
        let capability = Capability::new(
            handle,
            exo_types::CapabilityType::Memory,
            if executable {
                Rights::READ | Rights::WRITE | Rights::EXECUTE
            } else {
                Rights::READ | Rights::WRITE
            }
        ).with_metadata(exo_types::CapabilityMeta:for_memory(size, executable));
        
        Ok(SharedMemory {
            handle,
            ptr: VirtAddr::new(ptr),
            size,
            capability,
            persist,
        })
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        unsafe {
            extern "C" {
                fn sys_create_shared_memory(
                    name: *const u8,
                    name_len: usize,
                    size: usize,
                    executable: u8,
                    persist: u8,
                    handle: *mut u64,
                    phys_addr: *mut usize,
                ) -> i32;
            }
            
            let mut handle = 0;
            let mut phys_addr = 0;
            
            let name_ptr = name.map(|n| n.as_ptr()).unwrap_or(core::ptr::null());
            let name_len = name.map(|n| n.len()).unwrap_or(0);
            
            let result = sys_create_shared_memory(
                name_ptr,
                name_len,
                size,
                executable as u8,
                persist as u8,
                &mut handle,
                &mut phys_addr,
            );
            
            if result != 0 {
                return Err(IoError::Other);
            }
            
            // Mapper dans l'espace virtuel
            let virt_addr = sys_map_physical(PhysAddr::new(phys_addr), size)?;
            
            let capability = Capability::new(
                handle,
                exo_types::CapabilityType::Memory,
                if executable {
                    Rights::READ | Rights::WRITE | Rights::EXECUTE
                } else {
                    Rights::READ | Rights::WRITE
                }
            ).with_metadata(exo_types::CapabilityMeta:for_memory(size, executable));
            
            Ok(SharedMemory {
                handle,
                ptr: virt_addr,
                size,
                capability,
                persist,
            })
        }
    }
}

fn sys_open_shared_memory(name: &str) -> IoResult<SharedMemory> {
    // Implémentation similaire à sys_create_shared_memory
    // mais ouvre une mémoire existante par nom
    #[cfg(feature = "test_mode")]
    {
        sys_create_shared_memory(Some(name), 4096, false, false)
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        // TODO: Implémentation réelle
        Err(IoError::NotFound)
    }
}

fn sys_open_shared_memory_by_handle(handle: u64) -> IoResult<SharedMemory> {
    #[cfg(feature = "test_mode")]
    {
        // Mode test: retourner une mémoire factice
        let data = vec![0u8; 4096];
        let ptr = data.as_ptr() as usize;
        core::mem::forget(data);
        
        let capability = Capability::new(
            handle,
            exo_types::CapabilityType::Memory,
            Rights::READ | Rights::WRITE
        ).with_metadata(exo_types::CapabilityMeta:for_memory(4096, false));
        
        Ok(SharedMemory {
            handle,
            ptr: VirtAddr::new(ptr),
            size: 4096,
            capability,
            persist: false,
        })
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        // TODO: Implémentation réelle
        Err(IoError::NotFound)
    }
}

fn sys_map_shared_memory(handle: u64, virt_addr: Option<VirtAddr>) -> IoResult<VirtAddr> {
    #[cfg(feature = "test_mode")]
    {
        // Mode test: retourner une adresse virtuelle factice
        Ok(VirtAddr::new(0x10000000))
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        // TODO: Implémentation réelle
        Err(IoError::NotSupported)
    }
}

fn sys_unmap_shared_memory(handle: u64) -> IoResult<()> {
    #[cfg(feature = "test_mode")]
    {
        Ok(())
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        // TODO: Implémentation réelle
        Err(IoError::NotSupported)
    }
}

fn sys_close_shared_memory(handle: u64) -> IoResult<()> {
    #[cfg(feature = "test_mode")]
    {
        Ok(())
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        unsafe {
            extern "C" {
                fn sys_close_shared_memory(handle: u64) -> i32;
            }
            let result = sys_close_shared_memory(handle);
            if result == 0 {
                Ok(())
            } else {
                Err(IoError::Other)
            }
        }
    }
}

fn sys_map_physical(phys_addr: PhysAddr, size: usize) -> IoResult<VirtAddr> {
    #[cfg(feature = "test_mode")]
    {
        // Mode test: retourner une adresse virtuelle factice
        Ok(VirtAddr::new(0x20000000))
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        unsafe {
            extern "C" {
                fn sys_map_physical(phys_addr: usize, size: usize) -> usize;
            }
            let virt_addr = sys_map_physical(phys_addr.as_usize(), size);
            if virt_addr == 0 {
                Err(IoError::Other)
            } else {
                Ok(VirtAddr::new(virt_addr))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_shared_memory_builder() {
        let shm = SharedMemoryBuilder::new()
            .name("test_shm")
            .size(8192)
            .executable(false)
            .create()
            .unwrap();
        
        assert_eq!(shm.size(), 8192);
        assert!(!shm.as_ptr().is_null());
    }
    
    #[test]
    fn test_shared_memory_rw() {
        let shm = SharedMemoryBuilder::new()
            .size(4096)
            .create()
            .unwrap();
        
        // Écrire des données
        let data = b"Hello, Exo-OS!";
        shm.write(0, data).unwrap();
        
        // Lire les données
        let mut buffer = [0u8; 14];
        shm.read(0, &mut buffer).unwrap();
        
        assert_eq!(&buffer, b"Hello, Exo-OS!");
    }
    
    #[test]
    fn test_shared_memory_persistence() {
        let name = "persistent_test";
        let size = 4096;
        
        // Créer une mémoire persistante
        let shm1 = SharedMemoryBuilder::new()
            .name(name)
            .size(size)
            .persist(true)
            .create()
            .unwrap();
        
        // Écrire des données
        shm1.write(0, b"PERSISTENT DATA").unwrap();
        
        // Fermer la première instance (mais la mémoire persiste)
        drop(shm1);
        
        // Ouvrir de nouveau
        let shm2 = SharedMemory::open(name).unwrap();
        
        // Lire les données
        let mut buffer = [0u8; 15];
        shm2.read(0, &mut buffer).unwrap();
        
        assert_eq!(&buffer[..14], b"PERSISTENT DATA");
    }
    
    #[test]
    fn test_shared_memory_handle() {
        let shm = SharedMemoryBuilder::new()
            .size(4096)
            .create()
            .unwrap();
        
        let handle = shm.handle();
        
        // Ouvrir via le handle
        let shm2 = SharedMemory::open_from_handle(handle).unwrap();
        
        // Vérifier que c'est la même mémoire
        assert_eq!(shm2.handle(), handle);
        assert_eq!(shm2.size(), 4096);
    }
}