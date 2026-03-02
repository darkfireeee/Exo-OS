//! exoar_format.rs — Format d'archive ExoAR (no_std, native ExoFS).
//!
//! Structure sur disque :
//!   ExoarHeader (96 bytes) — en-tête global
//!   [ ExoarEntryHeader (80 bytes) + payload bytes ] * N
//!   ExoarFooter (16 bytes) — magic de fin + CRC32 global

use core::mem::size_of;

pub const EXOAR_MAGIC:         u64 = 0x4558_4F41_525F_4152; // "EXOAR_AR"
pub const EXOAR_ENTRY_MAGIC:   u32 = 0x4558_454E; // "EXEN"
pub const EXOAR_FOOTER_MAGIC:  u32 = 0x4558_454F; // "EXEO"
pub const EXOAR_VERSION:       u16 = 1;

/// En-tête global d'archive ExoAR.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ExoarHeader {
    pub magic:       u64,       // EXOAR_MAGIC
    pub version:     u16,
    pub _pad:        [u8; 6],
    pub entry_count: u64,
    pub epoch_id:    u64,
    pub created_at:  u64,       // ticks
    pub archive_id:  [u8; 32],  // UUID/hash de l'archive
    pub flags:       u32,
    pub _pad2:       [u8; 20],
}

const _: () = assert!(size_of::<ExoarHeader>() == 96);

/// En-tête par entrée (blob).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ExoarEntryHeader {
    pub magic:       u32,      // EXOAR_ENTRY_MAGIC
    pub flags:       u8,       // bit0=compressed, bit1=encrypted
    pub _pad:        [u8; 3],
    pub blob_id:     [u8; 32],
    pub payload_len: u64,
    pub raw_len:     u64,      // Taille non-compressée.
    pub checksum:    u32,      // CRC32 du payload.
    pub _pad2:       [u8; 4],
}

const _: () = assert!(size_of::<ExoarEntryHeader>() == 80);

/// Pied de page d'archive.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ExoarFooter {
    pub magic:    u32, // EXOAR_FOOTER_MAGIC
    pub _pad:     [u8; 4],
    pub crc32:    u32, // CRC32 Castagnoli du contenu total.
    pub _pad2:    [u8; 4],
}

const _: () = assert!(size_of::<ExoarFooter>() == 16);

impl ExoarHeader {
    pub fn is_valid(&self) -> bool {
        self.magic == EXOAR_MAGIC && self.version == EXOAR_VERSION
    }
}
