// libs/exo_ipc/src/message.rs
use crate::MAX_INLINE_SIZE;
use core::fmt;
use core::mem::size_of;

/// Flags pour les messages IPC
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageFlags(u8);

impl MessageFlags {
    /// Pas de flags spéciaux
    pub const NONE: Self = Self(0);

    /// Message inline (contenu directement dans l'en-tête)
    pub const INLINE: Self = Self(1 << 0);

    /// Message zero-copy (contient un pointeur vers des données partagées)
    pub const ZERO_COPY: Self = Self(1 << 1);

    /// Réponse attendue
    pub const RESPONSE_EXPECTED: Self = Self(1 << 2);

    /// Message asynchrone (pas de garantie de livraison)
    pub const ASYNC: Self = Self(1 << 3);

    /// Nouvelle construction
    pub fn new() -> Self {
        Self::NONE
    }

    /// Ajoute un flag
    pub fn set(&mut self, flag: Self) {
        self.0 |= flag.0;
    }

    /// Supprime un flag
    pub fn unset(&mut self, flag: Self) {
        self.0 &= !flag.0;
    }

    /// Vérifie si un flag est défini
    pub fn contains(&self, flag: Self) -> bool {
        (self.0 & flag.0) != 0
    }
}

/// En-tête d'un message IPC
#[repr(C, align(64))]
#[derive(Clone, Copy, Debug)]
pub struct MessageHeader {
    /// Taille du message (en octets)
    pub size: u16,

    /// Flags du message
    pub flags: u8,

    /// Type de message (application-specific)
    pub msg_type: u8,

    /// ID de source (PID ou capability ID)
    pub source_id: u32,

    /// ID de destination (PID ou capability ID)
    pub dest_id: u32,

    /// ID de réponse (pour les réponses)
    pub reply_id: u64,

    /// Réservé pour alignement et extensions futures
    pub reserved: [u8; 40],
}

impl MessageHeader {
    /// Crée un nouvel en-tête
    pub fn new(size: usize, flags: MessageFlags, msg_type: u8) -> Self {
        Self {
            size: size as u16,
            flags: flags.0,
            msg_type,
            source_id: 0,
            dest_id: 0,
            reply_id: 0,
            reserved: [0; 40],
        }
    }

    /// Définit l'ID source
    pub fn set_source(&mut self, id: u32) {
        self.source_id = id;
    }

    /// Définit l'ID destination
    pub fn set_destination(&mut self, id: u32) {
        self.dest_id = id;
    }

    /// Définit l'ID de réponse
    pub fn set_reply_id(&mut self, id: u64) {
        self.reply_id = id;
    }
}

/// Message IPC complet
#[repr(C)]
pub struct Message {
    /// En-tête du message
    pub header: MessageHeader,

    /// Données du message (inline ou pointeur vers données externes)
    pub data: [u8; 56],
}

impl Message {
    /// Crée un nouveau message vide
    pub fn new() -> Self {
        Self {
            header: MessageHeader::new(0, MessageFlags::new(), 0),
            data: [0; 56],
        }
    }

    /// Crée un message à partir d'un objet serializable
    pub fn from_serializable<T: Serialize>(obj: &T) -> Result<Self, &'static str> {
        let mut msg = Self::new();
        let size = obj.serialize(&mut msg.data)?;

        msg.header = MessageHeader::new(size, MessageFlags::new(), 0);

        // Définir le flag INLINE si le message tient dans l'en-tête
        if size <= MAX_INLINE_SIZE {
            let mut flags = MessageFlags::new();
            flags.set(MessageFlags::INLINE);
            msg.header.flags = flags.0;
        }

        Ok(msg)
    }

    /// Vérifie si le message est inline
    pub fn is_inline(&self) -> bool {
        MessageFlags(self.header.flags).contains(MessageFlags::INLINE)
    }

    /// Vérifie si le message utilise zero-copy
    pub fn is_zero_copy(&self) -> bool {
        MessageFlags(self.header.flags).contains(MessageFlags::ZERO_COPY)
    }

    /// Récupère les données inline
    pub fn inline_data(&self) -> &[u8] {
        let size = self.header.size as usize;
        if size <= MAX_INLINE_SIZE {
            &self.data[..size]
        } else {
            &[]
        }
    }
}

/// Trait pour la sérialisation dans les messages
pub trait Serialize {
    /// Sérialise l'objet dans le buffer fourni
    fn serialize(&self, buffer: &mut [u8]) -> Result<usize, &'static str>;

    /// Désérialise l'objet depuis un buffer
    fn deserialize(buffer: &[u8]) -> Result<Self, &'static str>
    where
        Self: Sized;
}

impl fmt::Display for MessageHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "MessageHeader(size={}, flags={:#02x}, type={}, src={}, dest={}, reply={})",
            self.size, self.flags, self.msg_type, self.source_id, self.dest_id, self.reply_id
        )
    }
}

impl fmt::Debug for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Message")
            .field("header", &self.header)
            .field("is_inline", &self.is_inline())
            .field("is_zero_copy", &self.is_zero_copy())
            .field("size", &(self.header.size as usize))
            .finish()
    }
}
