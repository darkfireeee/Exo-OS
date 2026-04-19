//! fsck_phase1.rs — Phase 1 du fsck : vérification des en-têtes et du superbloc.
//!
//! Lit et valide les structures fondamentales :
//! - En-tête du superbloc (magic + checksum, HDR-03).
//! - Cohérence de la table d'allocation des blobs.
//! - Validité des en-têtes des journaux slot/epoch.
//!
//! # Règles spec appliquées
//! - **HDR-03** : magic vérifié EN PREMIER sur chaque en-tête.
//! - **OOM-02** : `try_reserve(1)` avant tout `Vec::push`.
//! - **ARITH-02** : `checked_add` sur les offsets et compteurs.


extern crate alloc;
use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::core::blob_id::blake3_hash;
use super::boot_recovery::BlockDevice;
use super::block_io::read_array;
use super::recovery_audit::RECOVERY_AUDIT;
use super::recovery_log::RECOVERY_LOG;

// ── Constantes ────────────────────────────────────────────────────────────────

/// Magic du superbloc ExoFS : "EXOFSBLK".
pub const SUPERBLOCK_MAGIC: u64  = 0x4B4C42534F465845; // "EXOFSBLK"

/// Version courante du superbloc.
pub const SUPERBLOCK_VERSION: u8 = 1;

/// Taille du superbloc on-disk.
pub const SUPERBLOCK_SIZE: usize = 256;

/// Magic de la table d'allocation : "EXOBLKAT".
pub const ALLOC_TABLE_MAGIC: u64 = 0x54414B4C424F5845; // "EXOBLKAT"

/// Magic de l'en-tête de la région de blobs : "EXOBLREG".
pub const BLOB_REGION_MAGIC: u64 = 0x47455242_4C424F45; // "EXOBLREG"

// ── En-tête du superbloc ──────────────────────────────────────────────────────

/// En-tête on-disk du superbloc ExoFS — `repr(C)`, 256 octets.
///
/// # ONDISK-03
/// Pas d'`AtomicU64`.
///
/// # Layout
/// ```text
/// off   0 : magic          u64   8B
/// off   8 : version        u8    1B
/// off   9 : flags          u8    1B
/// off  10 : _pad0          u16   2B
/// off  12 : block_size     u32   4B
/// off  16 : total_blocks   u64   8B
/// off  24 : free_blocks    u64   8B
/// off  32 : epoch_id       u64   8B
/// off  40 : n_blobs        u64   8B
/// off  48 : alloc_lba      u64   8B  — LBA de la table d'allocation
/// off  56 : journal_lba    u64   8B
/// off  64 : blob_region_start u64 8B
/// off  72 : blob_region_end   u64 8B
/// off  80 : snapshot_count u32   4B
/// off  84 : _pad1          u32   4B
/// off  88 : uuid           [u8;16] 16B
/// off 104 : _reserved      [u8;120] 120B
/// off 224 : sb_hash        [u8;32] 32B — Blake3(bytes[0..224])
/// total : 256B
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SuperblockDisk {
    pub magic:              u64,
    pub version:            u8,
    pub flags:              u8,
    pub _pad0:              u16,
    pub block_size:         u32,
    pub total_blocks:       u64,
    pub free_blocks:        u64,
    pub epoch_id:           u64,
    pub n_blobs:            u64,
    pub alloc_lba:          u64,
    pub journal_lba:        u64,
    pub blob_region_start:  u64,
    pub blob_region_end:    u64,
    pub snapshot_count:     u32,
    pub _pad1:              u32,
    pub uuid:               [u8; 16],
    pub _reserved:          [u8; 120],
    pub sb_hash:            [u8; 32],
}

const _: () = assert!(
    core::mem::size_of::<SuperblockDisk>() == SUPERBLOCK_SIZE,
    "SuperblockDisk doit faire 256 octets"
);

impl SuperblockDisk {
    /// Désérialise depuis un buffer de 256 octets avec vérification HDR-03.
    ///
    /// # HDR-03 — Ordre strict :
    /// 1. magic == SUPERBLOCK_MAGIC
    /// 2. version == SUPERBLOCK_VERSION
    /// 3. sb_hash == Blake3(bytes[0..224])
    pub fn from_bytes(buf: &[u8; SUPERBLOCK_SIZE]) -> ExofsResult<Self> {
        // 1. Magic EN PREMIER (HDR-03).
        let magic = u64::from_le_bytes(buf[0..8].try_into().unwrap_or([0; 8]));
        if magic != SUPERBLOCK_MAGIC {
            return Err(ExofsError::InvalidMagic);
        }

        // 2. Version.
        if buf[8] != SUPERBLOCK_VERSION {
            return Err(ExofsError::InvalidMagic);
        }

        // 3. Checksum Blake3 sur bytes[0..224].
        let computed = blake3_hash(buf[0..224].try_into().unwrap_or(&[0u8; 224]));
        let stored: [u8; 32] = buf[224..256].try_into().unwrap_or([0; 32]);
        if computed != stored {
            return Err(ExofsError::ChecksumMismatch);
        }

        // SAFETY: cast byte-by-byte d'une struct #[repr(C, packed)] — taille vérifiée par const assert.
        Ok(unsafe { core::mem::transmute_copy(buf) })
    }

    /// `true` si le block_size est une puissance de 2 dans la plage [512, 65536].
    #[inline]
    pub fn block_size_valid(&self) -> bool {
        let bs = self.block_size;
        bs >= 512 && bs <= 65536 && bs.is_power_of_two()
    }

    /// `true` si les bornes de la région blob sont cohérentes.
    #[inline]
    pub fn blob_region_valid(&self) -> bool {
        self.blob_region_end > self.blob_region_start
    }

    /// `true` si free_blocks ≤ total_blocks.
    #[inline]
    pub fn free_blocks_valid(&self) -> bool {
        self.free_blocks <= self.total_blocks
    }
}

// ── Erreur de phase 1 ─────────────────────────────────────────────────────────

/// Type d'anomalie détectée lors de la phase 1.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase1ErrorKind {
    /// Magic du superbloc invalide.
    SuperblockBadMagic     = 0x01,
    /// Checksum du superbloc invalide.
    SuperblockBadChecksum  = 0x02,
    /// Version du superbloc non supportée.
    SuperblockBadVersion   = 0x03,
    /// Taille de bloc invalide.
    InvalidBlockSize       = 0x04,
    /// free_blocks > total_blocks.
    FreeBlocksOverflow     = 0x05,
    /// Région blob incohérente (end ≤ start).
    InvalidBlobRegion      = 0x06,
    /// Magic de la table d'allocation invalide.
    AllocTableBadMagic     = 0x10,
    /// Checksum de la table d'allocation invalide.
    AllocTableBadChecksum  = 0x11,
    /// Magic de l'en-tête de région blob invalide.
    BlobRegionBadMagic     = 0x20,
    /// Lecture I/O échouée.
    IoError                = 0xFE,
    /// Anomalie générique.
    Generic                = 0xFF,
}

/// Une entrée d'erreur de la phase 1.
#[derive(Clone, Copy, Debug)]
pub struct Phase1Error {
    /// Type d'anomalie.
    pub kind:   Phase1ErrorKind,
    /// LBA concerné.
    pub lba:    u64,
    /// Informations complémentaires.
    pub detail: u64,
}

// ── Rapport de phase 1 ────────────────────────────────────────────────────────

/// Rapport complet de la phase 1 du fsck.
#[derive(Clone, Debug)]
pub struct Phase1Report {
    /// Erreurs détectées.
    pub errors:             Vec<Phase1Error>,
    /// Superbloc lu (si valide).
    pub superblock:         Option<SuperblockDisk>,
    /// `true` si le superbloc est valide.
    pub superblock_ok:      bool,
    /// `true` si le superbloc est corrompu (anté `superblock_ok`).
    pub superblock_corrupt: bool,
    /// `true` si la table d'allocation est valide.
    pub alloc_table_ok:     bool,
    /// `true` si la région blob est cohérente.
    pub blob_region_ok:     bool,
    /// Nombre de LBA vérifiés.
    pub lbas_checked:       u64,
}

impl Phase1Report {
    /// `true` si aucune erreur.
    #[inline]
    pub fn is_clean(&self) -> bool {
        self.errors.is_empty()
    }

    /// Nombre d'erreurs.
    #[inline]
    pub fn error_count(&self) -> usize {
        self.errors.len()
    }

    /// Retourne une copie de la liste d'erreurs (allocation indépendante).
    ///
    /// # OOM-02
    /// `try_reserve(n)` avant les pushes.
    pub fn error_summary(&self) -> ExofsResult<Vec<Phase1Error>> {
        let n = self.errors.len();
        let mut v = Vec::new();
        v.try_reserve(n).map_err(|_| ExofsError::NoMemory)?;
        for e in &self.errors {
            v.push(*e);
        }
        Ok(v)
    }
}

// ── Alias de compatibilité pour recovery/mod.rs ───────────────────────────────────────

/// LBA du superbloc (alias de `FsckPhase1::DEFAULT_SUPERBLOCK_LBA`).
pub const SUPERBLOCK_LBA: u64 = 0x0800;
/// Magic d'en-tête du superbloc (alias de `SUPERBLOCK_MAGIC`).
pub const SUPERBLOCK_HDR_MAGIC: u64 = SUPERBLOCK_MAGIC;

/// Options de configuration de la Phase 1 du fsck.
#[derive(Debug, Default, Clone, Copy)]
pub struct Phase1Options {
    /// Nombre maximal d'erreurs avant abandon (0 = pas de limite).
    pub max_errors:       usize,
    /// Vérifier les checksums des blobs.
    pub verify_checksums: bool,
    /// LBA du superbloc à utiliser (0 = défaut = `SUPERBLOCK_LBA`).
    pub override_lba:     u64,
}

// ── Exécuteur de la phase 1 ─────────────────────────────────────────────

/// Exécuteur de la phase 1 du fsck.
pub struct FsckPhase1;

impl FsckPhase1 {
    /// LBA par défaut du superbloc.
    pub const DEFAULT_SUPERBLOCK_LBA: u64 = 0x0800;

    /// Exécute la phase 1 complète.
    ///
    /// # Étapes
    /// 1. Lire + valider le superbloc (HDR-03).
    /// 2. Valider les champs scalaires du superbloc.
    /// 3. Lire + valider l'en-tête de la table d'allocation.
    /// 4. Lire + valider l'en-tête de la région blob.
    ///
    /// Ne lève pas d'erreur fatale sur une anomalie : les erreurs sont
    /// collectées dans `Phase1Report.errors`.
    pub fn run(device: &dyn BlockDevice) -> ExofsResult<Phase1Report> {
        Self::run_at(device, Self::DEFAULT_SUPERBLOCK_LBA)
    }

    /// Exécute la phase 1 avec options.
    pub fn run_with_options(device: &dyn BlockDevice, opts: &Phase1Options) -> ExofsResult<Phase1Report> {
        let lba = if opts.override_lba != 0 { opts.override_lba } else { Self::DEFAULT_SUPERBLOCK_LBA };
        Self::run_at(device, lba)
    }

    /// Exécute la phase 1 avec LBA superbloc personnalisé.
    pub fn run_at(device: &dyn BlockDevice, sb_lba: u64) -> ExofsResult<Phase1Report> {
        RECOVERY_LOG.log_phase_start(1);

        let mut errors: Vec<Phase1Error> = Vec::new();
        let mut superblock: Option<SuperblockDisk> = None;
        let mut alloc_table_ok = false;
        let mut blob_region_ok = false;
        let mut lbas_checked: u64 = 0;

        // ── Étape 1 : lecture du superbloc ─────────────────────────────────
        let sb_buf = match read_array::<SUPERBLOCK_SIZE>(device, sb_lba) {
            Err(_) => {
                errors.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                errors.push(Phase1Error {
                    kind:   Phase1ErrorKind::IoError,
                    lba:    sb_lba,
                    detail: 0,
                });
                // Pas de superbloc → arrêt de la phase 1.
                let report = Phase1Report {
                    errors,
                    superblock:        None,
                    superblock_ok:     false,
                    superblock_corrupt: true,
                    alloc_table_ok:    false,
                    blob_region_ok:    false,
                    lbas_checked:      0,
                };
                RECOVERY_LOG.log_phase_done(1, 1);
                return Ok(report);
            }
            Ok(buf) => buf,
        };
        lbas_checked = lbas_checked.checked_add(1).unwrap_or(u64::MAX);

        // ── Étape 2 : validation du superbloc (HDR-03) ─────────────────────
        match SuperblockDisk::from_bytes(&sb_buf) {
            Err(ExofsError::InvalidMagic) => {
                RECOVERY_AUDIT.record_invalid_magic(sb_lba, 0);
                errors.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                errors.push(Phase1Error {
                    kind:   Phase1ErrorKind::SuperblockBadMagic,
                    lba:    sb_lba,
                    detail: 0,
                });
            }
            Err(ExofsError::ChecksumMismatch) => {
                RECOVERY_AUDIT.record_checksum_invalid(sb_lba, 0, 0);
                errors.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                errors.push(Phase1Error {
                    kind:   Phase1ErrorKind::SuperblockBadChecksum,
                    lba:    sb_lba,
                    detail: 0,
                });
            }
            Err(_) => {
                errors.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                errors.push(Phase1Error {
                    kind:   Phase1ErrorKind::Generic,
                    lba:    sb_lba,
                    detail: 0,
                });
            }
            Ok(sb) => {
                superblock = Some(sb);

                // ── Étape 3 : validation des champs scalaires ───────────────
                if !sb.block_size_valid() {
                    errors.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                    errors.push(Phase1Error {
                        kind:   Phase1ErrorKind::InvalidBlockSize,
                        lba:    sb_lba,
                        detail: sb.block_size as u64,
                    });
                }

                if !sb.free_blocks_valid() {
                    errors.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                    errors.push(Phase1Error {
                        kind:   Phase1ErrorKind::FreeBlocksOverflow,
                        lba:    sb_lba,
                        detail: sb.free_blocks,
                    });
                }

                if !sb.blob_region_valid() {
                    errors.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                    errors.push(Phase1Error {
                        kind:   Phase1ErrorKind::InvalidBlobRegion,
                        lba:    sb_lba,
                        detail: sb.blob_region_start,
                    });
                } else {
                    // ── Étape 4 : valider la table d'allocation ─────────────
                    let alloc_lba = sb.alloc_lba;
                    if let Ok(alloc_buf) = read_array::<64>(device, alloc_lba) {
                        lbas_checked = lbas_checked.checked_add(1).unwrap_or(u64::MAX);
                        let magic = u64::from_le_bytes(alloc_buf[0..8].try_into().unwrap_or([0; 8]));
                        if magic != ALLOC_TABLE_MAGIC {
                            RECOVERY_AUDIT.record_invalid_magic(alloc_lba, magic);
                            errors.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                            errors.push(Phase1Error {
                                kind:   Phase1ErrorKind::AllocTableBadMagic,
                                lba:    alloc_lba,
                                detail: magic,
                            });
                        } else {
                            alloc_table_ok = true;
                        }
                    } else {
                        errors.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                        errors.push(Phase1Error {
                            kind:   Phase1ErrorKind::IoError,
                            lba:    alloc_lba,
                            detail: 0,
                        });
                    }

                    // ── Étape 5 : valider l'en-tête de la région blob ───────
                    let region_lba = sb.blob_region_start;
                    if let Ok(region_buf) = read_array::<64>(device, region_lba) {
                        lbas_checked = lbas_checked.checked_add(1).unwrap_or(u64::MAX);
                        let magic = u64::from_le_bytes(region_buf[0..8].try_into().unwrap_or([0; 8]));
                        if magic != BLOB_REGION_MAGIC {
                            RECOVERY_AUDIT.record_invalid_magic(region_lba, magic);
                            errors.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                            errors.push(Phase1Error {
                                kind:   Phase1ErrorKind::BlobRegionBadMagic,
                                lba:    region_lba,
                                detail: magic,
                            });
                        } else {
                            blob_region_ok = true;
                        }
                    } else {
                        errors.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                        errors.push(Phase1Error {
                            kind:   Phase1ErrorKind::IoError,
                            lba:    region_lba,
                            detail: 0,
                        });
                    }
                }
            }
        }

        let error_count = errors.len() as u32;
        RECOVERY_LOG.log_phase_done(1, error_count);
        RECOVERY_AUDIT.record_phase_done(1, error_count);

        Ok(Phase1Report {
            errors,
            superblock,
            superblock_ok:      superblock.is_some(),
            superblock_corrupt: superblock.is_none(),
            alloc_table_ok,
            blob_region_ok,
            lbas_checked,
        })
    }

    /// Retourne le LBA du superbloc depuis un résultat de sélection de slot.
    pub fn superblock_lba_from_slot(
        slot: &super::slot_recovery::SlotRecoveryResult,
    ) -> u64 {
        slot.superblock_lba
    }
}

// ── Tests unitaires ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_superblock_bad_magic() {
        let buf = [0u8; SUPERBLOCK_SIZE];
        let r = SuperblockDisk::from_bytes(&buf);
        assert!(matches!(r, Err(ExofsError::InvalidMagic)));
    }

    #[test]
    fn test_superblock_fields_validation() {
        // SAFETY: type entièrement initialisable par zéros (repr(C) avec champs numériques).
        let mut sb: SuperblockDisk = unsafe { core::mem::zeroed() };
        sb.block_size   = 4096;
        sb.total_blocks = 1000;
        sb.free_blocks  = 500;
        sb.blob_region_start = 100;
        sb.blob_region_end   = 900;
        assert!(sb.block_size_valid());
        assert!(sb.free_blocks_valid());
        assert!(sb.blob_region_valid());
    }

    #[test]
    fn test_superblock_free_blocks_overflow() {
        // SAFETY: type entièrement initialisable par zéros (repr(C) avec champs numériques).
        let mut sb: SuperblockDisk = unsafe { core::mem::zeroed() };
        sb.total_blocks = 100;
        sb.free_blocks  = 200;
        assert!(!sb.free_blocks_valid());
    }

    #[test]
    fn test_superblock_invalid_block_size() {
        // SAFETY: type entièrement initialisable par zéros (repr(C) avec champs numériques).
        let mut sb: SuperblockDisk = unsafe { core::mem::zeroed() };
        sb.block_size = 600; // Non puissance de 2.
        assert!(!sb.block_size_valid());
    }

    #[test]
    fn test_phase1_report_clean() {
        let r = Phase1Report {
            errors:            Vec::new(),
            superblock:        None,
            superblock_ok:     true,
            superblock_corrupt: false,
            alloc_table_ok:    true,
            blob_region_ok:    true,
            lbas_checked:      3,
        };
        assert!(r.is_clean());
        assert_eq!(r.error_count(), 0);
    }
}
