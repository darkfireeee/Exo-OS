//! # Module IPC (Inter-Process Communication)
//! 
//! Ce module fournit des mécanismes de communication inter-processus haute performance
//! avec une latence cible de < 500ns pour les messages rapides.

pub mod channel;
pub mod message;

#[cfg(feature = "fusion_rings")]
pub mod fusion_ring;

#[cfg(test)]
pub mod bench_fusion;

use alloc::collections::BTreeMap;
use spin::Mutex;
use lazy_static::lazy_static;
use core::sync::atomic::{AtomicU32, Ordering};
use crate::println;

lazy_static! {
    /// Registre global des canaux IPC
    static ref CHANNEL_REGISTRY: Mutex<BTreeMap<u32, channel::ChannelHandle>> = 
        Mutex::new(BTreeMap::new());
}

/// Identifiant unique pour les canaux IPC
static NEXT_CHANNEL_ID: AtomicU32 = AtomicU32::new(1);

/// Types de messages IPC supportés
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MessageType {
    /// Message rapide via registres (≤ 64 octets)
    Fast,
    /// Message plus grand nécessitant une copie en mémoire
    Buffered,
    /// Message avec descripteurs de fichiers ou handles
    WithHandles,
}

/// Crée un nouveau canal IPC et retourne son identifiant
pub fn create_channel(name: &str, buffer_size: usize) -> Result<u32, &'static str> {
    let id = NEXT_CHANNEL_ID.fetch_add(1, Ordering::SeqCst);
    let channel = channel::Channel::new(name, buffer_size);
    let handle = channel::ChannelHandle::new(id, channel);
    
    let mut registry = CHANNEL_REGISTRY.lock();
    registry.insert(id, handle);
    
    Ok(id)
}

/// Récupère un canal par son identifiant
pub fn get_channel(id: u32) -> Option<channel::ChannelHandle> {
    let registry = CHANNEL_REGISTRY.lock();
    registry.get(&id).cloned()
}

/// Envoie un message via un canal IPC
pub fn send_message(channel_id: u32, msg: message::Message) -> Result<(), &'static str> {
    if let Some(channel) = get_channel(channel_id) {
        channel.send(msg)
    } else {
        Err("Canal introuvable")
    }
}

/// Reçoit un message depuis un canal IPC
pub fn receive_message(channel_id: u32) -> Result<message::Message, &'static str> {
    if let Some(channel) = get_channel(channel_id) {
        channel.receive()
    } else {
        Err("Canal introuvable")
    }
}

/// Initialise le système IPC
pub fn init() {
    println!("[IPC] Initialisation du système IPC...");
    
    // Créer les canaux par défaut
    let _ = create_channel("kernel", 256);
    let _ = create_channel("debug", 128);
    let _ = create_channel("broadcast", 512);
    let _ = create_channel("log", 256);
    
    println!("[IPC] Système IPC initialisé avec 4 canaux par défaut.");
}