//! Typed Channel Wrappers pour Syscalls IPC
//!
//! Fournit des wrappers type-safe pour les canaux IPC utilisés par les syscalls

use crate::ipc::channel::typed::TypedChannel;
use crate::memory::{MemoryResult, MemoryError};
use core::marker::PhantomData;
use alloc::sync::Arc;

/// Handle pour canal typé utilisé dans les syscalls
pub struct SyscallTypedChannel<T> {
    channel: Arc<TypedChannel<T>>,
    _phantom: PhantomData<T>,
}

impl<T: Clone> SyscallTypedChannel<T> {
    /// Crée un nouveau canal typé pour les syscalls
    pub fn new(capacity: usize) -> MemoryResult<Self> {
        let channel = TypedChannel::new(capacity)?;
        Ok(Self {
            channel: Arc::new(channel),
            _phantom: PhantomData,
        })
    }
    
    /// Envoie un message typé
    pub fn send(&self, msg: &T) -> MemoryResult<()> {
        self.channel.send(msg)
    }
    
    /// Reçoit un message typé
    pub fn recv(&self) -> MemoryResult<T> {
        self.channel.recv()
    }
    
    /// Essaie d'envoyer sans bloquer
    pub fn try_send(&self, msg: &T) -> MemoryResult<()> {
        // TODO: Implement non-blocking version
        self.send(msg)
    }
    
    /// Essaie de recevoir sans bloquer
    pub fn try_recv(&self) -> MemoryResult<T> {
        // TODO: Implement non-blocking version
        self.recv()
    }
    
    /// Clone le handle pour partage entre processus
    pub fn clone_handle(&self) -> Self {
        Self {
            channel: Arc::clone(&self.channel),
            _phantom: PhantomData,
        }
    }
}

/// Crée une paire de canaux typés (bidirectionnel)
pub fn create_typed_pair<T: Clone>(capacity: usize) -> MemoryResult<(SyscallTypedChannel<T>, SyscallTypedChannel<T>)> {
    let channel1 = SyscallTypedChannel::new(capacity)?;
    let channel2 = SyscallTypedChannel::new(capacity)?;
    Ok((channel1, channel2))
}

/// Message typé pour communication inter-processus
#[derive(Debug, Clone)]
pub struct IpcMessage<T> {
    /// Données du message
    pub data: T,
    /// PID de l'expéditeur
    pub sender_pid: u64,
    /// Timestamp d'envoi
    pub timestamp: u64,
}

impl<T> IpcMessage<T> {
    /// Crée un nouveau message IPC
    pub fn new(data: T, sender_pid: u64) -> Self {
        Self {
            data,
            sender_pid,
            timestamp: 0, // TODO: Use real timestamp from HPET/RTC
        }
    }
}

/// Canal typé pour requêtes/réponses
pub struct RequestResponseChannel<Req, Resp> {
    request_channel: SyscallTypedChannel<Req>,
    response_channel: SyscallTypedChannel<Resp>,
}

impl<Req: Clone, Resp: Clone> RequestResponseChannel<Req, Resp> {
    /// Crée un nouveau canal requête/réponse
    pub fn new(capacity: usize) -> MemoryResult<Self> {
        Ok(Self {
            request_channel: SyscallTypedChannel::new(capacity)?,
            response_channel: SyscallTypedChannel::new(capacity)?,
        })
    }
    
    /// Envoie une requête et attend la réponse
    pub fn request(&self, req: &Req) -> MemoryResult<Resp> {
        self.request_channel.send(req)?;
        self.response_channel.recv()
    }
    
    /// Reçoit une requête (côté serveur)
    pub fn recv_request(&self) -> MemoryResult<Req> {
        self.request_channel.recv()
    }
    
    /// Envoie une réponse (côté serveur)
    pub fn send_response(&self, resp: &Resp) -> MemoryResult<()> {
        self.response_channel.send(resp)
    }
}
