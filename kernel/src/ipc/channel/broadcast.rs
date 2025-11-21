//! # Canal de Diffusion (Broadcast)
//!
//! `BroadcastChannel<T>` permet à un producteur d'envoyer un message à
//! plusieurs consommateurs (1->N). Il utilise une architecture "fan-out"
//! où une tâche dédiée lit depuis un anneau central et redistribue les
//! messages à des anneaux privés pour chaque consommateur.

use super::typed::{TypedSender, TypedReceiver, ChannelError};
// use serde::{Serialize, Deserialize};

/// L'extrémité d'envoi pour un canal de diffusion.
#[derive(Debug)]
pub struct BroadcastSender<T> {
    sender: TypedSender<T>,
}

/// L'extrémité de réception pour un canal de diffusion.
#[derive(Debug)]
pub struct BroadcastReceiver<T> {
    receiver: TypedReceiver<T>,
}

/// Un canal de diffusion.
pub struct BroadcastChannel<T> {
    _marker: core::marker::PhantomData<T>,
}

impl<T> BroadcastChannel<T>
where
    T: Serialize + for<'de> Deserialize<'de> + Send + Clone + 'static,
{
    /// Crée un nouveau canal de diffusion.
    pub fn new() -> Result<(Self, BroadcastSender<T>, BroadcastReceiver<T>), ChannelError> {
        // TODO: Implement broadcast channel without tokio
        Err(ChannelError::InternalError)
    }
}

impl<T> BroadcastSender<T>
where
    T: Serialize,
{
    pub fn send(&self, item: T) -> Result<(), ChannelError> {
        self.sender.send(item)
    }
}

impl<T> BroadcastReceiver<T>
where
    T: for<'de> Deserialize<'de>,
{
    pub fn recv(&self) -> Result<T, ChannelError> {
        self.receiver.recv()
    }
}