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
/// let dest = 42;
/// let message = b"Hello, process!";
/// ipc::send(dest, message).unwrap();
/// ```
pub fn send(dest: ChannelId, data: &[u8]) -> Result<()> {
    if data.is_empty() {
        return Err(ExoStdError::Ipc(IpcError::InvalidDestination));
    }

    #[cfg(feature = "test_mode")]
    {
        let _ = (dest, data);
        Ok(())
    }

    #[cfg(not(feature = "test_mode"))]
    unsafe {
        use crate::syscall::{syscall3, SyscallNumber, check_syscall_result};

        let result = syscall3(
            SyscallNumber::IpcSend,
            dest as usize,
            data.as_ptr() as usize,
            data.len()
        );
        check_syscall_result(result)?;
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

    #[cfg(feature = "test_mode")]
    {
        let _ = buffer;
        Ok((0, 0))
    }

    #[cfg(not(feature = "test_mode"))]
    unsafe {
        use crate::syscall::{syscall2, SyscallNumber, check_syscall_result};

        let mut sender = 0u32;
        let result = syscall2(
            SyscallNumber::IpcRecv,
            buffer.as_mut_ptr() as usize,
            buffer.len()
        );
        let bytes_read = check_syscall_result(result)?;

        sender = ((result as usize) >> 32) as u32;

        Ok((sender, bytes_read))
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
    unsafe {
        use crate::syscall::{syscall3, SyscallNumber};

        const IPC_TRY_RECV: usize = 1;
        let result = syscall3(
            SyscallNumber::IpcRecv,
            buffer.as_mut_ptr() as usize,
            buffer.len(),
            IPC_TRY_RECV
        );

        if result == -5 {
            Ok(None)
        } else if result < 0 {
            use crate::syscall::check_syscall_result;
            check_syscall_result(result)?;
            Ok(None)
        } else {
            let sender = ((result as usize) >> 32) as u32;
            let bytes_read = (result as usize) & 0xFFFFFFFF;
            Ok(Some((sender, bytes_read)))
        }
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
    unsafe {
        use crate::syscall::{syscall0, SyscallNumber, check_syscall_result};

        let result = syscall0(SyscallNumber::IpcCreate);
        let channel_id = check_syscall_result(result)?;
        Ok(channel_id as ChannelId)
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
    unsafe {
        use crate::syscall::{syscall1, SyscallNumber, check_syscall_result};

        let result = syscall1(SyscallNumber::Close, channel as usize);
        check_syscall_result(result)?;
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
    }
}
