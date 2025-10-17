//! # Structure des messages IPC
//! 
//! Ce module définit les structures de messages utilisées pour la communication
//! inter-processus, avec une optimisation pour les messages rapides via registres.

use alloc::vec::Vec;
use core::mem;

/// Taille maximale d'un message rapide (qui peut passer par registres)
pub const FAST_MESSAGE_SIZE: usize = 8 * mem::size_of::<usize>(); // 8 registres sur x86_64

/// Structure de message IPC optimisée pour différentes tailles
#[derive(Debug, Clone)]
pub struct Message {
    /// Type de message
    pub msg_type: MessageType,
    /// Identifiant du processus émetteur
    pub sender_id: u32,
    /// Identifiant du processus destinataire
    pub receiver_id: u32,
    /// Code du message (pour identifier le type de requête)
    pub code: u32,
    /// Données du message
    pub data: MessageData,
}

/// Types de messages IPC
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MessageType {
    /// Message rapide via registres (≤ 64 octets)
    Fast,
    /// Message plus grand nécessitant une copie en mémoire
    Buffered,
    /// Message avec descripteurs de fichiers ou handles
    WithHandles,
}

/// Données du message, optimisées pour différents cas d'usage
#[derive(Debug, Clone)]
pub enum MessageData {
    /// Message rapide (≤ 64 octets) stocké directement dans la structure
    Fast([u8; FAST_MESSAGE_SIZE]),
    /// Message plus grand stocké dans le tas
    Buffered(Vec<u8>),
    /// Message avec descripteurs
    WithHandles {
        /// Données du message
        data: Vec<u8>,
        /// Descripteurs de fichiers ou handles
        handles: Vec<u32>,
    },
}

impl Message {
    /// Crée un nouveau message rapide
    pub fn new_fast(sender_id: u32, receiver_id: u32, code: u32, data: &[u8]) -> Self {
        let mut fast_data = [0; FAST_MESSAGE_SIZE];
        let copy_len = core::cmp::min(data.len(), FAST_MESSAGE_SIZE);
        fast_data[..copy_len].copy_from_slice(&data[..copy_len]);
        
        Self {
            msg_type: MessageType::Fast,
            sender_id,
            receiver_id,
            code,
            data: MessageData::Fast(fast_data),
        }
    }
    
    /// Crée un nouveau message bufferisé
    pub fn new_buffered(sender_id: u32, receiver_id: u32, code: u32, data: Vec<u8>) -> Self {
        Self {
            msg_type: MessageType::Buffered,
            sender_id,
            receiver_id,
            code,
            data: MessageData::Buffered(data),
        }
    }
    
    /// Crée un nouveau message avec descripteurs
    pub fn new_with_handles(
        sender_id: u32, 
        receiver_id: u32, 
        code: u32, 
        data: Vec<u8>,
        handles: Vec<u32>
    ) -> Self {
        Self {
            msg_type: MessageType::WithHandles,
            sender_id,
            receiver_id,
            code,
            data: MessageData::WithHandles { data, handles },
        }
    }
    
    /// Retourne les données du message sous forme de slice
    pub fn data(&self) -> &[u8] {
        match &self.data {
            MessageData::Fast(data) => data,
            MessageData::Buffered(data) => data,
            MessageData::WithHandles { data, .. } => data,
        }
    }
    
    /// Retourne les descripteurs du message (s'il en a)
    pub fn handles(&self) -> Option<&[u32]> {
        match &self.data {
            MessageData::WithHandles { handles, .. } => Some(handles),
            _ => None,
        }
    }
    
    /// Retourne la taille du message en octets
    pub fn size(&self) -> usize {
        match &self.data {
            MessageData::Fast(_) => FAST_MESSAGE_SIZE,
            MessageData::Buffered(data) => data.len(),
            MessageData::WithHandles { data, .. } => data.len(),
        }
    }
}