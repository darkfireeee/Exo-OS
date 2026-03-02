// kernel/src/fs/exofs/storage/layout.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Layout — offsets disque fixes, conversion secteur/octet
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLE ARITH-01 : checked_add() OBLIGATOIRE pour TOUS les calculs d'offset.
// Violation = overflow u64 → écriture à l'offset 0 → superblock écrasé silencieusement.

use crate::fs::exofs::core::{DiskOffset, ExofsError, ExofsResult};
use crate::fs::exofs::core::constants::{
    SB_PRIMARY_OFFSET, EPOCH_SLOT_A_OFFSET, EPOCH_SLOT_B_OFFSET,
    EPOCH_SLOT_C_FROM_END, SB_MIRROR_12K_OFFSET, SB_MIRROR_END_FROM_END,
    HEAP_START_OFFSET, SUPERBLOCK_SIZE, EPOCH_SLOT_SIZE, BLOCK_SIZE,
};

// ─────────────────────────────────────────────────────────────────────────────
// Offsets statiques (indépendants de la taille du disque)
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne l'offset du SuperBlock primaire.
#[inline]
pub fn superblock_primary() -> DiskOffset {
    DiskOffset(SB_PRIMARY_OFFSET)
}

/// Retourne l'offset du Slot Epoch A (fixe : 4 KB).
#[inline]
pub fn epoch_slot_a() -> DiskOffset {
    DiskOffset(EPOCH_SLOT_A_OFFSET)
}

/// Retourne l'offset du Slot Epoch B (fixe : 8 KB).
#[inline]
pub fn epoch_slot_b() -> DiskOffset {
    DiskOffset(EPOCH_SLOT_B_OFFSET)
}

/// Retourne l'offset du SuperBlock miroir à 12 KB.
#[inline]
pub fn superblock_mirror_12k() -> DiskOffset {
    DiskOffset(SB_MIRROR_12K_OFFSET)
}

/// Retourne l'offset du début du heap général (1 MB).
#[inline]
pub fn heap_start() -> DiskOffset {
    DiskOffset(HEAP_START_OFFSET)
}

// ─────────────────────────────────────────────────────────────────────────────
// Offsets dépendants de la taille du disque
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne l'offset du Slot Epoch C (disk_size - 8 KB).
///
/// # Règle ARITH-01
/// Utilise checked_sub pour éviter l'underflow.
#[inline]
pub fn epoch_slot_c(disk_size_bytes: u64) -> ExofsResult<DiskOffset> {
    disk_size_bytes
        .checked_sub(EPOCH_SLOT_C_FROM_END)
        .ok_or(ExofsError::OffsetOverflow)
        .map(DiskOffset)
}

/// Retourne l'offset du SuperBlock miroir final (disk_size - 4 KB).
///
/// # Règle ARITH-01
#[inline]
pub fn superblock_mirror_end(disk_size_bytes: u64) -> ExofsResult<DiskOffset> {
    disk_size_bytes
        .checked_sub(SB_MIRROR_END_FROM_END)
        .ok_or(ExofsError::OffsetOverflow)
        .map(DiskOffset)
}

// ─────────────────────────────────────────────────────────────────────────────
// Conversions secteur / octet
// ─────────────────────────────────────────────────────────────────────────────

/// Convertit un offset en secteur 512 octets (arrondi bas).
///
/// # Règle ARITH-01
/// Pas d'overflow possible (division pure).
#[inline]
pub fn offset_to_sector_512(offset: DiskOffset) -> u64 {
    offset.0 / 512
}

/// Convertit un offset en numéro de bloc 4 KB (arrondi bas).
#[inline]
pub fn offset_to_block_4k(offset: DiskOffset) -> u64 {
    offset.0 / BLOCK_SIZE
}

/// Calcule l'offset de fin d'une zone {start, len} avec overflow check.
///
/// # Règle ARITH-01
#[inline]
pub fn zone_end(start: DiskOffset, len: u64) -> ExofsResult<DiskOffset> {
    start.0
        .checked_add(len)
        .ok_or(ExofsError::OffsetOverflow)
        .map(DiskOffset)
}

/// Vérifie qu'un {offset, len} est compris dans un disque de `disk_size` octets.
///
/// # Règle ARITH-01
pub fn check_bounds(offset: DiskOffset, len: u64, disk_size: u64) -> ExofsResult<()> {
    let end = offset.0
        .checked_add(len)
        .ok_or(ExofsError::OffsetOverflow)?;
    if end > disk_size {
        return Err(ExofsError::OffsetOverflow);
    }
    Ok(())
}

/// Calcule le déplacement de `n` blocs de 4 KB depuis un offset de base.
///
/// # Règle ARITH-01
#[inline]
pub fn blocks_to_offset(base: DiskOffset, n_blocks: u64) -> ExofsResult<DiskOffset> {
    n_blocks
        .checked_mul(BLOCK_SIZE)
        .and_then(|delta| base.0.checked_add(delta))
        .ok_or(ExofsError::OffsetOverflow)
        .map(DiskOffset)
}

/// Aligne un offset au prochain multiple de `align`.
///
/// `align` DOIT être une puissance de 2.
///
/// # Règle ARITH-01
#[inline]
pub fn align_up(offset: DiskOffset, align: u64) -> ExofsResult<DiskOffset> {
    debug_assert!(align.is_power_of_two(), "align must be power of 2");
    let mask = align - 1;
    let aligned = offset.0
        .checked_add(mask)
        .ok_or(ExofsError::OffsetOverflow)?
        & !mask;
    Ok(DiskOffset(aligned))
}
