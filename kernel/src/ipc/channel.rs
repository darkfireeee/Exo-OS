//! # Canaux de communication IPC
//! 
//! Ce module implémente des canaux de communication lock-free pour l'IPC,
//! utilisant des queues MPSC (Multiple Producer, Single Consumer) pour une haute performance.

use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};
use crossbeam_queue::SegQueue;
use spin::Mutex;
use crate::ipc::message::Message;

/// Handle vers un canal IPC
#[derive(Debug, Clone)]
pub struct ChannelHandle {
    /// Identifiant unique du canal
    pub id: u32,
    /// Canal partagé
    channel: Arc<Mutex<Channel>>,
}

impl ChannelHandle {
    /// Crée un nouveau handle de canal
    pub fn new(id: u32, channel: Channel) -> Self {
        Self {
            id,
            channel: Arc::new(Mutex::new(channel)),
        }
    }
    
    /// Envoie un message via le canal
    pub fn send(&self, msg: Message) -> Result<(), &'static str> {
        let mut channel = self.channel.lock();
        channel.send(msg)
    }
    
    /// Reçoit un message depuis le canal
    pub fn receive(&self) -> Result<Message, &'static str> {
        let mut channel = self.channel.lock();
        channel.receive()
    }
    
    /// Retourne le nombre de messages en attente dans le canal
    pub fn pending_count(&self) -> usize {
        let channel = self.channel.lock();
        channel.pending_count()
    }
}

/// Canal de communication IPC
#[derive(Debug)]
pub struct Channel {
    /// Nom du canal (pour le debug)
    name: alloc::string::String,
    /// Queue de messages lock-free
    queue: SegQueue<Message>,
    /// Nombre de messages envoyés
    sent_count: AtomicUsize,
    /// Nombre de messages reçus
    received_count: AtomicUsize,
    /// Taille maximale du buffer (0 = illimité)
    max_size: usize,
}

impl Channel {
    /// Crée un nouveau canal
    pub fn new(name: &str, max_size: usize) -> Self {
        Self {
            name: alloc::string::String::from(name),
            queue: SegQueue::new(),
            sent_count: AtomicUsize::new(0),
            received_count: AtomicUsize::new(0),
            max_size,
        }
    }
    
    /// Envoie un message dans le canal
    pub fn send(&mut self, msg: Message) -> Result<(), &'static str> {
        // Vérifier si le canal est plein
        if self.max_size > 0 && self.pending_count() >= self.max_size {
            return Err("Canal plein");
        }
        
        self.queue.push(msg);
        self.sent_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    
    /// Reçoit un message depuis le canal
    pub fn receive(&mut self) -> Result<Message, &'static str> {
        match self.queue.pop() {
            Some(msg) => {
                self.received_count.fetch_add(1, Ordering::SeqCst);
                Ok(msg)
            }
            None => Err("Aucun message en attente"),
        }
    }
    
    /// Retourne le nombre de messages en attente
    pub fn pending_count(&self) -> usize {
        self.queue.len()
    }
    
    /// Retourne le nombre total de messages envoyés
    pub fn sent_count(&self) -> usize {
        self.sent_count.load(Ordering::SeqCst)
    }
    
    /// Retourne le nombre total de messages reçus
    pub fn received_count(&self) -> usize {
        self.received_count.load(Ordering::SeqCst)
    }
    
    /// Vide le canal
    pub fn clear(&mut self) {
        while self.queue.pop().is_some() {
            // On ignore les messages
        }
    }
}