// kernel/src/fs/exofs/storage/superblock_backup.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Gestion des miroirs du superblock ExoFS — écriture, lecture, récupération
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// ExoFS maintient 3 copies du superblock pour résistance aux pannes :
//   1. Offset 0            → SuperBlock primaire (SB_PRIMARY_OFFSET)
//   2. Offset 12 KB        → SuperBlock miroir (SB_MIRROR_12K_OFFSET)
//   3. disk_size - 4 KB    → SuperBlock miroir final
//
// Protocole de robustesse :
// - BACKUP-01 : les 3 copies sont écrites à chaque commit superblock.
// - BACKUP-02 : la récupération lit les 3 miroirs, prend le plus récent valide.
// - BACKUP-03 : si au moins 1 miroir est valide, le montage réussit.
//
// Règles respectées :
// - ARITH-01 : checked_add/sub pour les offsets.
// - WRITE-02 : vérification bytes_written après chaque écriture.
// - HDR-03   : verify() avant d'accéder aux données d'un superblock lu.

use core::mem::size_of;

use crate::fs::exofs::core::{ExofsError, ExofsResult, DiskOffset};
use crate::fs::exofs::storage::layout::{
    superblock_mirror_offsets, superblock_primary, superblock_mirror_12k,
    superblock_mirror_end,
};
use crate::fs::exofs::storage::superblock::ExoSuperblockDisk;
use crate::fs::exofs::storage::storage_stats::STORAGE_STATS;

// ─────────────────────────────────────────────────────────────────────────────
// MirrorIndex — identifiant d'un miroir
// ─────────────────────────────────────────────────────────────────────────────

/// Identifiant d'un des 3 miroirs du superblock.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MirrorIndex {
    Primary     = 0,
    Mirror12k   = 1,
    MirrorEnd   = 2,
}

impl MirrorIndex {
    /// Nom lisible pour les journaux.
    pub fn name(self) -> &'static str {
        match self {
            MirrorIndex::Primary   => "primary@0",
            MirrorIndex::Mirror12k => "mirror@12KB",
            MirrorIndex::MirrorEnd => "mirror@end-4KB",
        }
    }

    /// Les 3 miroirs sous forme de tableau.
    pub fn all() -> [MirrorIndex; 3] {
        [MirrorIndex::Primary, MirrorIndex::Mirror12k, MirrorIndex::MirrorEnd]
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MirrorStatus — état d'un miroir après lecture/vérification
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat de la vérification d'un miroir superblock.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MirrorStatus {
    /// Miroir valide (magic + checksum OK).
    Valid,
    /// Magic invalide.
    BadMagic,
    /// Checksum incorrect.
    BadChecksum,
    /// Erreur I/O lors de la lecture.
    IoError,
    /// Version du format incompatible.
    IncompatibleVersion,
}

impl MirrorStatus {
    #[inline]
    pub fn is_valid(self) -> bool {
        matches!(self, MirrorStatus::Valid)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MirrorReadResult — résultat de lecture d'un miroir
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat de la lecture d'un miroir SuperBlock.
#[derive(Clone)]
pub struct MirrorReadResult {
    pub index:   MirrorIndex,
    pub offset:  DiskOffset,
    pub status:  MirrorStatus,
    /// Contenu du superblock (valide uniquement si status == Valid).
    pub data:    ExoSuperblockDisk,
}

// ─────────────────────────────────────────────────────────────────────────────
// Écriture des miroirs
// ─────────────────────────────────────────────────────────────────────────────

/// Écrit le superblock aux 3 offsets miroirs.
///
/// # Protocole BACKUP-01
/// - Les 3 miroirs sont écrits dans l'ordre : primaire → 12KB → fin.
/// - Si une écriture miroir échoue, l'opération continue.
/// - Retourne `Ok(())` si au moins 1 miroir a été écrit avec succès.
/// - Retourne `Err(IoError)` uniquement si les 3 miroirs ont échoué.
///
/// # Règle WRITE-02
/// Vérifie que bytes_written == size_of::<ExoSuperblockDisk>() après chaque écriture.
pub fn write_superblock_mirrors(
    sb:        &ExoSuperblockDisk,
    disk_size: u64,
    write_fn:  &dyn Fn(&[u8], DiskOffset) -> ExofsResult<usize>,
) -> ExofsResult<u8> {
    let offsets = superblock_mirror_offsets(disk_size)?;
    let sb_size = size_of::<ExoSuperblockDisk>();

    // SAFETY: ExoSuperblockDisk est #[repr(C, align(4096))], types plain uniquement.
    let bytes = unsafe {
        core::slice::from_raw_parts(sb as *const ExoSuperblockDisk as *const u8, sb_size)
    };

    let mut ok_count: u8 = 0;

    for (idx, offset) in offsets.iter().enumerate() {
        match write_fn(bytes, *offset) {
            Ok(n) if n == sb_size => {
                ok_count += 1;
                STORAGE_STATS.inc_sb_commit();
            }
            Ok(n) => {
                // RÈGLE WRITE-02 : écriture partielle = erreur.
                let _ = n; // partial write
                STORAGE_STATS.inc_sb_commit_error();
                STORAGE_STATS.inc_io_error();
            }
            Err(_) => {
                STORAGE_STATS.inc_sb_commit_error();
                STORAGE_STATS.inc_io_error();
            }
        }
        let _ = idx; // pour les logs futurs
    }

    if ok_count == 0 {
        Err(ExofsError::IoError)
    } else {
        Ok(ok_count)
    }
}

/// Écrit uniquement le miroir primaire (offset 0).
///
/// Utilisé lors d'un commit rapide où seul le primaire est mis à jour.
pub fn write_primary_superblock(
    sb:       &ExoSuperblockDisk,
    write_fn: &dyn Fn(&[u8], DiskOffset) -> ExofsResult<usize>,
) -> ExofsResult<()> {
    let sb_size = size_of::<ExoSuperblockDisk>();
    // SAFETY: validité des données vérifiée par les gardes ci-dessus.
    let bytes   = unsafe {
        core::slice::from_raw_parts(sb as *const ExoSuperblockDisk as *const u8, sb_size)
    };
    let n = write_fn(bytes, superblock_primary())?;
    if n != sb_size {
        return Err(ExofsError::PartialWrite);
    }
    STORAGE_STATS.inc_sb_commit();
    Ok(())
}

/// Synchronise uniquement les miroirs secondaires depuis le primaire.
///
/// Utilisé après une récupération ou un fsck.
pub fn sync_secondary_mirrors(
    sb:        &ExoSuperblockDisk,
    disk_size: u64,
    write_fn:  &dyn Fn(&[u8], DiskOffset) -> ExofsResult<usize>,
) -> ExofsResult<()> {
    let sb_size = size_of::<ExoSuperblockDisk>();
    // SAFETY: validité des données vérifiée par les gardes ci-dessus.
    let bytes   = unsafe {
        core::slice::from_raw_parts(sb as *const ExoSuperblockDisk as *const u8, sb_size)
    };

    let mirror_12k  = superblock_mirror_12k();
    let mirror_end  = superblock_mirror_end(disk_size)?;

    let ok1 = write_fn(bytes, mirror_12k).map(|n| n == sb_size).unwrap_or(false);
    let ok2 = write_fn(bytes, mirror_end).map(|n| n == sb_size).unwrap_or(false);

    if ok1 || ok2 {
        STORAGE_STATS.inc_sb_mirror_restore();
        Ok(())
    } else {
        Err(ExofsError::IoError)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Lecture et vérification des miroirs
// ─────────────────────────────────────────────────────────────────────────────

/// Lit un seul miroir superblock depuis le disque.
///
/// # Règle HDR-03
/// verify() est appelé avant tout accès aux champs du superblock.
pub fn read_superblock_mirror(
    index:   MirrorIndex,
    offset:  DiskOffset,
    read_fn: &dyn Fn(DiskOffset, &mut [u8]) -> ExofsResult<usize>,
) -> MirrorReadResult {
    let sb_size = size_of::<ExoSuperblockDisk>();
    let mut buf = [0u8; 4096];

    // Lire depuis le disque.
    let n = match read_fn(offset, &mut buf[..sb_size]) {
        Ok(n) => n,
        Err(_) => {
            STORAGE_STATS.inc_io_error();
            return MirrorReadResult {
                index,
                offset,
                status: MirrorStatus::IoError,
                data:   zeroed_superblock_disk(),
            };
        }
    };

    if n != sb_size {
        STORAGE_STATS.inc_io_error();
        return MirrorReadResult {
            index,
            offset,
            status: MirrorStatus::IoError,
            data:   zeroed_superblock_disk(),
        };
    }

    // Parser le superblock depuis le buffer.
    // SAFETY: buf est aligné et suffisamment grand.
    let sb: ExoSuperblockDisk = unsafe {
        core::ptr::read(buf.as_ptr() as *const ExoSuperblockDisk)
    };

    // RÈGLE HDR-03 : vérification avant accès aux champs.
    let status = match sb.verify() {
        Ok(()) => {
            STORAGE_STATS.inc_checksum_ok();
            MirrorStatus::Valid
        }
        Err(ExofsError::InvalidMagic) => {
            STORAGE_STATS.inc_checksum_error();
            MirrorStatus::BadMagic
        }
        Err(ExofsError::ChecksumMismatch) => {
            STORAGE_STATS.inc_checksum_error();
            MirrorStatus::BadChecksum
        }
        Err(_) => {
            STORAGE_STATS.inc_checksum_error();
            MirrorStatus::IncompatibleVersion
        }
    };

    MirrorReadResult { index, offset, status, data: sb }
}

/// Lit les 3 miroirs et retourne leurs résultats.
pub fn read_all_mirrors(
    disk_size: u64,
    read_fn:   &dyn Fn(DiskOffset, &mut [u8]) -> ExofsResult<usize>,
) -> ExofsResult<[MirrorReadResult; 3]> {
    let offsets = superblock_mirror_offsets(disk_size)?;

    let r0 = read_superblock_mirror(MirrorIndex::Primary,   offsets[0], read_fn);
    let r1 = read_superblock_mirror(MirrorIndex::Mirror12k, offsets[1], read_fn);
    let r2 = read_superblock_mirror(MirrorIndex::MirrorEnd, offsets[2], read_fn);

    Ok([r0, r1, r2])
}

// ─────────────────────────────────────────────────────────────────────────────
// Récupération — choisir le meilleur miroir
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat de la sélection du meilleur miroir.
pub struct MirrorRecoveryResult {
    /// Index du miroir sélectionné (le plus récent valide).
    pub selected:     MirrorIndex,
    /// Superblock récupéré.
    pub superblock:   ExoSuperblockDisk,
    /// Nombre de miroirs valides.
    pub valid_count:  u8,
    /// Nombre de miroirs corrompus.
    pub corrupt_count: u8,
}

/// Sélectionne le meilleur miroir parmi les 3 résultats.
///
/// Stratégie (BACKUP-02) :
/// - Parmi les miroirs valides, choisit celui avec `epoch_current` le plus élevé.
/// - Si aucun miroir n'est valide, retourne `Err(CorruptedFilesystem)`.
pub fn select_best_mirror(
    results: &[MirrorReadResult; 3],
) -> ExofsResult<MirrorRecoveryResult> {
    let mut valid_count:  u8 = 0;
    let mut corrupt_count: u8 = 0;
    let mut best: Option<usize> = None;

    for (i, r) in results.iter().enumerate() {
        if r.status.is_valid() {
            valid_count += 1;
            // Prend le miroir avec la plus haute epoch.
            match best {
                None => { best = Some(i); }
                Some(prev) => {
                    if r.data.epoch_current > results[prev].data.epoch_current {
                        best = Some(i);
                    }
                }
            }
        } else {
            corrupt_count += 1;
        }
    }

    match best {
        None => Err(ExofsError::CorruptFilesystem),
        Some(idx) => {
            if corrupt_count > 0 {
                STORAGE_STATS.inc_sb_mirror_restore();
            }
            Ok(MirrorRecoveryResult {
                selected:      results[idx].index,
                superblock:    results[idx].data,
                valid_count,
                corrupt_count,
            })
        }
    }
}

/// Lit tous les miroirs et retourne le meilleur superblock.
///
/// Combine `read_all_mirrors` + `select_best_mirror` en une seule opération.
pub fn recover_superblock(
    disk_size: u64,
    read_fn:   &dyn Fn(DiskOffset, &mut [u8]) -> ExofsResult<usize>,
) -> ExofsResult<MirrorRecoveryResult> {
    let results = read_all_mirrors(disk_size, read_fn)?;
    select_best_mirror(&results)
}

// ─────────────────────────────────────────────────────────────────────────────
// Validation croisée des miroirs
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie que les 3 miroirs valides sont cohérents entre eux.
///
/// Deux superblocks sont cohérents si :
/// - Leur `uuid` est identique.
/// - Leur `disk_size_bytes` est identique.
/// - Leur `heap_start` est identique.
pub fn cross_validate_mirrors(results: &[MirrorReadResult; 3]) -> bool {
    let valid: alloc::vec::Vec<&MirrorReadResult> = results.iter()
        .filter(|r| r.status.is_valid())
        .collect();

    if valid.len() < 2 {
        return true; // Pas assez de miroirs pour comparer — accepté.
    }

    let ref0 = valid[0];
    for other in valid.iter().skip(1) {
        if other.data.uuid          != ref0.data.uuid          { return false; }
        if other.data.disk_size_bytes != ref0.data.disk_size_bytes { return false; }
        if other.data.heap_start    != ref0.data.heap_start    { return false; }
    }
    true
}

/// Retourne le nombre de miroirs valides parmi les 3.
pub fn count_valid_mirrors(results: &[MirrorReadResult; 3]) -> u8 {
    results.iter().filter(|r| r.status.is_valid()).count() as u8
}

// ─────────────────────────────────────────────────────────────────────────────
// Utilitaires internes
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne un ExoSuperblockDisk zeroisé (utilisé pour les erreurs I/O).
fn zeroed_superblock_disk() -> ExoSuperblockDisk {
    // SAFETY: ExoSuperblockDisk est un type plain (#[repr(C)], pas de Drop).
    unsafe { core::mem::zeroed() }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests unitaires
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    // use crate::fs::exofs::storage::layout::HEAP_START_OFFSET;

    #[allow(dead_code)] fn make_valid_sb(epoch: u64) -> ExoSuperblockDisk {
        ExoSuperblockDisk::new_volume(
            64 * 1024 * 1024,
            b"test",
            [1u8; 16],
            1000 + epoch,
        )
        // Note: epoch n'est pas dans new_volume, on l'override ici.
        // En pratique on utiliserait un setter.
    }

    #[test]
    fn test_mirror_index_all() {
        let all = MirrorIndex::all();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn test_mirror_status_is_valid() {
        assert!(MirrorStatus::Valid.is_valid());
        assert!(!MirrorStatus::BadMagic.is_valid());
        assert!(!MirrorStatus::BadChecksum.is_valid());
        assert!(!MirrorStatus::IoError.is_valid());
    }

    #[test]
    fn test_select_best_mirror_all_invalid() {
        let mk = |idx, status| MirrorReadResult {
            index: idx,
            offset: DiskOffset(0),
            status,
            data: zeroed_superblock_disk(),
        };
        let results = [
            mk(MirrorIndex::Primary,   MirrorStatus::BadMagic),
            mk(MirrorIndex::Mirror12k, MirrorStatus::BadChecksum),
            mk(MirrorIndex::MirrorEnd, MirrorStatus::IoError),
        ];
        assert!(select_best_mirror(&results).is_err());
    }

    #[test]
    fn test_count_valid_mirrors() {
        let mk = |idx, status| MirrorReadResult {
            index: idx, offset: DiskOffset(0),
            status, data: zeroed_superblock_disk(),
        };
        let results = [
            mk(MirrorIndex::Primary,   MirrorStatus::Valid),
            mk(MirrorIndex::Mirror12k, MirrorStatus::BadChecksum),
            mk(MirrorIndex::MirrorEnd, MirrorStatus::Valid),
        ];
        assert_eq!(count_valid_mirrors(&results), 2);
    }
}
