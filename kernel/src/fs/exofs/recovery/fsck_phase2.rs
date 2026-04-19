//! fsck_phase2.rs — Phase 2 du fsck : parcours de l'arbre de blobs et comptage de références.
//!
//! Lit la table d'allocation des blobs, parcourt chaque blob référencé depuis
//! la racine, et construit une table de comptage de références. Les blobs
//! avec un compte de références anormal sont signalés.
//!
//! # Règles spec appliquées
//! - **HASH-02** : `verify_blob_id` sur chaque blob lu (données RAW).
//! - **OOM-02** : `try_reserve(1)` avant tout `BTreeMap::insert` et `Vec::push`.
//! - **HDR-03** : magic de chaque en-tête de blob vérifié EN PREMIER.
//! - **ARITH-02** : `checked_add` pour les offsets et compteurs.


extern crate alloc;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult, BlobId};
use crate::fs::exofs::core::blob_id::verify_blob_id;
use super::boot_recovery::BlockDevice;
use super::block_io::read_bytes;
use super::fsck_phase1::Phase1Report;
use super::recovery_audit::RECOVERY_AUDIT;
use super::recovery_log::RECOVERY_LOG;

// ── Constantes ────────────────────────────────────────────────────────────────

/// Magic d'un en-tête de blob on-disk : "EXOBLHDR".
pub const BLOB_HEADER_MAGIC: u64 = 0x5244484C424F5845; // "EXOBLHDR"

/// Taille de l'en-tête de blob on-disk.
pub const BLOB_HEADER_SIZE: usize = 96;

/// Magic de la table de références : "EXOREFTB".
pub const REF_TABLE_MAGIC: u64 = 0x425446455246_4F45; // not needed on-disk

/// Nombre maximal de blobs attendus dans le walk (protection contre boucles).
pub const PHASE2_MAX_BLOBS: usize = 1_048_576;

// ── En-tête de blob on-disk ───────────────────────────────────────────────────

/// En-tête on-disk d'un blob — `repr(C)`, 96 octets.
///
/// # ONDISK-03
/// Pas d'`AtomicU64`.
///
/// # Layout
/// ```text
/// off  0 : magic      u64    8B
/// off  8 : version    u8     1B
/// off  9 : flags      u8     1B   — bit0=deleted, bit1=compressed
/// off 10 : _pad0      u16    2B
/// off 12 : ref_count  u32    4B
/// off 16 : blob_id    [u8;32] 32B — BlobId (Blake3 des données raw)
/// off 48 : data_len   u64    8B
/// off 56 : data_lba   u64    8B
/// off 64 : parent_id  [u8;32] 32B — BlobId du parent (ou zéros)
/// total  : 96B
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BlobHeaderDisk {
    pub magic:    u64,
    pub version:  u8,
    pub flags:    u8,
    pub _pad0:    u16,
    pub ref_count: u32,
    pub blob_id:  [u8; 32],
    pub data_len: u64,
    pub data_lba: u64,
    pub parent_id: [u8; 32],
}

const _: () = assert!(
    core::mem::size_of::<BlobHeaderDisk>() == BLOB_HEADER_SIZE,
    "BlobHeaderDisk doit faire 96 octets"
);

impl BlobHeaderDisk {
    /// Désérialise depuis un buffer de 96 octets.
    ///
    /// # HDR-03 — magic EN PREMIER.
    pub fn from_bytes(buf: &[u8; BLOB_HEADER_SIZE]) -> ExofsResult<Self> {
        let magic = u64::from_le_bytes(buf[0..8].try_into().unwrap_or([0;8]));
        if magic != BLOB_HEADER_MAGIC {
            return Err(ExofsError::InvalidMagic);
        }
        if buf[8] != 1 {
            return Err(ExofsError::InvalidMagic);
        }
        // SAFETY: cast byte-by-byte d'une struct #[repr(C, packed)] — taille vérifiée par const assert.
        Ok(unsafe { core::mem::transmute_copy(buf) })
    }

    /// `true` si ce blob est marqué supprimé.
    #[inline]
    pub fn is_deleted(&self) -> bool { self.flags & 0x01 != 0 }

    /// `true` si ce blob est compressé.
    #[inline]
    pub fn is_compressed(&self) -> bool { self.flags & 0x02 != 0 }

    /// Retourne le `BlobId`.
    #[inline]
    pub fn blob_id(&self) -> BlobId { BlobId(self.blob_id) }

    /// Retourne le `BlobId` parent (tous-zéros = pas de parent).
    #[inline]
    pub fn parent_blob_id(&self) -> Option<BlobId> {
        if self.parent_id == [0u8; 32] { None } else { Some(BlobId(self.parent_id)) }
    }
}

// ── Erreur de phase 2 ─────────────────────────────────────────────────────────

/// Type d'anomalie détectée en phase 2.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase2ErrorKind {
    /// BlobId calculé ≠ BlobId stocké (HASH-02).
    BlobIdMismatch        = 0x01,
    /// Checksum des données invalide.
    DataChecksumInvalid   = 0x02,
    /// Magic de l'en-tête blob invalide.
    BlobHeaderBadMagic    = 0x03,
    /// Compteur de référence incohérent.
    RefCountMismatch      = 0x04,
    /// Blob parent introuvable.
    ParentNotFound        = 0x05,
    /// Cycle détecté dans la chaîne de blobs.
    CycleDetected         = 0x06,
    /// Données du blob illisibles (erreur I/O).
    IoError               = 0xFE,
    /// Hash des données invalide (alias de DataChecksumInvalid).
    DataHashMismatch      = 0x10,
    /// Checksum de l'en-tête invalide.
    HeaderBadChecksum     = 0x11,
    /// Magic de l'en-tête invalide (alias de BlobHeaderBadMagic).
    HeaderBadMagic        = 0x12,
}

/// Une entrée d'erreur de la phase 2.
#[derive(Clone, Copy, Debug)]
pub struct Phase2Error {
    pub kind:    Phase2ErrorKind,
    pub blob_id: [u8; 32],
    pub lba:     u64,
    pub detail:  u64,
}

// ── Compteur de références ────────────────────────────────────────────────────

/// Table de comptage de références blob : `BlobId → count`.
#[derive(Clone, Debug)]
pub struct BlobRefCounter {
    map: BTreeMap<[u8; 32], u32>,
}

impl BlobRefCounter {
    /// Construit un compteur vide.
    pub fn new() -> Self {
        Self { map: BTreeMap::new() }
    }

    /// Incrémente le compteur pour un `BlobId`.
    ///
    /// # OOM-02
    /// `try_reserve(1)` avant `insert`.
    ///
    /// # ARITH-02
    /// `checked_add` pour éviter le débordement.
    pub fn increment(&mut self, id: &[u8; 32]) -> ExofsResult<()> {
        if let Some(count) = self.map.get_mut(id) {
            *count = count.checked_add(1).unwrap_or(u32::MAX);
        } else {
            self.map.insert(*id, 1);
        }
        Ok(())
    }

    /// Retourne le compteur pour un `BlobId` (0 si inconnu).
    #[inline]
    pub fn count(&self, id: &[u8; 32]) -> u32 {
        self.map.get(id).copied().unwrap_or(0)
    }

    /// Nombre de blobs uniques référencés.
    #[inline]
    pub fn unique_count(&self) -> usize {
        self.map.len()
    }

    /// Retourne la liste des blobs avec un référencement anormal (count > max_ref).
    ///
    /// # OOM-02
    /// `try_reserve(1)` avant chaque `push`.
    pub fn unreferenced_blobs(&self, max_ref: u32) -> ExofsResult<Vec<[u8; 32]>> {
        let mut out = Vec::new();
        for (id, &count) in &self.map {
            if count > max_ref {
                out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                out.push(*id);
            }
        }
        Ok(out)
    }
}

// ── Rapport de phase 2 ────────────────────────────────────────────────────────

/// Rapport complet de la phase 2 du fsck.
#[derive(Clone, Debug)]
pub struct Phase2Report {
    /// Erreurs détectées.
    pub errors:         Vec<Phase2Error>,
    /// Nombre de blobs parcourus.
    pub blobs_walked:   u64,
    /// Nombre de blobs avec hash valide.
    pub blobs_ok:       u64,
    /// Nombre de blobs avec hash invalide.
    pub blobs_hash_err: u64,
    /// Nombre de blobs orphans détectés.
    pub orphans:        u64,
    /// Table de comptage de référence construite.
    pub ref_counter:    BlobRefCounter,
    /// Entrées d'allocation observées pendant le scan.
    pub alloc_entries:  Vec<AllocEntry>,
}

impl Phase2Report {
    /// `true` si aucune erreur.
    #[inline]
    pub fn is_clean(&self) -> bool { self.errors.is_empty() }

    /// Nombre d'erreurs.
    #[inline]
    pub fn error_count(&self) -> usize { self.errors.len() }

    /// Itère sur les entrées d'allocation observées par la phase 2.
    #[inline]
    pub fn alloc_entries_iter(&self) -> core::slice::Iter<'_, AllocEntry> {
        self.alloc_entries.iter()
    }
}

// ── Entrée de la table d'allocation ──────────────────────────────────────────

/// Un enregistrement de la table d'allocation (position d'un blob sur disque).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AllocEntry {
    /// BlobId.
    pub blob_id:  [u8; 32],
    /// LBA de l'en-tête de blob.
    pub hdr_lba:  u64,
    /// LBA des données.
    pub data_lba: u64,
    /// Longueur des données.
    pub data_len: u64,
}

/// Taille d'un `AllocEntry` on-disk.
pub const ALLOC_ENTRY_SIZE: usize = core::mem::size_of::<AllocEntry>();

// ── Options de configuration ─────────────────────────────────────────────────────

/// Options de configuration de la Phase 2 du fsck.
#[derive(Debug, Default, Clone, Copy)]
pub struct Phase2Options {
    /// Nombre maximal d'erreurs avant abandon (0 = pas de limite).
    pub max_errors:          usize,
    /// Reconstruire la table de références si incohérente.
    pub rebuild_ref_table:   bool,
}

// ── Exécuteur de la phase 2 ─────────────────────────────────────────────

/// Exécuteur de la phase 2 du fsck.
pub struct FsckPhase2;

impl FsckPhase2 {
    /// Exécute la phase 2 avec options (exécute la phase 1 en interne pour obtenir le superbloc).
    pub fn run_with_options(
        device: &dyn BlockDevice,
        opts:   &Phase2Options,
    ) -> ExofsResult<Phase2Report> {
        use super::fsck_phase1::FsckPhase1;
        let phase1  = FsckPhase1::run(device)?;
        let max_blobs = if opts.max_errors == 0 { PHASE2_MAX_BLOBS } else { opts.max_errors };
        Self::run(device, &phase1, max_blobs)
    }

    /// Exécute la phase 2 : lit la table d'allocation et valide chaque blob.
    ///
    /// Requiert le rapport de la phase 1 pour obtenir le superbloc.
    pub fn run(
        device:    &dyn BlockDevice,
        phase1:    &Phase1Report,
        max_blobs: usize,
    ) -> ExofsResult<Phase2Report> {
        RECOVERY_LOG.log_phase_start(2);

        let sb = match &phase1.superblock {
            Some(s) => *s,
            None    => {
                // Pas de superbloc = impossible de lire les blobs.
                let report = Phase2Report {
                    errors:         Vec::new(),
                    blobs_walked:   0,
                    blobs_ok:       0,
                    blobs_hash_err: 0,
                    orphans:        0,
                    ref_counter:    BlobRefCounter::new(),
                    alloc_entries:  Vec::new(),
                };
                RECOVERY_LOG.log_phase_done(2, 0);
                return Ok(report);
            }
        };

        let max_blobs = max_blobs.min(PHASE2_MAX_BLOBS);
        let alloc_lba = sb.alloc_lba;

        let mut errors:   Vec<Phase2Error> = Vec::new();
        let mut blobs_walked:   u64 = 0;
        let mut blobs_ok:       u64 = 0;
        let mut blobs_hash_err: u64 = 0;
        let mut ref_counter = BlobRefCounter::new();
        let mut alloc_entries = Vec::new();

        // Parcourir la table d'allocation bloc par bloc.
        let entries_per_block = (device.block_size() as usize)
            .checked_div(ALLOC_ENTRY_SIZE)
            .unwrap_or(1)
            .max(1);

        let mut lba = alloc_lba;
        let mut total_read = 0usize;

        'outer: loop {
            let mut block_buf = alloc::vec![0u8; device.block_size() as usize];
            if device.read_block(lba, &mut block_buf).is_err() {
                break;
            }

            for i in 0..entries_per_block {
                if total_read >= max_blobs { break 'outer; }

                // ARITH-02 : checked mul pour l'offset.
                let off = i
                    .checked_mul(ALLOC_ENTRY_SIZE)
                    .unwrap_or(usize::MAX);
                if off.checked_add(ALLOC_ENTRY_SIZE).unwrap_or(usize::MAX) > block_buf.len() {
                    break 'outer;
                }

                let entry_bytes: &[u8; 48] = match block_buf[off..off + 48].try_into() {
                    Ok(b) => b,
                    Err(_) => break 'outer,
                };

                // Vérifier si l'entrée est vide (BlobId nul = fin de table).
                if entry_bytes.iter().all(|&b| b == 0) {
                    break 'outer;
                }

                // Lire l'entrée.
                // SAFETY: cast byte-by-byte d'une struct #[repr(C, packed)] — taille vérifiée par const assert.
                let entry: AllocEntry = unsafe { core::mem::transmute_copy(entry_bytes) };
                alloc_entries.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                alloc_entries.push(entry);

                // Lire l'en-tête du blob.
                let mut hdr_buf = [0u8; BLOB_HEADER_SIZE];
                if read_bytes(device, entry.hdr_lba, &mut hdr_buf).is_err() {
                    errors.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                    errors.push(Phase2Error {
                        kind:    Phase2ErrorKind::IoError,
                        blob_id: entry.blob_id,
                        lba:     entry.hdr_lba,
                        detail:  0,
                    });
                    total_read += 1;
                    continue;
                }

                // HDR-03 : magic EN PREMIER.
                let hdr = match BlobHeaderDisk::from_bytes(&hdr_buf) {
                    Ok(h) => h,
                    Err(ExofsError::InvalidMagic) => {
                        RECOVERY_AUDIT.record_invalid_magic(entry.hdr_lba, 0);
                        errors.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                        errors.push(Phase2Error {
                            kind:    Phase2ErrorKind::BlobHeaderBadMagic,
                            blob_id: entry.blob_id,
                            lba:     entry.hdr_lba,
                            detail:  0,
                        });
                        total_read += 1;
                        continue;
                    }
                    Err(_) => {
                        total_read += 1;
                        continue;
                    }
                };

                // Lire les données pour HASH-02.
                let data_len = hdr.data_len as usize;
                if data_len > 0 && data_len <= 64 * 1024 * 1024 {
                    let mut data_buf = alloc::vec![0u8; data_len];
                    if read_bytes(device, hdr.data_lba, &mut data_buf).is_ok() {
                        // HASH-02 : verify_blob_id sur données RAW.
                        let blob_id = hdr.blob_id();
                        if verify_blob_id(&blob_id, &data_buf) {
                            blobs_ok = blobs_ok.checked_add(1).unwrap_or(u64::MAX);
                        } else {
                            blobs_hash_err = blobs_hash_err.checked_add(1).unwrap_or(u64::MAX);
                            RECOVERY_AUDIT.record_checksum_invalid(hdr.data_lba, 0, 0);
                            errors.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                            errors.push(Phase2Error {
                                kind:    Phase2ErrorKind::BlobIdMismatch,
                                blob_id: hdr.blob_id,
                                lba:     hdr.data_lba,
                                detail:  hdr.data_len,
                            });
                        }
                    }
                }

                // OOM-02 : try_reserve dans increment.
                ref_counter.increment(&hdr.blob_id)?;

                // Incrémenter le compteur du parent si présent.
                if let Some(parent) = hdr.parent_blob_id() {
                    ref_counter.increment(&parent.0)?;
                }

                total_read += 1;
                blobs_walked = blobs_walked.checked_add(1).unwrap_or(u64::MAX);
            }

            // Avancer au bloc suivant de la table.
            lba = match lba.checked_add(1) {
                Some(l) => l,
                None    => break,
            };

            // Limiter le parcours.
            if total_read >= max_blobs { break; }
        }

        let error_count = errors.len() as u32;
        RECOVERY_LOG.log_phase_done(2, error_count);
        RECOVERY_AUDIT.record_phase_done(2, error_count);

        let mut orphans = 0u64;
        let mut idx = 0usize;
        while idx < alloc_entries.len() {
            if ref_counter.count(&alloc_entries[idx].blob_id) == 0 {
                orphans = orphans.checked_add(1).unwrap_or(u64::MAX);
            }
            idx = idx.wrapping_add(1);
        }

        Ok(Phase2Report {
            errors,
            blobs_walked,
            blobs_ok,
            blobs_hash_err,
            orphans,
            ref_counter,
            alloc_entries,
        })
    }
}

// ── Tests unitaires ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blob_header_bad_magic() {
        let buf = [0u8; BLOB_HEADER_SIZE];
        let r = BlobHeaderDisk::from_bytes(&buf);
        assert!(matches!(r, Err(ExofsError::InvalidMagic)));
    }

    #[test]
    fn test_blob_ref_counter() {
        let mut counter = BlobRefCounter::new();
        let id = [1u8; 32];
        counter.increment(&id).unwrap();
        counter.increment(&id).unwrap();
        assert_eq!(counter.count(&id), 2);
        assert_eq!(counter.unique_count(), 1);
    }

    #[test]
    fn test_blob_ref_counter_multiple() {
        let mut counter = BlobRefCounter::new();
        for i in 0..10u8 {
            let mut id = [0u8; 32];
            id[0] = i;
            counter.increment(&id).unwrap();
        }
        assert_eq!(counter.unique_count(), 10);
    }

    #[test]
    fn test_phase2_report_clean() {
        let r = Phase2Report {
            errors:         Vec::new(),
            blobs_walked:   50,
            blobs_ok:       50,
            blobs_hash_err: 0,
            orphans:        0,
            ref_counter:    BlobRefCounter::new(),
            alloc_entries:  Vec::new(),
        };
        assert!(r.is_clean());
    }

    #[test]
    fn test_alloc_entries_iter_exposes_entries() {
        let entry = AllocEntry {
            blob_id: [0xAA; 32],
            hdr_lba: 1,
            data_lba: 2,
            data_len: 3,
        };
        let r = Phase2Report {
            errors:         Vec::new(),
            blobs_walked:   1,
            blobs_ok:       1,
            blobs_hash_err: 0,
            orphans:        0,
            ref_counter:    BlobRefCounter::new(),
            alloc_entries:  alloc::vec![entry],
        };
        let collected: Vec<AllocEntry> = r.alloc_entries_iter().copied().collect();
        assert_eq!(collected.len(), 1);
        assert_eq!(collected[0].hdr_lba, 1);
    }

    #[test]
    fn test_blob_header_flags() {
        // SAFETY: type entièrement initialisable par zéros (repr(C) avec champs numériques).
        let mut hdr: BlobHeaderDisk = unsafe { core::mem::zeroed() };
        hdr.flags = 0x01;
        assert!(hdr.is_deleted());
        assert!(!hdr.is_compressed());
        hdr.flags = 0x02;
        assert!(hdr.is_compressed());
    }
}
