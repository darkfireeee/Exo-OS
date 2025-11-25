//! # Canal Typé (TypedChannel)
//!
//! `TypedChannel<T>` permet l'envoi et la réception de messages d'un type
//! spécifique `T`. Les données sont automatiquement sérialisées/désérialisées.

use crate::ipc::fusion_ring::{Ring, FusionRing};
use crate::memory::{MemoryResult, MemoryError};
use alloc::sync::Arc;
use core::marker::PhantomData;
use core::mem;

/// L'extrémité d'envoi d'un `TypedChannel`.
#[derive(Clone)]
pub struct TypedSender<T> {
    ring: Arc<FusionRing>,
    _phantom: PhantomData<T>,
}

/// L'extrémité de réception d'un `TypedChannel`.
#[derive(Clone)]
pub struct TypedReceiver<T> {
    ring: Arc<FusionRing>,
    _phantom: PhantomData<T>,
}

/// Un canal de communication typé
pub struct TypedChannel<T> {
    pub sender: TypedSender<T>,
    pub receiver: TypedReceiver<T>,
}

impl<T> TypedChannel<T>
where
    T: Copy + Send + 'static,
{
    /// Crée un nouveau canal typé
    pub fn new(capacity: usize) -> MemoryResult<Self> {
        // Allocate ring from fusion_ring (returns &'static Ring)
        let actual_ring = crate::ipc::fusion_ring::ring::Ring::new(capacity);
        let ring = FusionRing {
            ring: Some(actual_ring), // Now actual_ring is &'static Ring
            sync: crate::ipc::fusion_ring::sync::RingSync::new(),
        };
        let ring = Arc::new(ring);
        
        let sender = TypedSender {
            ring: Arc::clone(&ring),
            _phantom: PhantomData,
        };
        
        let receiver = TypedReceiver {
            ring,
            _phantom: PhantomData,
        };
        
        Ok(TypedChannel { sender, receiver })
    }
}

impl<T> TypedSender<T>
where
    T: Copy + Send + 'static,
{
    /// Envoie un message
    pub fn send(&self, msg: T) -> MemoryResult<()> {
        // Sérialise le message en bytes
        let data = unsafe {
            core::slice::from_raw_parts(
                &msg as *const T as *const u8,
                mem::size_of::<T>(),
            )
        };
        
        self.ring.send(data)
    }
    
    /// Essaie d'envoyer un message sans bloquer
    pub fn try_send(&self, msg: T) -> MemoryResult<()> {
        self.send(msg)
    }
    
    /// Envoie un message en bloquant si nécessaire
    pub fn send_blocking(&self, msg: T) -> MemoryResult<()> {
        let data = unsafe {
            core::slice::from_raw_parts(
                &msg as *const T as *const u8,
                mem::size_of::<T>(),
            )
        };
        
        self.ring.send_blocking(data)
    }
}

impl<T> TypedReceiver<T>
where
    T: Copy + Send + 'static,
{
    /// Reçoit un message
    pub fn recv(&self) -> MemoryResult<T> {
        let mut buffer = [0u8; 4096]; // Max message size
        let len = self.ring.recv(&mut buffer)?;
        
        if len != mem::size_of::<T>() {
            log::error!("Size mismatch: expected {} bytes, got {} bytes", mem::size_of::<T>(), len);
            return Err(MemoryError::OutOfMemory); // TODO: Create IpcError::InvalidMessageSize
        }
        
        // Désérialise
        let msg = unsafe {
            core::ptr::read_unaligned(buffer.as_ptr() as *const T)
        };
        
        Ok(msg)
    }
    
    /// Essaie de recevoir sans bloquer
    pub fn try_recv(&self) -> MemoryResult<T> {
        self.recv()
    }
    
    /// Reçoit en bloquant
    pub fn recv_blocking(&self) -> MemoryResult<T> {
        let mut buffer = [0u8; 4096];
        let len = self.ring.recv_blocking(&mut buffer)?;
        
        if len != mem::size_of::<T>() {
            return Err(MemoryError::OutOfMemory);
        }
        
        let msg = unsafe {
            core::ptr::read_unaligned(buffer.as_ptr() as *const T)
        };
        
        Ok(msg)
    }
}

/// Erreur de canal
#[derive(Debug, Clone, Copy)]
pub enum ChannelError {
    Full,
    Empty,
    Closed,
    InvalidSize,
}

// Fonction helper pour créer un canal
pub fn typed_channel<T>(capacity: usize) -> MemoryResult<(TypedSender<T>, TypedReceiver<T>)>
where
    T: Copy + Send + 'static,
{
    let channel = TypedChannel::new(capacity)?;
    Ok((channel.sender, channel.receiver))
}
