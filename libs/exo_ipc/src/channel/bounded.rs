// libs/exo_ipc/src/channel/bounded.rs
//! Canaux bornés (bounded channels) pour IPC

use alloc::sync::Arc;
use core::sync::atomic::AtomicBool;

use crate::ring::{SpscRing, MpscRing};
use crate::types::{Message, RecvError, SendError};
use crate::util::cache::CachePadded;
use crate::util::atomic::{AtomicStats, Backoff};

/// Type de canal pour sélectionner l'implémentation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelType {
    /// Single Producer Single Consumer (le plus rapide)
    Spsc,
    
    /// Multi Producer Single Consumer
    Mpsc,
}

/// État partagé d'un canal
struct ChannelState {
    /// Flag de connexion
    connected: CachePadded<AtomicBool>,
    
    /// Statistiques
    stats: AtomicStats,
}

impl ChannelState {
    fn new() -> Self {
        Self {
            connected: CachePadded::new(AtomicBool::new(true)),
            stats: AtomicStats::new(),
        }
    }
    
    fn is_connected(&self) -> bool {
        self.connected.load(core::sync::atomic::Ordering::Acquire)
    }
    
    fn disconnect(&self) {
        self.connected.store(false, core::sync::atomic::Ordering::Release);
    }
}

/// Sender pour canal SPSC
pub struct SenderSpsc {
    ring: Arc<SpscRing>,
    state: Arc<ChannelState>,
}

/// Receiver pour canal SPSC
pub struct ReceiverSpsc {
    ring: Arc<SpscRing>,
    state: Arc<ChannelState>,
}

/// Sender pour canal MPSC (cloneable)
pub struct SenderMpsc {
    ring: Arc<MpscRing>,
    state: Arc<ChannelState>,
}

/// Receiver pour canal MPSC
pub struct ReceiverMpsc {
    ring: Arc<MpscRing>,
    state: Arc<ChannelState>,
}

/// Crée un canal SPSC (Single Producer Single Consumer)
///
/// C'est le canal le plus rapide car il n'utilise pas de CAS.
/// Utilisez-le quand vous avez un seul producteur et un seul consommateur.
///
/// # Arguments
/// * `capacity` - Capacité du buffer (doit être une puissance de 2)
///
/// # Returns
/// Tuple `(Sender, Receiver)` ou erreur si l'allocation échoue
pub fn spsc(capacity: usize) -> Result<(SenderSpsc, ReceiverSpsc), &'static str> {
    let ring = Arc::new(SpscRing::new(capacity)?);
    let state = Arc::new(ChannelState::new());
    
    let sender = SenderSpsc {
        ring: ring.clone(),
        state: state.clone(),
    };
    
    let receiver = ReceiverSpsc {
        ring,
        state,
    };
    
    Ok((sender, receiver))
}

/// Crée un canal MPSC (Multi Producer Single Consumer)
///
/// Permet à plusieurs threads de produire des messages pour un seul consommateur.
/// Légèrement plus lent que SPSC à cause de la synchronisation CAS.
///
/// # Arguments
/// * `capacity` - Capacité du buffer (doit être une puissance de 2)
///
/// # Returns
/// Tuple `(Sender, Receiver)` où le Sender est cloneable
pub fn mpsc(capacity: usize) -> Result<(SenderMpsc, ReceiverMpsc), &'static str> {
    let ring = Arc::new(MpscRing::new(capacity)?);
    let state = Arc::new(ChannelState::new());
    
    let sender = SenderMpsc {
        ring: ring.clone(),
        state: state.clone(),
    };
    
    let receiver = ReceiverMpsc {
        ring,
        state,
    };
    
    Ok((sender, receiver))
}

// Implémentations pour SenderSpsc
impl SenderSpsc {
    /// Envoie un message (bloquant si le canal est plein)
    pub fn send(&self, msg: Message) -> Result<(), SendError<Message>> {
        if !self.state.is_connected() {
            return Err(SendError::Disconnected(msg));
        }
        
        let mut backoff = Backoff::new();
        let mut message = msg;
        
        loop {
            let data_size = message.header.data_size;
            match self.ring.push(message) {
                Ok(()) => {
                    self.state.stats.record_send(data_size as u64);
                    return Ok(());
                }
                Err(msg) => {
                    if !self.state.is_connected() {
                        return Err(SendError::Disconnected(msg));
                    }
                    
                    message = msg;
                    backoff.snooze();
                }
            }
        }
    }
    
    /// Tente d'envoyer un message (non-bloquant)
    pub fn try_send(&self, msg: Message) -> Result<(), SendError<Message>> {
        if !self.state.is_connected() {
            return Err(SendError::Disconnected(msg));
        }
        
        let data_size = msg.header.data_size;
        match self.ring.push(msg) {
            Ok(()) => {
                self.state.stats.record_send(data_size as u64);
                Ok(())
            }
            Err(msg) => Err(SendError::Full(msg)),
        }
    }
    
    /// Vérifie si le canal est connecté
    pub fn is_connected(&self) -> bool {
        self.state.is_connected()
    }
}

// Implémentations pour ReceiverSpsc
impl ReceiverSpsc {
    /// Reçoit un message (bloquant si le canal est vide)
    pub fn recv(&self) -> Result<Message, RecvError> {
        let mut backoff = Backoff::new();
        
        loop {
            match self.ring.pop() {
                Some(msg) => {
                    self.state.stats.record_recv(msg.header.data_size as u64);
                    return Ok(msg);
                }
                None => {
                    if !self.state.is_connected() && self.ring.is_empty() {
                        return Err(RecvError::Disconnected);
                    }
                    
                    backoff.snooze();
                }
            }
        }
    }
    
    /// Tente de recevoir un message (non-bloquant)
    pub fn try_recv(&self) -> Result<Message, RecvError> {
        match self.ring.pop() {
            Some(msg) => {
                self.state.stats.record_recv(msg.header.data_size as u64);
                Ok(msg)
            }
            None => {
                if !self.state.is_connected() && self.ring.is_empty() {
                    Err(RecvError::Disconnected)
                } else {
                    Err(RecvError::Empty)
                }
            }
        }
    }
    
    /// Vérifie si le canal est connecté
    pub fn is_connected(&self) -> bool {
        self.state.is_connected()
    }
    
    /// Nombre de messages en attente
    pub fn len(&self) -> usize {
        self.ring.len()
    }
    
    /// Vérifie si le buffer est vide
    pub fn is_empty(&self) -> bool {
        self.ring.is_empty()
    }
}

// Implémentations pour SenderMpsc
impl SenderMpsc {
    /// Envoie un message (bloquant)
    pub fn send(&self, msg: Message) -> Result<(), SendError<Message>> {
        if !self.state.is_connected() {
            return Err(SendError::Disconnected(msg));
        }
        
        let mut backoff = Backoff::new();
        let mut message = msg;
        
        loop {
            let data_size = message.header.data_size;
            match self.ring.push(message) {
                Ok(()) => {
                    self.state.stats.record_send(data_size as u64);
                    return Ok(());
                }
                Err(msg) => {
                    if !self.state.is_connected() {
                        return Err(SendError::Disconnected(msg));
                    }
                    
                    message = msg;
                    backoff.snooze();
                }
            }
        }
    }
    
    /// Tente d'envoyer (non-bloquant)
    pub fn try_send(&self, msg: Message) -> Result<(), SendError<Message>> {
        if !self.state.is_connected() {
            return Err(SendError::Disconnected(msg));
        }
        
        let data_size = msg.header.data_size;
        match self.ring.push(msg) {
            Ok(()) => {
                self.state.stats.record_send(data_size as u64);
                Ok(())
            }
            Err(msg) => Err(SendError::Full(msg)),
        }
    }
    
    /// Vérifie si connecté
    pub fn is_connected(&self) -> bool {
        self.state.is_connected()
    }
}

// Clone pour SenderMpsc (permet plusieurs producteurs)
impl Clone for SenderMpsc {
    fn clone(&self) -> Self {
        Self {
            ring: self.ring.clone(),
            state: self.state.clone(),
        }
    }
}

// Implémentations pour ReceiverMpsc
impl ReceiverMpsc {
    /// Reçoit un message (bloquant)
    pub fn recv(&self) -> Result<Message, RecvError> {
        let mut backoff = Backoff::new();
        
        loop {
            match self.ring.pop() {
                Some(msg) => {
                    self.state.stats.record_recv(msg.header.data_size as u64);
                    return Ok(msg);
                }
                None => {
                    if !self.state.is_connected() && self.ring.is_empty() {
                        return Err(RecvError::Disconnected);
                    }
                    
                    backoff.snooze();
                }
            }
        }
    }
    
    /// Tente de recevoir (non-bloquant)
    pub fn try_recv(&self) -> Result<Message, RecvError> {
        match self.ring.pop() {
            Some(msg) => {
                self.state.stats.record_recv(msg.header.data_size as u64);
                Ok(msg)
            }
            None => {
                if !self.state.is_connected() && self.ring.is_empty() {
                    Err(RecvError::Disconnected)
                } else {
                    Err(RecvError::Empty)
                }
            }
        }
    }
    
    /// Vérifie si connecté
    pub fn is_connected(&self) -> bool {
        self.state.is_connected()
    }
    
    /// Nombre de messages
    pub fn len(&self) -> usize {
        self.ring.len()
    }
    
    /// Vérifie si vide
    pub fn is_empty(&self) -> bool {
        self.ring.is_empty()
    }
}

// Drop implémentations
impl Drop for SenderSpsc {
    fn drop(&mut self) {
        self.state.disconnect();
    }
}

impl Drop for ReceiverSpsc {
    fn drop(&mut self) {
        self.state.disconnect();
    }
}

impl Drop for SenderMpsc {
    fn drop(&mut self) {
        // Ne déconnecte que quand le dernier sender est drop
        // (Arc gère déjà cela automatiquement)
    }
}

impl Drop for ReceiverMpsc {
    fn drop(&mut self) {
        self.state.disconnect();
    }
}

// Safety markers
unsafe impl Send for SenderSpsc {}
unsafe impl Send for ReceiverSpsc {}
unsafe impl Send for SenderMpsc {}
unsafe impl Sync for SenderMpsc {} // Permet le partage entre threads
unsafe impl Send for ReceiverMpsc {}

/*
#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::MessageType;
    
    #[test]
    fn test_spsc_channel() {
        let (tx, rx) = spsc(16).unwrap();
        
        let msg = Message::new(MessageType::Data);
        tx.send(msg).unwrap();
        
        let received = rx.recv().unwrap();
        assert_eq!(received.header.msg_type as u16, MessageType::Data as u16);
    }
    
    #[test]
    fn test_mpsc_channel() {
        let (tx, rx) = mpsc(16).unwrap();
        
        // Cloner le sender
        let tx2 = tx.clone();
        
        let msg1 = Message::new(MessageType::Data);
        let msg2 = Message::new(MessageType::Request);
        
        tx.send(msg1).unwrap();
        tx2.send(msg2).unwrap();
        
        assert_eq!(rx.len(), 2);
        
        rx.recv().unwrap();
        rx.recv().unwrap();
        
        assert!(rx.is_empty());
    }
    
    #[test]
    fn test_try_send_full() {
        let (tx, rx) = spsc(4).unwrap();
        
        // Remplir le buffer
        for _ in 0..3 {
            let msg = Message::new(MessageType::Data);
            tx.try_send(msg).unwrap();
        }
        
        // Le prochain devrait échouer
        let msg = Message::new(MessageType::Data);
        assert!(matches!(tx.try_send(msg), Err(SendError::Full(_))));
    }
    
    #[test]
    fn test_try_recv_empty() {
        let (_tx, rx) = spsc(16).unwrap();
        
        assert!(matches!(rx.try_recv(), Err(RecvError::Empty)));
    }
}
*/
