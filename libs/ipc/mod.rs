// libs/exo_std/src/ipc/mod.rs
pub mod channel;
pub mod shared_mem;
pub mod async_channel;

pub use channel::{Channel, Sender, Receiver};
pub use shared_mem::{SharedMemory, SharedMemoryBuilder};
pub use async_channel::{AsyncChannel, AsyncSender, AsyncReceiver};

use crate::io::Result as IoResult;
use exo_types::Capability;

/// Crée un nouveau canal IPC synchronisé
pub fn channel<T>() -> IoResult<(Sender<T>, Receiver<T>)> {
    Channel::new()
}

/// Crée un nouveau canal IPC asynchrone
pub fn async_channel<T>() -> IoResult<(AsyncSender<T>, AsyncReceiver<T>)> {
    AsyncChannel::new()
}

/// Ouvre une mémoire partagée existante
pub fn open_shared_memory(name: &str) -> IoResult<SharedMemory> {
    SharedMemory::open(name)
}

/// Attends plusieurs canaux IPC
pub fn select<'a, T>(channels: &'a [Receiver<T>]) -> IoResult<(&'a Receiver<T>, T)> {
    sys_select(channels)
}

// Appels système pour les opérations IPC
fn sys_select<'a, T>(_channels: &'a [Receiver<T>]) -> IoResult<(&'a Receiver<T>, T)> {
    #[cfg(feature = "test_mode")]
    {
        // En mode test, retourner le premier canal et une valeur par défaut
        Err(crate::io::IoError::NotSupported)
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        // Implémentation réelle avec des appels système
        Err(crate::io::IoError::NotSupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_channel_roundtrip() {
        let (tx, rx) = channel::<u32>().unwrap();
        
        tx.send(42).unwrap();
        let received = rx.recv().unwrap();
        
        assert_eq!(received, 42);
    }
    
    #[test]
    fn test_async_channel() {
        let (tx, rx) = async_channel::<u32>().unwrap();
        
        tx.try_send(42).unwrap();
        let received = rx.try_recv().unwrap();
        
        assert_eq!(received, 42);
    }
    
    #[test]
    fn test_shared_memory() {
        let name = "test_shm";
        let size = 4096;
        
        // Créer une mémoire partagée
        let shm = SharedMemoryBuilder::new()
            .name(name)
            .size(size)
            .create()
            .unwrap();
        
        // Écrire dans la mémoire
        let slice = unsafe { core::slice::from_raw_parts_mut(shm.as_ptr() as *mut u8, size) };
        slice[0..4].copy_from_slice(b"TEST");
        
        // Ouvrir la même mémoire partagée
        let shm2 = open_shared_memory(name).unwrap();
        let slice2 = unsafe { core::slice::from_raw_parts(shm2.as_ptr() as *const u8, size) };
        
        assert_eq!(&slice2[0..4], b"TEST");
    }
}