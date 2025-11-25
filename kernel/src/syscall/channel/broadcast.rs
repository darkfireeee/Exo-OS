//! Broadcast Channel Wrappers pour Syscalls IPC
//!
//! Fournit des canaux de diffusion (1-to-many) pour les syscalls

use crate::ipc::channel::broadcast::BroadcastChannel;
use crate::memory::MemoryResult;
use alloc::sync::Arc;
use alloc::vec::Vec;

/// Handle pour canal de broadcast utilisé dans les syscalls
pub struct SyscallBroadcastSender<T> {
    channel: Arc<BroadcastChannel<T>>,
}

impl<T: Clone> SyscallBroadcastSender<T> {
    /// Crée un nouveau canal broadcast
    pub fn new() -> Self {
        Self {
            channel: Arc::new(BroadcastChannel::new()),
        }
    }
    
    /// Diffuse un message à tous les abonnés
    pub fn broadcast(&self, msg: T) -> MemoryResult<usize> {
        let count = self.channel.broadcast(msg);
        Ok(count)
    }
    
    /// Nombre d'abonnés actifs
    pub fn subscriber_count(&self) -> usize {
        self.channel.subscriber_count()
    }
    
    /// Clone le sender
    pub fn clone_sender(&self) -> Self {
        Self {
            channel: Arc::clone(&self.channel),
        }
    }
}

/// Handle pour recevoir depuis un canal broadcast
pub struct SyscallBroadcastReceiver<T> {
    channel: Arc<BroadcastChannel<T>>,
    subscriber_id: usize,
}

impl<T: Clone> SyscallBroadcastReceiver<T> {
    /// Crée un receiver depuis un sender
    pub fn subscribe(sender: &SyscallBroadcastSender<T>) -> Self {
        let subscriber_id = sender.channel.subscribe();
        Self {
            channel: Arc::clone(&sender.channel),
            subscriber_id,
        }
    }
    
    /// Reçoit le prochain message broadcast
    pub fn recv(&self) -> MemoryResult<T> {
        self.channel.recv(self.subscriber_id)
    }
    
    /// Essaie de recevoir sans bloquer
    pub fn try_recv(&self) -> Option<T> {
        self.channel.try_recv(self.subscriber_id)
    }
}

impl<T> Drop for SyscallBroadcastReceiver<T> {
    fn drop(&mut self) {
        self.channel.unsubscribe(self.subscriber_id);
    }
}

/// Crée une paire sender/receiver pour broadcast
pub fn create_broadcast_channel<T: Clone>() -> (SyscallBroadcastSender<T>, SyscallBroadcastReceiver<T>) {
    let sender = SyscallBroadcastSender::new();
    let receiver = SyscallBroadcastReceiver::subscribe(&sender);
    (sender, receiver)
}

/// Canal broadcast avec filtrage
pub struct FilteredBroadcastChannel<T, F>
where
    F: Fn(&T) -> bool,
{
    channel: Arc<BroadcastChannel<T>>,
    filter: Arc<F>,
}

impl<T: Clone, F> FilteredBroadcastChannel<T, F>
where
    F: Fn(&T) -> bool,
{
    /// Crée un canal broadcast avec filtre
    pub fn new(filter: F) -> Self {
        Self {
            channel: Arc::new(BroadcastChannel::new()),
            filter: Arc::new(filter),
        }
    }
    
    /// Diffuse seulement si le filtre accepte
    pub fn broadcast_filtered(&self, msg: T) -> MemoryResult<usize> {
        if (self.filter)(&msg) {
            Ok(self.channel.broadcast(msg))
        } else {
            Ok(0) // Message filtré
        }
    }
}

/// Canal broadcast avec priorités
pub struct PriorityBroadcastChannel<T> {
    high_priority: Arc<BroadcastChannel<T>>,
    normal_priority: Arc<BroadcastChannel<T>>,
    low_priority: Arc<BroadcastChannel<T>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    High,
    Normal,
    Low,
}

impl<T: Clone> PriorityBroadcastChannel<T> {
    /// Crée un canal broadcast avec priorités
    pub fn new() -> Self {
        Self {
            high_priority: Arc::new(BroadcastChannel::new()),
            normal_priority: Arc::new(BroadcastChannel::new()),
            low_priority: Arc::new(BroadcastChannel::new()),
        }
    }
    
    /// Diffuse avec une priorité spécifique
    pub fn broadcast_priority(&self, msg: T, priority: Priority) -> MemoryResult<usize> {
        let count = match priority {
            Priority::High => self.high_priority.broadcast(msg),
            Priority::Normal => self.normal_priority.broadcast(msg),
            Priority::Low => self.low_priority.broadcast(msg),
        };
        Ok(count)
    }
    
    /// Souscrit à un niveau de priorité
    pub fn subscribe_priority(&self, priority: Priority) -> SyscallBroadcastReceiver<T> {
        let sender = match priority {
            Priority::High => SyscallBroadcastSender { channel: Arc::clone(&self.high_priority) },
            Priority::Normal => SyscallBroadcastSender { channel: Arc::clone(&self.normal_priority) },
            Priority::Low => SyscallBroadcastSender { channel: Arc::clone(&self.low_priority) },
        };
        SyscallBroadcastReceiver::subscribe(&sender)
    }
}

/// Groupe de canaux broadcast (pour multi-topic)
pub struct BroadcastGroup<T> {
    channels: Vec<Arc<BroadcastChannel<T>>>,
}

impl<T: Clone> BroadcastGroup<T> {
    /// Crée un nouveau groupe de broadcast
    pub fn new() -> Self {
        Self {
            channels: Vec::new(),
        }
    }
    
    /// Ajoute un nouveau canal au groupe
    pub fn add_channel(&mut self) -> usize {
        let channel = Arc::new(BroadcastChannel::new());
        self.channels.push(channel);
        self.channels.len() - 1
    }
    
    /// Diffuse sur un canal spécifique du groupe
    pub fn broadcast_to(&self, channel_id: usize, msg: T) -> MemoryResult<usize> {
        if let Some(channel) = self.channels.get(channel_id) {
            Ok(channel.broadcast(msg))
        } else {
            Err(crate::memory::MemoryError::NotFound)
        }
    }
    
    /// Diffuse sur tous les canaux du groupe
    pub fn broadcast_all(&self, msg: T) -> MemoryResult<usize> {
        let mut total = 0;
        for channel in &self.channels {
            total += channel.broadcast(msg.clone());
        }
        Ok(total)
    }
}
