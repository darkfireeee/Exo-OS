<<<<<<< Updated upstream
// libs/exo_std/src/ipc.rs
//! Communication inter-processus (IPC)
//!
//! Ce module fournit des mécanismes pour la communication entre processus.

use crate::Result;
use crate::error::{IpcError, ExoStdError};

// Réexporte les types de exo_ipc
pub use exo_ipc::*;

/// ID de canal IPC
pub type ChannelId = u32;

/// Envoie un message via IPC
///
/// # Exemple
/// ```no_run
/// use exo_std::ipc;
///
/// let dest = 42; // PID destination
/// let message = b"Hello, process!";
/// ipc::send(dest, message).unwrap();
/// ```
pub fn send(dest: ChannelId, data: &[u8]) -> Result<()> {
    if data.is_empty() {
        return Err(ExoStdError::Ipc(IpcError::InvalidDestination));
    }
    
=======
//! Inter-Process Communication (IPC)
//!
//! Ce module fournit les primitives pour la communication entre processus.

pub use exo_ipc::*;

/// Envoie un message à un processus
///
/// # Arguments
///
/// * `dest` - ID du processus destination
/// * `data` - Données à envoyer
///
/// # Exemples
///
/// ```ignore
/// use exo_std::ipc::send;
///
/// let data = b"Hello, process!";
/// send(123, data).unwrap();
/// ```
pub fn send(dest: u32, data: &[u8]) -> crate::Result<()> {
>>>>>>> Stashed changes
    #[cfg(feature = "test_mode")]
    {
        let _ = (dest, data);
        Ok(())
    }
    
    #[cfg(not(feature = "test_mode"))]
<<<<<<< Updated upstream
    {
        // TODO: Appel système réel pour envoyer via IPC
        // unsafe {
        //     extern "C" {
        //         fn sys_ipc_send(dest: u32, data: *const u8, len: usize) -> i32;
        //     }
        //     let result = sys_ipc_send(dest, data.as_ptr(), data.len());
        //     if result == 0 { 
        //         Ok(()) 
        //     } else { 
        //         Err(ExoStdError::Ipc(IpcError::Other))
        //     }
        // }
        let _ = (dest, data);
        Ok(())
    }
}

/// Reçoit un message via IPC
///
/// Bloque jusqu'à réception d'un message.
///
/// # Exemple
/// ```no_run
/// use exo_std::ipc;
///
/// let mut buffer = vec![0u8; 1024];
/// let (sender, len) = ipc::receive(&mut buffer).unwrap();
/// println!("Reçu {} bytes de {}", len, sender);
/// ```
pub fn receive(buffer: &mut [u8]) -> Result<(ChannelId, usize)> {
    if buffer.is_empty() {
        return Err(ExoStdError::Ipc(IpcError::MessageTooLarge));
    }
    
=======
    unsafe {
        use crate::syscall::{syscall3, SyscallId};
        
        let result = syscall3(
            SyscallId::Send,
            dest as usize,
            data.as_ptr() as usize,
            data.len(),
        );
        
        if result < 0 {
            Err(crate::error::Error::Io(crate::error::IoError::BrokenPipe))
        } else {
            Ok(())
        }
    }
}

/// Reçoit un message
///
/// # Arguments
///
/// * `buffer` - Buffer pour recevoir les données
///
/// # Returns
///
/// Nombre d'octets reçus et ID du processus expéditeur
pub fn recv(buffer: &mut [u8]) -> crate::Result<(usize, u32)> {
>>>>>>> Stashed changes
    #[cfg(feature = "test_mode")]
    {
        let _ = buffer;
        Ok((0, 0))
    }
    
    #[cfg(not(feature = "test_mode"))]
<<<<<<< Updated upstream
    {
        // TODO: Syscall pour recevoir
        let _ = buffer;
        Ok((0, 0))
    }
}

/// Tente de recevoir sans bloquer
///
/// Retourne None si aucun message disponible.
pub fn try_receive(buffer: &mut [u8]) -> Result<Option<(ChannelId, usize)>> {
    #[cfg(feature = "test_mode")]
    {
        let _ = buffer;
        Ok(None)
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        // TODO: Syscall non-bloquant
        let _ = buffer;
        Ok(None)
    }
}

/// Crée un nouveau canal IPC
///
/// Retourne l'ID du canal créé.
pub fn create_channel() -> Result<ChannelId> {
    #[cfg(feature = "test_mode")]
    {
        Ok(1)
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        // TODO: Syscall pour créer canal
        Ok(1)
    }
}

/// Ferme un canal IPC
pub fn close_channel(channel: ChannelId) -> Result<()> {
    #[cfg(feature = "test_mode")]
    {
        let _ = channel;
        Ok(())
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        // TODO: Syscall pour fermer
        let _ = channel;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_create_channel() {
        let channel = create_channel().unwrap();
        assert!(channel > 0);
    }
    
    #[test]
    fn test_send() {
        let result = send(1, b"test message");
        assert!(result.is_ok());
=======
    unsafe {
        use crate::syscall::{syscall2, SyscallId};
        
        let result = syscall2(
            SyscallId::Recv,
            buffer.as_mut_ptr() as usize,
            buffer.len(),
        );
        
        if result < 0 {
            Err(crate::error::Error::Io(crate::error::IoError::BrokenPipe))
        } else {
            // Les bits hauts contiennent le PID, les bits bas la taille
            let size = (result & 0xFFFF) as usize;
            let pid = ((result >> 16) & 0xFFFF) as u32;
            Ok((size, pid))
        }
>>>>>>> Stashed changes
    }
}
