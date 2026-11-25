// drivers/fs/src/lib.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// exo-os-driver-fs — Pilotes FS tiers  (Exo-OS · crate séparé)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Ce crate contient les pilotes pour les systèmes de fichiers TIERS :
//   • ext4  — lecture/écriture de disques Linux ext4 standard
//   • fat32 — lecture/écriture de clés USB FAT32
//
// ISOLATION :
//   Ce crate est TOTALEMENT INDÉPENDANT du kernel.
//   Il ne dépend pas de crate::fs::*, crate::memory::*, etc.
//   Il définit ses propres types d'erreur et structures de données.
//   Le kernel l'appelle via le registre VFS (FsTypeRegistry).
//
// DOC6 FS/ v2 — règles FS-EXT4-01..04 et FS-FAT32-01..05.
// ═══════════════════════════════════════════════════════════════════════════════

#![no_std]
#![allow(dead_code)]

extern crate alloc;

/// Pilote ext4 classique (format Linux standard — pas ext4plus).
pub mod ext4;

/// Pilote FAT32 (clés USB, échange universel Windows/Linux/Mac).
pub mod fat32;

// ─────────────────────────────────────────────────────────────────────────────
// Types d'erreur partagés
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsDriverError {
    /// Mauvaise signature de format (magic invalide).
    BadSignature,
    /// Ce n'est pas le bon type de FS (FAT12/FAT16 au lieu de FAT32, etc.).
    WrongFsType,
    /// Ce disque est ext4plus — utiliser le driver ext4plus du kernel.
    IsExt4Plus,
    /// Flags INCOMPAT inconnus — refus de monter pour protéger les données.
    UnknownIncompatFlags { flags: u32 },
    /// Journal non propre — monter en lecture seule seulement.
    JournalNeedsRecovery,
    /// Erreur I/O lors de la lecture.
    IoError,
    /// Paramètre invalide (BPB corrompu, offset hors limites, etc.).
    InvalidParameter,
    /// Plus d'espace disponible.
    NoSpace,
    /// Pas de mémoire disponible.
    OutOfMemory,
    /// Fichier/répertoire non trouvé.
    NotFound,
}

pub type DriverResult<T> = Result<T, FsDriverError>;
