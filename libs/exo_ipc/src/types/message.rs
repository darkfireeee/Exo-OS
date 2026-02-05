// libs/exo_ipc/src/types/message.rs
//! Système de messages IPC avec versioning et checksums

use core::fmt;
use core::mem;
use super::endpoint::{EndpointId, IpcAddress};

/// Version du protocole IPC
pub const PROTOCOL_VERSION: u16 = 1;

/// Taille maximale pour données inline (optimisé pour cache-line)
pub const MAX_INLINE_SIZE: usize = 48;

/// Taille totale d'un message (aligné sur cache-line)
pub const MESSAGE_SIZE: usize = 128;

/// Flags de message
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct MessageFlags(pub u16);

impl MessageFlags {
    /// Aucun flag
    pub const NONE: Self = Self(0);
    
    /// Message inline (données dans le message)
    pub const INLINE: Self = Self(1 << 0);
    
    /// Zero-copy (pointeur vers mémoire partagée)
    pub const ZERO_COPY: Self = Self(1 << 1);
    
    /// Réponse requise
    pub const REPLY_REQUIRED: Self = Self(1 << 2);
    
    /// Message asynchrone (best-effort)
    pub const ASYNC: Self = Self(1 << 3);
    
    /// Haute priorité
    pub const HIGH_PRIORITY: Self = Self(1 << 4);
    
    /// Message fragmenté (multi-part)
    pub const FRAGMENTED: Self = Self(1 << 5);
    
    /// Dernier fragment
    pub const LAST_FRAGMENT: Self = Self(1 << 6);
    
    /// Checksum présent
    pub const HAS_CHECKSUM: Self = Self(1 << 7);
    
    /// Message crypté
    pub const ENCRYPTED: Self = Self(1 << 8);
    
    /// Message compressé
    pub const COMPRESSED: Self = Self(1 << 9);
    
    /// Crée un nouvel ensemble de flags
    pub const fn new() -> Self {
        Self::NONE
    }
    
    /// Ajoute un flag
    pub const fn with(self, flag: Self) -> Self {
        Self(self.0 | flag.0)
    }
    
    /// Vérifie si un flag est présent
    pub const fn has(&self, flag: Self) -> bool {
        (self.0 & flag.0) != 0
    }
    
    /// Supprime un flag
    pub const fn without(self, flag: Self) -> Self {
        Self(self.0 & !flag.0)
    }
}

/// Type de message (application-specific)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum MessageType {
    /// Message de données générique
    Data = 0,
    
    /// Requête
    Request = 1,
    
    /// Réponse
    Response = 2,
    
    /// Notification
    Notification = 3,
    
    /// Erreur
    Error = 4,
    
    /// Handshake
    Handshake = 5,
    
    /// ACK
    Ack = 6,
    
    /// Ping
    Ping = 7,
    
    /// Pong
    Pong = 8,
    
    /// Custom (défini par l'application)
    Custom = 0xFF,
}

/// En-tête de message (aligné cache-line, 64 bytes)
#[repr(C, align(64))]
#[derive(Clone, Copy)]
pub struct MessageHeader {
    // === Identification (16 bytes) ===
    /// Version du protocole
    pub version: u16,
    
    /// Flags du message
    pub flags: MessageFlags,
    
    /// Type de message
    pub msg_type: MessageType,
    
    /// Taille totale des données (bytes)
    pub data_size: u16,
    
    /// ID de message unique
    pub message_id: u64,
    
    // === Adressage (16 bytes) ===
    /// Endpoint source
    pub source: EndpointId,
    
    /// Endpoint destination  
    pub destination: EndpointId,
    
    // === Metadata (16 bytes) ===
    /// ID de réponse (pour corréler requête/réponse)
    pub reply_id: u64,
    
    /// Timestamp (cycles CPU ou monotonic clock)
    pub timestamp: u64,
    
    // === Fragmentation & Intégrité (12 bytes) ===
    /// Index de fragment (pour messages multi-part)
    pub fragment_index: u16,
    
    /// Total de fragments
    pub fragment_total: u16,
    
    /// Checksum CRC32C
    pub checksum: u32,
    
    /// Numéro de séquence
    pub sequence: u32,
    
    // === Réservé pour extensions futures (4 bytes) ===
    pub reserved: [u8; 4],
}

// Vérification à la compilation de la taille
const _: () = assert!(mem::size_of::<MessageHeader>() == 64);

impl MessageHeader {
    /// Crée un nouvel en-tête
    pub const fn new(msg_type: MessageType) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            flags: MessageFlags::NONE,
            msg_type,
            data_size: 0,
            message_id: 0,
            source: EndpointId::INVALID,
            destination: EndpointId::INVALID,
            reply_id: 0,
            timestamp: 0,
            fragment_index: 0,
            fragment_total: 1,
            checksum: 0,
            sequence: 0,
            reserved: [0; 4],
        }
    }
    
    /// Configure l'adressage
    pub fn set_address(&mut self, addr: IpcAddress) {
        self.source = addr.source;
        self.destination = addr.destination;
        self.sequence = addr.sequence;
    }
    
    /// Récupère l'adresse
    pub fn address(&self) -> IpcAddress {
        IpcAddress {
            source: self.source,
            destination: self.destination,
            session_id: 0,
            sequence: self.sequence,
        }
    }
    
    /// Vérifie si c'est un message inline
    pub const fn is_inline(&self) -> bool {
        self.flags.has(MessageFlags::INLINE)
    }
    
    /// Vérifie si c'est zero-copy
    pub const fn is_zero_copy(&self) -> bool {
        self.flags.has(MessageFlags::ZERO_COPY)
    }
    
    /// Vérifie si le message est fragmenté
    pub const fn is_fragmented(&self) -> bool {
        self.flags.has(MessageFlags::FRAGMENTED)
    }
    
    /// Vérifie si c'est le dernier fragment
    pub const fn is_last_fragment(&self) -> bool {
        self.flags.has(MessageFlags::LAST_FRAGMENT)
    }
}

impl fmt::Debug for MessageHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MessageHeader")
            .field("version", &self.version)
            .field("flags", &format_args!("{:#06x}", self.flags.0))
            .field("msg_type", &self.msg_type)
            .field("data_size", &self.data_size)
            .field("message_id", &self.message_id)
            .field("source", &self.source)
            .field("destination", &self.destination)
            .field("reply_id", &self.reply_id)
            .field("sequence", &self.sequence)
            .finish()
    }
}

/// Payload du message
#[derive(Clone, Copy)]
pub union MessagePayload {
    /// Données inline (48 bytes)
    inline: [u8; MAX_INLINE_SIZE],
    
    /// Pointeur vers mémoire partagée (zero-copy)
    zero_copy: ZeroCopyPtr,
}

/// Pointeur zero-copy vers mémoire partagée
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ZeroCopyPtr {
    /// Adresse de la région partagée
    pub addr: u64,
    
    /// Taille de la région
    pub size: usize,
    
    /// ID de la région (pour validation)
    pub region_id: u64,
    
    /// Offset dans la région
    pub offset: usize,
    
    /// Padding
    _padding: [u8; 16],
}

const _: () = assert!(mem::size_of::<ZeroCopyPtr>() == MAX_INLINE_SIZE);

/// Message IPC complet (128 bytes, aligné cache-line)
#[repr(C, align(64))]
pub struct Message {
    /// En-tête (64 bytes)
    pub header: MessageHeader,
    
    /// Payload (64 bytes)
    payload: MessagePayload,
}

const _: () = assert!(mem::size_of::<Message>() == MESSAGE_SIZE);

impl Message {
    /// Crée un nouveau message vide
    pub fn new(msg_type: MessageType) -> Self {
        Self {
            header: MessageHeader::new(msg_type),
            payload: MessagePayload {
                inline: [0; MAX_INLINE_SIZE],
            },
        }
    }
    
    /// Crée un message inline avec données
    pub fn with_inline_data(data: &[u8], msg_type: MessageType) -> Result<Self, &'static str> {
        if data.len() > MAX_INLINE_SIZE {
            return Err("Données trop grandes pour message inline");
        }
        
        let mut msg = Self::new(msg_type);
        msg.header.data_size = data.len() as u16;
        msg.header.flags = msg.header.flags.with(MessageFlags::INLINE);
        
        unsafe {
            msg.payload.inline[..data.len()].copy_from_slice(data);
        }
        
        Ok(msg)
    }
    
    /// Crée un message zero-copy
    pub fn with_zero_copy(
        ptr: ZeroCopyPtr,
        msg_type: MessageType,
    ) -> Self {
        let mut msg = Self::new(msg_type);
        msg.header.data_size = ptr.size as u16;
        msg.header.flags = msg.header.flags.with(MessageFlags::ZERO_COPY);
        msg.payload = MessagePayload { zero_copy: ptr };
        msg
    }
    
    /// Récupère les données inline
    pub fn inline_data(&self) -> Option<&[u8]> {
        if self.header.is_inline() {
            let size = self.header.data_size as usize;
            if size <= MAX_INLINE_SIZE {
                Some(unsafe { &self.payload.inline[..size] })
            } else {
                None
            }
        } else {
            None
        }
    }
    
    /// Récupère les données inline mutables
    pub fn inline_data_mut(&mut self) -> Option<&mut [u8]> {
        if self.header.is_inline() {
            let size = self.header.data_size as usize;
            if size <= MAX_INLINE_SIZE {
                Some(unsafe { &mut self.payload.inline[..size] })
            } else {
                None
            }
        } else {
            None
        }
    }
    
    /// Récupère le pointeur zero-copy
    pub fn zero_copy_ptr(&self) -> Option<&ZeroCopyPtr> {
        if self.header.is_zero_copy() {
            Some(unsafe { &self.payload.zero_copy })
        } else {
            None
        }
    }
    
    /// Configure l'adresse complète
    pub fn set_address(&mut self, addr: IpcAddress) {
        self.header.set_address(addr);
    }
    
    /// Configure le message ID
    pub fn set_message_id(&mut self, id: u64) {
        self.header.message_id = id;
    }
    
    /// Configure le timestamp
    pub fn set_timestamp(&mut self, ts: u64) {
        self.header.timestamp = ts;
    }
    
    /// Calcule et définit le checksum
    pub fn compute_checksum(&mut self) {
        // Pour l'instant, checksum simple - sera remplacé par CRC32C
        self.header.checksum = 0;
        self.header.flags = self.header.flags.with(MessageFlags::HAS_CHECKSUM);
        
        // TODO: Implémenter CRC32C optimisé
    }
    
    /// Vérifie le checksum
    pub fn verify_checksum(&self) -> bool {
        if !self.header.flags.has(MessageFlags::HAS_CHECKSUM) {
            return true; // Pas de checksum à vérifier
        }
        
        // TODO: Implémenter vérification CRC32C
        true
    }
}

impl Clone for Message {
    fn clone(&self) -> Self {
        Self {
            header: self.header,
            payload: self.payload,
        }
    }
}

impl fmt::Debug for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Message")
            .field("header", &self.header)
            .field("is_inline", &self.header.is_inline())
            .field("is_zero_copy", &self.header.is_zero_copy())
            .finish()
    }
}

// Implémentations de sécurité
unsafe impl Send for Message {}
unsafe impl Sync for Message {}
