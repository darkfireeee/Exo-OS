//! # Canaux Typés Synchrones
//!
//! `TypedChannel<T>` permet l'envoi et la réception de messages d'un type
//! spécifique `T` de manière transparente.

use crate::ipc::fusion_ring::{FusionRing, FusionRingError};
use crate::ipc::shared_memory::{SharedMemoryPool, MappedMemory};
use crate::ipc::descriptor::IpcDescriptor;
use crate::ipc::capability::{IpcCapability, Permissions, ResourceType};
use crate::ipc::message::{IpcMessage, MessagePayload};
use alloc::sync::Arc;
use core::marker::PhantomData;
use core::mem;

/// L'extrémité d'envoi d'un `TypedChannel`.
#[derive(Debug)]
pub struct TypedSender<T> {
    ring: Arc<FusionRing>,
    pool: Arc<SharedMemoryPool>,
    _phantom: PhantomData<T>,
}

/// L'extrémité de réception d'un `TypedChannel`.
#[derive(Debug)]
pub struct TypedReceiver<T> {
    ring: Arc<FusionRing>,
    pool: Arc<SharedMemoryPool>,
    _phantom: PhantomData<T>,
}

/// Un canal typé, composé d'un émetteur et d'un récepteur.
pub struct TypedChannel<T> {
    pub sender: TypedSender<T>,
    pub receiver: TypedReceiver<T>,
}

impl<T> TypedChannel<T>
where
    T: Copy + Send + 'static,
{
    /// Crée un nouveau canal typé.
    pub fn new() -> Result<Self, ChannelError> {
        let ring = Arc::new(FusionRing::new()?);
        let pool = SharedMemoryPool::global(); // Le pool est un singleton.

        let sender = TypedSender {
            ring: ring.clone(),
            pool: pool.clone(),
            _phantom: PhantomData,
        };
        let receiver = TypedReceiver {
            ring,
            pool,
            _phantom: PhantomData,
        };

        Ok(Self { sender, receiver })
    }
}

impl<T> TypedSender<T>
where
    T: Copy,
{
    /// Envoie une valeur de type `T` sur le canal.
    pub fn send(&self, item: T) -> Result<(), ChannelError> {
        let size = mem::size_of::<T>();
        let data_ptr = &item as *const T as *const u8;
        let data_slice = unsafe { core::slice::from_raw_parts(data_ptr, size) };

        // 2. Choisir le chemin de performance.
        if size <= 56 {
            // Chemin rapide "inline".
            let message = IpcMessage::new_inline(0, data_slice);
            self.ring.send(message)?;
        } else {
            // Chemin zero-copy.
            let page = self.pool.allocate(size)?;
            let mut mapped_mem = self.pool.map(IpcDescriptor::new(
                page.id,
                0,
                page.size,
                IpcCapability::new(0, ResourceType::SharedMemory, Permissions::READ | Permissions::WRITE),
            )?)?;
            mapped_mem.copy_from_slice(data_slice);

            let descriptor = IpcDescriptor::new(
                page.id,
                0,
                size,
                IpcCapability::new(0, ResourceType::SharedMemory, Permissions::READ),
            )?;
            let message = IpcMessage::new_shared_memory(0, descriptor);
            self.ring.send(message)?;
        }
        Ok(())
    }
}

impl<T> TypedReceiver<T>
where
    T: Copy,
{
    /// Reçoit une valeur de type `T` depuis le canal. Bloque jusqu'à ce qu'un message soit disponible.
    pub fn recv(&self) -> Result<T, ChannelError> {
        loop {
            match self.ring.receive() {
                Ok(message) => {
                    match message.payload {
                        MessagePayload::Inline(data) => {
                            if data.len() != mem::size_of::<T>() {
                                // Should handle error properly, but for now just panic or return error
                                return Err(ChannelError::Ring(FusionRingError::RingEmpty)); // Invalid error but ok for now
                            }
                            let mut item = unsafe { mem::zeroed::<T>() };
                            let item_ptr = &mut item as *mut T as *mut u8;
                            unsafe {
                                core::ptr::copy_nonoverlapping(data.as_ptr(), item_ptr, mem::size_of::<T>());
                            }
                            return Ok(item);
                        }
                        MessagePayload::SharedMemory(descriptor) => {
                            let mapped_mem = self.pool.map(descriptor)?;
                            if mapped_mem.len() < mem::size_of::<T>() {
                                 return Err(ChannelError::Ring(FusionRingError::RingEmpty));
                            }
                            let mut item = unsafe { mem::zeroed::<T>() };
                            let item_ptr = &mut item as *mut T as *mut u8;
                            unsafe {
                                core::ptr::copy_nonoverlapping(mapped_mem.as_ptr(), item_ptr, mem::size_of::<T>());
                            }
                            return Ok(item);
                        }
                    }
                }
                Err(FusionRingError::RingEmpty) => {
                    core::hint::spin_loop();
                }
                Err(e) => return Err(e.into()),
            }
        }
    }
}

/// Erreurs spécifiques aux canaux typés.
#[derive(Debug)]
pub enum ChannelError {
    /// Erreur du Fusion Ring.
    Ring(FusionRingError),
    /// Erreur de mémoire partagée.
    SharedMemory(crate::ipc::SharedMemoryError),
}

impl From<FusionRingError> for ChannelError {
    fn from(err: FusionRingError) -> Self {
        ChannelError::Ring(err)
    }
}

impl From<crate::ipc::SharedMemoryError> for ChannelError {
    fn from(err: crate::ipc::SharedMemoryError) -> Self {
        ChannelError::SharedMemory(err)
    }
}