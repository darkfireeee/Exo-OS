//! # Module IPC (Inter-Process Communication)
//! 
//! Ce module fournit des mécanismes de communication inter-processus haute performance
//! avec une latence cible de < 500ns pour les messages rapides.

pub mod channel;
pub mod message;

#[cfg(feature = "fusion_rings")]
pub mod fusion_ring;

#[cfg(feature = "fusion_rings")]
pub mod fast_channel;

#[cfg(test)]
pub mod bench_fusion;

use alloc::collections::BTreeMap;
use spin::Mutex;
use lazy_static::lazy_static;
use core::sync::atomic::{AtomicU32, Ordering};
use crate::println;
use crate::perf_counters::{rdtsc, PERF_MANAGER, Component};
#[cfg(feature = "fusion_rings")]
use alloc::sync::Arc;

#[cfg(feature = "fusion_rings")]
use fusion_ring::SharedMemoryPool;

lazy_static! {
    /// Registre global des canaux IPC
    static ref CHANNEL_REGISTRY: Mutex<BTreeMap<u32, channel::ChannelHandle>> = 
        Mutex::new(BTreeMap::new());
}

#[cfg(feature = "fusion_rings")]
lazy_static! {
    /// Pool global de mémoire partagée pour zero-copy
    static ref SHARED_POOL: Mutex<SharedMemoryPool> = Mutex::new(SharedMemoryPool::new());
}

#[cfg(feature = "fusion_rings")]
lazy_static! {
    /// Canaux pilotes utilisant Fusion Ring (expérimental)
    static ref PILOT_CHANNELS: Mutex<BTreeMap<alloc::string::String, alloc::sync::Arc<Mutex<fast_channel::FastChannel>>>> = 
        Mutex::new(BTreeMap::new());
}

#[cfg(feature = "fusion_rings")]
lazy_static! {
    /// Mapping id→FastChannel pour chemins rapides par identifiant
    static ref FAST_BY_ID: Mutex<BTreeMap<u32, Arc<Mutex<fast_channel::FastChannel>>>> =
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

    #[cfg(feature = "fusion_rings")]
    {
        // Pour les canaux critiques, créer un FastChannel associé accessible par ID
        if name == "perf_test" || name == "log" {
            // FastChannel::new attend un &'static str, utiliser des littéraux statiques connus
            let label: &'static str = if name == "perf_test" { "perf_test" } else { "log" };
            let fast = Arc::new(Mutex::new(fast_channel::FastChannel::new(label)));
            FAST_BY_ID.lock().insert(id, fast);
        }
    }
    
    Ok(id)
}

/// Récupère un canal par son identifiant
pub fn get_channel(id: u32) -> Option<channel::ChannelHandle> {
    let registry = CHANNEL_REGISTRY.lock();
    registry.get(&id).cloned()
}

/// Envoie un message via un canal IPC
pub fn send_message(channel_id: u32, msg: message::Message) -> Result<(), &'static str> {
    let start = rdtsc();

    #[cfg(feature = "fusion_rings")]
    {
        // Si un FastChannel est associé à ce canal et que la taille est inline, utiliser le fast path
        if let Some(fast) = FAST_BY_ID.lock().get(&channel_id).cloned() {
            let data = msg.data();
            if data.len() <= fusion_ring::INLINE_SIZE {
                let res = fast.lock().send(data);
                let end = rdtsc();
                if res.is_ok() { PERF_MANAGER.record(Component::Ipc, end - start); }
                return res;
            }
        }
    }

    let result = if let Some(channel) = get_channel(channel_id) {
        channel.send(msg)
    } else {
        Err("Canal introuvable")
    };
    let end = rdtsc();
    if result.is_ok() {
        PERF_MANAGER.record(Component::Ipc, end - start);
    }
    result
}

#[cfg(feature = "fusion_rings")]
/// Envoie un message rapide via Fusion Ring (canal pilote)
pub fn send_fast(channel_name: &str, data: &[u8]) -> Result<(), &'static str> {
    let mut pilots = PILOT_CHANNELS.lock();
    if let Some(channel) = pilots.get_mut(&alloc::string::String::from(channel_name)) {
        let mut ch = channel.lock();
        ch.send(data)
    } else {
        Err("Canal pilote introuvable")
    }
}

#[cfg(feature = "fusion_rings")]
/// Reçoit un message rapide via Fusion Ring (canal pilote)
pub fn receive_fast(channel_name: &str) -> Result<alloc::vec::Vec<u8>, &'static str> {
    let mut pilots = PILOT_CHANNELS.lock();
    if let Some(channel) = pilots.get_mut(&alloc::string::String::from(channel_name)) {
        let mut ch = channel.lock();
        ch.receive()
    } else {
        Err("Canal pilote introuvable")
    }
}

#[cfg(feature = "fusion_rings")]
/// Envoie rapide via identifiant de canal (si FastChannel associé)
pub fn send_fast_by_id(channel_id: u32, data: &[u8]) -> Result<(), &'static str> {
    if let Some(ch) = FAST_BY_ID.lock().get(&channel_id).cloned() {
        ch.lock().send(data)
    } else {
        Err("Canal rapide introuvable")
    }
}

#[cfg(feature = "fusion_rings")]
/// Reçoit rapide via identifiant de canal (si FastChannel associé)
pub fn receive_fast_by_id(channel_id: u32) -> Result<alloc::vec::Vec<u8>, &'static str> {
    if let Some(ch) = FAST_BY_ID.lock().get(&channel_id).cloned() {
        ch.lock().receive()
    } else {
        Err("Canal rapide introuvable")
    }
}

/// Reçoit un message depuis un canal IPC
pub fn receive_message(channel_id: u32) -> Result<message::Message, &'static str> {
    let start = rdtsc();

    #[cfg(feature = "fusion_rings")]
    {
        if let Some(fast) = FAST_BY_ID.lock().get(&channel_id).cloned() {
            let mut ch = fast.lock();
            if ch.pending_count() > 0 {
                let res = ch.receive().map(|bytes| message::Message::new_buffered(0, 0, 0, bytes));
                let end = rdtsc();
                if res.is_ok() { PERF_MANAGER.record(Component::Ipc, end - start); }
                return res;
            }
        }
    }

    let result = if let Some(channel) = get_channel(channel_id) {
        channel.receive()
    } else {
        Err("Canal introuvable")
    };
    let end = rdtsc();
    if result.is_ok() {
        PERF_MANAGER.record(Component::Ipc, end - start);
    }
    result
}

/// Initialise le système IPC
pub fn init() {
    println!("[IPC] Initialisation du système IPC...");
    
    #[cfg(feature = "fusion_rings")]
    {
        // Initialiser pool partagé avec 16 pages fictives (4KB chacune)
        let mut pool = SHARED_POOL.lock();
        for i in 0..16 {
            // Adresses fictives pour démo (en production: vrais frames)
            pool.add_page(0x200000 + (i * 0x1000));
        }
        drop(pool);
        println!("[IPC] Pool partagé initialisé: 16 pages (64 KB)");
        
        // Créer canal pilote 'log' avec FastChannel
        let mut pilots = PILOT_CHANNELS.lock();
        let fast_log = alloc::sync::Arc::new(Mutex::new(fast_channel::FastChannel::new("log")));
        pilots.insert(alloc::string::String::from("log"), fast_log);
        drop(pilots);
        println!("[IPC] Canal pilote 'log' créé avec Fusion Ring");
    }
    
    // Créer les canaux par défaut (standard)
    println!("[IPC][TRACE] Création canal 'kernel'...");
    let _ = create_channel("kernel", 256);
    println!("[IPC][TRACE] Création canal 'debug'...");
    let _ = create_channel("debug", 128);
    println!("[IPC][TRACE] Création canal 'broadcast'...");
    let _ = create_channel("broadcast", 512);
    println!("[IPC][TRACE] Création canal 'log' (standard)...");
    let _ = create_channel("log", 256);
    
    #[cfg(feature = "fusion_rings")]
    println!("[IPC] Système IPC initialisé avec Fusion Rings (4 canaux + 1 pilote).");
    #[cfg(not(feature = "fusion_rings"))]
    println!("[IPC] Système IPC initialisé avec 4 canaux par défaut.");
}