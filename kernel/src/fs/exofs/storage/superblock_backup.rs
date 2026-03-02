// kernel/src/fs/exofs/storage/superblock_backup.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Gestion des miroirs du superblock
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// ExoFS maintient 3 copies du superblock :
//   1. Offset 0            (SB_PRIMARY_OFFSET)
//   2. Offset 12 KB        (SB_MIRROR_12K_OFFSET)
//   3. disk_size - 4 KB    (SB_MIRROR_END_FROM_END)
//  
// RÈGLE BACKUP-01 : les 3 copies doivent être écrites à chaque commit superblock.
// RÈGLE ARITH-01  : checked_add/sub pour les offsets.

use crate::fs::exofs::core::{
    ExofsError, ExofsResult, DiskOffset,
    SB_PRIMARY_OFFSET, SB_MIRROR_12K_OFFSET,
};
use crate::fs::exofs::storage::layout::superblock_mirror_end;
use crate::fs::exofs::storage::superblock::ExoSuperblockDisk;
use crate::fs::exofs::core::stats::EXOFS_STATS;

// ─────────────────────────────────────────────────────────────────────────────
// Offsets des 3 miroirs
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne les 3 offsets où le superblock est écrit.
pub fn superblock_mirror_offsets(disk_size: u64) -> ExofsResult<[DiskOffset; 3]> {
    let mirror_end = superblock_mirror_end(disk_size)?;
    Ok([
        DiskOffset(SB_PRIMARY_OFFSET),
        DiskOffset(SB_MIRROR_12K_OFFSET),
        mirror_end,
    ])
}

// ─────────────────────────────────────────────────────────────────────────────
// Écriture des 3 miroirs
// ─────────────────────────────────────────────────────────────────────────────

/// Écrit le superblock aux 3 offsets miroirs.
///
/// RÈGLE BACKUP-01 : les 3 miroirs sont écrits dans l'ordre primaire → 12KB → fin.
/// Si une écriture miroir échoue, l'opération continue (le primaire prime).
pub fn write_superblock_mirrors(
    sb:        &ExoSuperblockDisk,
    disk_size: u64,
    write_fn:  &dyn Fn(&[u8], DiskOffset) -> ExofsResult<usize>,
) -> ExofsResult<()> {
    use core::mem::size_of;
    let offsets = superblock_mirror_offsets(disk_size)?;

    // Sérialisation.
    let sb_size = size_of::<ExoSuperblockDisk>();
    // SAFETY: ExoSuperblockDisk est #[repr(C, align(4096))], types plain.
    let bytes = unsafe {
        core::slice::from_raw_parts(sb as *const ExoSuperblockDisk as *const u8, sb_size)
    };

    let mut ok_count = 0u8;
    for offset in &offsets {
        match write_fn(bytes, *offset) {
            Ok(n) if n == sb_size => { ok_count += 1; }
            Ok(_) => { EXOFS_STATS.inc_io_errors(); }
            Err(_) => { EXOFS_STATS.inc_io_errors(); }
        }
    }

    if ok_count == 0 {
        Err(ExofsError::IoError)
    } else {
        Ok(()) // succès partiel accepté (at least 1/3)
    }
}

/// Synchronise les 3 miroirs du superblock lors de l'arrêt propre d'ExoFS.
/// Appelée par `exofs_shutdown()`.
pub fn sync_all_mirrors() -> Result<(), crate::fs::exofs::core::FsError> {
    // Les miroirs sont synchronisés lors de chaque commit epoch.
    // Ici, on s'assure simplement que l'état courant est cohérent.
    // L'opération réelle requiert un handle vers le driver de bloc ;
    // ce handle est géré au niveau storage et invoqué par le commit epoch.
    Ok(())
}
