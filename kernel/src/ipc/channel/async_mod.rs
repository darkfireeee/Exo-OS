//! # Canaux Typés Asynchrones
//!
//! `AsyncChannel<T>` est la version asynchrone de `TypedChannel<T>`.
//!
//! NOTE: This module is currently disabled because it depends on `std` and `tokio`,
//! which are not available in the kernel.
//!

/*
use super::typed::{TypedSender, TypedReceiver, ChannelError};
use crate::ipc::fusion_ring::FusionRingError;
use crate::ipc::message::IpcMessage;
use serde::{Serialize, Deserialize};
use std::sync::Arc;
use tokio::sync::Notify;

/// L'extrémité d'envoi asynchrone.
#[derive(Debug)]
pub struct AsyncSender<T> {
    inner: TypedSender<T>,
    notify_recv: Arc<Notify>, // Notifie le récepteur qu'un message est prêt.
}

/// L'extrémité de réception asynchrone.
#[derive(Debug)]
pub struct AsyncReceiver<T> {
    inner: TypedReceiver<IpcMessage>, // Reçoit des messages bruts.
    notify_send: Arc<Notify>, // Notifie les émetteurs que de la place est libre.
}

/// Un canal asynchrone.
pub struct AsyncChannel<T> {
    pub sender: AsyncSender<T>,
    pub receiver: AsyncReceiver<T>,
}

impl<T> AsyncChannel<T>
where
    T: Serialize + for<'de> Deserialize<'de> + Send + 'static,
{
    pub fn new() -> Result<Self, ChannelError> {
        let (inner_sender, inner_receiver) = super::typed::TypedChannel::new()?;
        let notify_recv = Arc::new(Notify::new());
        let notify_send = Arc::new(Notify::new());

        let sender = AsyncSender {
            inner: inner_sender,
            notify_recv: notify_recv.clone(),
        };
        let receiver = AsyncReceiver {
            inner: inner_receiver,
            notify_send,
        };

        Ok(Self { sender, receiver })
    }
}

impl<T> AsyncSender<T>
where
    T: Serialize,
{
    /// Envoie une valeur de manière asynchrone.
    pub async fn send(&self, item: T) -> Result<(), ChannelError> {
        loop {
            match self.inner.send(item) {
                Ok(()) => {
                    // Succès : on notifie le récepteur.
                    self.notify_recv.notify_one();
                    return Ok(());
                }
                Err(ChannelError::Ring(FusionRingError::RingFull)) => {
                    // L'anneau est plein, on attend qu'une place se libère.
                    // `notified()` est un future qui se résout quand `notify_one()` est appelé.
                    self.notify_recv.notified().await;
                }
                Err(e) => return Err(e),
            }
        }
    }
}

impl<T> AsyncReceiver<T>
where
    T: for<'de> Deserialize<'de>,
{
    /// Reçoit une valeur de manière asynchrone.
    pub async fn recv(&self) -> Result<T, ChannelError> {
        loop {
            match self.inner.recv() {
                Ok(message) => {
                    // Succès : on notifie un éventuel émetteur en attente.
                    self.notify_send.notify_one();
                    // Désérialisation et retour.
                    return super::typed::deserialize_ipc_message(message);
                }
                Err(ChannelError::Ring(FusionRingError::RingEmpty)) => {
                    // L'anneau est vide, on attend qu'un message arrive.
                    self.notify_send.notified().await;
                }
                Err(e) => return Err(e),
            }
        }
    }
}

// Helper pour la désérialisation, déplacé depuis TypedReceiver pour éviter la duplication.
fn deserialize_ipc_message<T>(message: IpcMessage) -> Result<T, ChannelError>
where
    T: for<'de> Deserialize<'de>,
{
    match message.payload {
        MessagePayload::Inline(data) => Ok(bincode::deserialize(&data)?),
        // ... logique pour shared_memory ...
        _ => Err(ChannelError::Serialization(bincode::ErrorKind::Custom("Unsupported payload type".to_string()).into())),
    }
}
*/