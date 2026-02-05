//! Appels système IPC

use super::{SysResult, syscall2, syscall3, SyscallId};
use crate::error::{SystemError, IpcError};

/// Envoie un message
pub unsafe fn send(channel_id: u64, buffer: &[u8]) -> Result<(), IpcError> {
    let result = syscall3(
        SyscallId::Send,
        channel_id as usize,
        buffer.as_ptr() as usize,
        buffer.len(),
    );

    if result < 0 {
        Err(IpcError::Other)
    } else {
        Ok(())
    }
}

/// Reçoit un message
pub unsafe fn recv(channel_id: u64, buffer: &mut [u8]) -> Result<usize, IpcError> {
    let result = syscall3(
        SyscallId::Recv,
        channel_id as usize,
        buffer.as_mut_ptr() as usize,
        buffer.len(),
    );

    if result < 0 {
        Err(IpcError::Other)
    } else {
        Ok(result as usize)
    }
}
