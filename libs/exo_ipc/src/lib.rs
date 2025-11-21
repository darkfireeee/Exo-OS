#![no_std]

extern crate alloc;

pub mod channel;
pub mod message;

// Réexportations
pub use channel::{Channel, Receiver, Sender, TryRecvError, TrySendError};
pub use message::{Message, MessageFlags, MessageHeader};

/// Taille maximale pour un message inline
pub const MAX_INLINE_SIZE: usize = 56;

/// Taille d'une slot dans le ring buffer
pub const SLOT_SIZE: usize = 64;

/// Alignement des slots pour l'accès cache-friendly
pub const SLOT_ALIGN: usize = 64;
