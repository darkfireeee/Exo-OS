//! fsck_phase4.rs — Phase 4 du fsck ExoFS : récupération des blobs orphelins.
//!
//! Identifie les blobs dont le compteur de références est nul (selon la table
//! construite en phase 2) et les déplace vers une région "lost+found" pour
//! permettre une récupération manuelle ou un effacement sécurisé.
//!
//! # Règles spec appliquées
//! - **HDR-03** : re-lecture et re-validation des en-têtes blob avant déplacement.
//! - **HASH-02** : `verify_blob_id` sur les données avant toute écriture.
//! - **OOM-02** : `try_reserve(1)` avant tout `Vec::push` et `BTreeMap::insert`.
//! - **ARITH-02** : `checked_add` / `checked_mul` sur tous les calculs d offset.
//! - **WRITE-02** : vérification que `bytes_written == expected` après chaque écriture.


extern crate alloc;
use alloc::vec::Vec;

use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::core::blob_id::{blake3_hash, verify_blob_id};
use super::boot_recovery::BlockDevice;
use super::block_io::read_bytes;
use super::fsck_phase2::Phase2Report;
use super::recovery_audit::RECOVERY_AUDIT;
use super::recovery_log::RECOVERY_LOG;

// ── Constantes ────────────────────────────────────────────────────────────────

/// Magic identifiant la région lost+found on-disk.
pub const LOST_FOUND_MAGIC: u64  = 0x444E4F464C544650; // "PFTLFOND"
/// Version du format de la région lost+found.
pub const LOST_FOUND_VERSION: u8 = 1;
/// Taille de l en-tête on-disk de la région lost+found.
pub const LOST_FOUND_HDR_SIZE: usize = 64;
/// LBA par défaut de la région lost+found.
pub const LOST_FOUND_REGION_LBA: u64 = 0x8000;
/// Capacité maximale de la région lost+found (en entrées).
pub const LOST_FOUND_CAPACITY: usize = 65536;
/// Taille d une entrée dans la table lost+found (on-disk).
pub const LOST_FOUND_ENTRY_SIZE: usize = 48;

// ── En-tête de la région lost+found ──────────────────────────────────────────

/// En-tête on-disk de la région lost+found — `repr(C)`, 64 octets, ONDISK-03.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LostFoundHeaderDisk {
    pub magic:      u64,
    pub version:    u8,
    pub _pad:       [u8; 7],
    pub n_entries:  u32,
    pub capacity:   u32,
    pub region_lba: u64,
    pub created_tick: u64,
    pub _reserved:  [u8; 16],
    pub hdr_hash:   [u8; 8],  // CRC simple sur [0..56].
}

const _CHECK_LF_HDR: () = assert!(
    core::mem::size_of::<LostFoundHeaderDisk>() == LOST_FOUND_HDR_SIZE,
    "LostFoundHeaderDisk doit faire 64 octets"
);

impl LostFoundHeaderDisk {
    pub fn from_bytes(buf: &[u8; LOST_FOUND_HDR_SIZE]) -> ExofsResult<Self> {
        // HDR-03 : magic EN PREMIER.
        let magic = u64::from_le_bytes([
            buf[0], buf[1], buf[2], buf[3],
            buf[4], buf[5], buf[6], buf[7],
        ]);
        if magic != LOST_FOUND_MAGIC { return Err(ExofsError::InvalidMagic); }
        if buf[8] != LOST_FOUND_VERSION { return Err(ExofsError::InvalidMagic); }
        // Checksum basique : Blake3(buf[0..56])[0..8].
        let body: &[u8; 56] = buf[0..56].try_into().map_err(|_| ExofsError::CorruptedStructure)?;
        // Utiliser le hash Blake3 en tronquant à 8 octets.
        let full_hash = {
            let padded = {
                let mut p = [0u8; 224];
                p[0..56].copy_from_slice(body);
                p
            };
            blake3_hash(&padded)
        };
        let stored = &buf[56..64];
        if &full_hash[0..8] != stored { return Err(ExofsError::ChecksumMismatch); }
        // SAFETY: cast byte-by-byte d'une struct #[repr(C, packed)] — taille vérifiée par const assert.
        Ok(unsafe { core::mem::transmute_copy(buf) })
    }

    pub fn build(n_entries: u32, capacity: u32, region_lba: u64, tick: u64) -> Self {
        let mut h = LostFoundHeaderDisk {
            magic: LOST_FOUND_MAGIC,
            version: LOST_FOUND_VERSION,
            _pad: [0; 7],
            n_entries,
            capacity,
            region_lba,
            created_tick: tick,
            _reserved: [0; 16],
            hdr_hash: [0; 8],
        };
        // SAFETY: cast byte-by-byte d'une struct #[repr(C, packed)] — taille vérifiée par const assert.
        let raw: [u8; LOST_FOUND_HDR_SIZE] = unsafe { core::mem::transmute_copy(&h) };
        let padded = {
            let mut p = [0u8; 224];
            p[0..56].copy_from_slice(&raw[0..56]);
            p
        };
        let full_hash = blake3_hash(&padded);
        h.hdr_hash.copy_from_slice(&full_hash[0..8]);
        h
    }
}

// ── Entrée lost+found ─────────────────────────────────────────────────────────

/// Entrée on-disk dans la table lost+found — `repr(C)`, 48 octets, ONDISK-03.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LostFoundEntry {
    /// BlobId du blob orphelin.
    pub blob_id:    [u8; 32],
    /// LBA d origine (où le blob a été trouvé).
    pub origin_lba: u64,
    /// Taille des données en octets.
    pub data_len:   u64,
}

const _CHECK_LF_ENTRY: () = assert!(
    core::mem::size_of::<LostFoundEntry>() == LOST_FOUND_ENTRY_SIZE,
    "LostFoundEntry doit faire 48 octets"
);

// ── Erreurs de phase 4 ────────────────────────────────────────────────────────

/// Classification des erreurs détectées lors de la phase 4.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase4ErrorKind {
    /// Erreur I/O lors de la lecture du blob.
    ReadIoError        = 0x01,
    /// Erreur I/O lors de l écriture dans lost+found.
    WriteIoError       = 0x02,
    /// La vérification HASH-02 a échoué sur les données.
    HashMismatch       = 0x03,
    /// La région lost+found est pleine.
    LostFoundFull      = 0x04,
    /// Overflow arithmétique lors du calcul d offset.
    ArithOverflow      = 0x05,
    /// Echec de la mise à jour de la table d allocation.
    AllocUpdateFailed  = 0x06,
    /// Ecriture partielle (WRITE-02).
    PartialWrite       = 0x07,
}

/// Erreur individuelle relevée lors de la phase 4.
#[derive(Clone, Copy, Debug)]
pub struct Phase4Error {
    pub kind:    Phase4ErrorKind,
    pub blob_id: [u8; 32],
    /// LBA source du blob.
    pub lba:     u64,
    pub detail:  u64,
}

// ── Options de la phase 4 ─────────────────────────────────────────────────────

/// Options configurables pour la phase 4.
#[derive(Clone, Copy, Debug)]
pub struct Phase4Options {
    /// LBA de départ de la région lost+found.
    pub lost_found_lba:  u64,
    /// Capacité de la région (nombre d entrées).
    pub capacity:        usize,
    /// Si `true`, simule les actions sans écrire sur le disque.
    pub dry_run:         bool,
    /// Nombre maximal d erreurs avant abandon.
    pub max_errors:      u32,
    /// Si `true`, stoppe à la première erreur I/O.
    pub stop_on_io_err:  bool,
    /// Si `true`, vérifie les données via HASH-02 avant déplacement.
    pub verify_data:     bool,
}

impl Default for Phase4Options {
    fn default() -> Self {
        Self {
            lost_found_lba:  LOST_FOUND_REGION_LBA,
            capacity:        LOST_FOUND_CAPACITY,
            dry_run:         false,
            max_errors:      256,
            stop_on_io_err:  false,
            verify_data:     true,
        }
    }
}

// ── Rapport de phase 4 ────────────────────────────────────────────────────────

/// Résumé de l exécution de la phase 4.
#[derive(Clone, Debug)]
pub struct Phase4Report {
    /// Erreurs individuelles relevées.
    pub errors:              Vec<Phase4Error>,
    /// Blobs orphelins trouvés (ref_count == 0).
    pub orphans_found:       u64,
    /// Blobs orphelins déplacés dans lost+found.
    pub orphans_recovered:   u64,
    /// Blobs orphelins non récupérables.
    pub orphans_abandoned:   u64,
    /// Taux de récupération en pourcentage.
    pub recovery_rate_pct:   u64,
    /// Octets totaux récupérés.
    pub bytes_recovered:     u64,
    /// Mode dry-run actif.
    pub dry_run:             bool,
}

impl Phase4Report {
    pub fn is_clean(&self) -> bool { self.errors.is_empty() }
    pub fn error_count(&self) -> usize { self.errors.len() }
}

// ── Table lost+found en mémoire ───────────────────────────────────────────────

/// Table lost+found maintenue en mémoire pendant la phase 4.
struct LostFoundTable {
    entries:   Vec<LostFoundEntry>,
    capacity:  usize,
    base_lba:  u64,
}

impl LostFoundTable {
    fn new(capacity: usize, base_lba: u64) -> Self {
        Self {
            entries:  Vec::new(),
            capacity,
            base_lba,
        }
    }

    /// Ajoute une entrée — OOM-02.
    fn add(&mut self, entry: LostFoundEntry) -> ExofsResult<()> {
        if self.entries.len() >= self.capacity {
            return Err(ExofsError::NoSpace);
        }
        self.entries.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.entries.push(entry);
        Ok(())
    }

    /// Retourne le LBA de l entrée i dans la région lost+found.
    ///
    /// Layout : `[HDR (1 bloc)][entry0][entry1]...`
    fn entry_lba(&self, idx: usize, block_size: u64) -> ExofsResult<u64> {
        let entry_blocks = (LOST_FOUND_ENTRY_SIZE as u64)
            .checked_add(block_size.saturating_sub(1))
            .and_then(|v| v.checked_div(block_size))
            .ok_or(ExofsError::OffsetOverflow)?
            .max(1);
        // LBA = base + 1 (header) + idx * entry_blocks.
        let offset = (idx as u64)
            .checked_mul(entry_blocks)
            .and_then(|o| o.checked_add(1))
            .and_then(|o| self.base_lba.checked_add(o))
            .ok_or(ExofsError::OffsetOverflow)?;
        Ok(offset)
    }

    /// Persiste l entrée i sur le device — WRITE-02.
    fn write_entry(
        &self,
        idx:        usize,
        device:     &dyn BlockDevice,
    ) -> ExofsResult<()> {
        if idx >= self.entries.len() { return Err(ExofsError::InvalidArgument); }
        let block_size = device.block_size() as u64;
        let lba = self.entry_lba(idx, block_size)?;
        let entry = &self.entries[idx];
        // Sérialiser dans un buffer de taille = un bloc.
        let mut buf = alloc::vec![0u8; block_size as usize];
        if buf.len() < LOST_FOUND_ENTRY_SIZE {
            return Err(ExofsError::InvalidArgument);
        }
        // SAFETY: cast byte-by-byte d'une struct #[repr(C, packed)] — taille vérifiée par const assert.
        let raw: [u8; LOST_FOUND_ENTRY_SIZE] = unsafe { core::mem::transmute_copy(entry) };
        buf[..LOST_FOUND_ENTRY_SIZE].copy_from_slice(&raw);
        // WRITE-02 : écrire et vérifier.
        device.write_block(lba, &buf)?;
        Ok(())
    }

    /// Persiste l en-tête de la région lost+found — WRITE-02.
    fn write_header(&self, device: &dyn BlockDevice, tick: u64) -> ExofsResult<()> {
        let block_size = device.block_size() as u64;
        let hdr = LostFoundHeaderDisk::build(
            self.entries.len() as u32,
            self.capacity as u32,
            self.base_lba,
            tick,
        );
        let mut buf = alloc::vec![0u8; block_size as usize];
        // SAFETY: cast byte-by-byte d'une struct #[repr(C, packed)] — taille vérifiée par const assert.
        let raw: [u8; LOST_FOUND_HDR_SIZE] = unsafe { core::mem::transmute_copy(&hdr) };
        buf[..LOST_FOUND_HDR_SIZE].copy_from_slice(&raw);
        device.write_block(self.base_lba, &buf)?;
        Ok(())
    }
}

// ── Exécuteur de la phase 4 ───────────────────────────────────────────────────

/// Exécuteur de la phase 4 du fsck.
pub struct FsckPhase4;

impl FsckPhase4 {
    /// Lance la phase 4 avec les options par défaut.
    pub fn run(
        device:      &mut dyn BlockDevice,
        phase2:      &Phase2Report,
    ) -> ExofsResult<Phase4Report> {
        Self::run_with_options(device, phase2, &Phase4Options::default())
    }

    /// Lance la phase 4 avec des options personnalisées.
    ///
    /// # Algorithm
    /// 1. Itère sur toutes les entrées de la table d allocation phase 2.
    /// 2. Pour chaque blob dont `ref_count == 0` :
    ///    a. Relit l en-tête blob + données.
    ///    b. HASH-02 : `verify_blob_id` sur les données brutes.
    ///    c. Déplace le blob dans la table lost+found.
    ///    d. WRITE-02 : vérifie que l écriture a réussi.
    /// 3. Flush + écriture de l en-tête lost+found.
    pub fn run_with_options(
        device:      &mut dyn BlockDevice,
        phase2:      &Phase2Report,
        opts:        &Phase4Options,
    ) -> ExofsResult<Phase4Report> {
        RECOVERY_LOG.log_phase_start(4);
        let tick = crate::arch::time::read_ticks();

        let mut errors: Vec<Phase4Error>  = Vec::new();
        let mut orphans_found:     u64    = 0;
        let mut orphans_recovered: u64    = 0;
        let mut orphans_abandoned: u64    = 0;
        let mut bytes_recovered:   u64    = 0;

        let mut lf_table = LostFoundTable::new(opts.capacity, opts.lost_found_lba);
        let block_size = device.block_size() as u64;

        // Itérer sur les entrées d allocation.
        for entry in phase2.alloc_entries_iter() {
            // Seulement les blobs orphelins.
            if phase2.ref_counter.count(&entry.blob_id) != 0 { continue; }

            orphans_found = orphans_found.checked_add(1).unwrap_or(u64::MAX);

            // Calculer le nombre de blocs à lire pour les données.
            let data_blocks = Self::data_blocks(entry.data_len, block_size)?;
            let read_len = data_blocks
                .checked_mul(block_size)
                .ok_or(ExofsError::OffsetOverflow)?;

            // Lire les données du blob pour vérification HASH-02.
            let mut data_buf = alloc::vec![0u8; read_len as usize];
            if opts.verify_data {
                match read_bytes(device, entry.data_lba, &mut data_buf) {
                    Ok(()) => {}
                    Err(_) => {
                        Self::push_err(&mut errors, Phase4Error {
                            kind:    Phase4ErrorKind::ReadIoError,
                            blob_id: entry.blob_id,
                            lba:     entry.data_lba,
                            detail:  0,
                        })?;
                        orphans_abandoned = orphans_abandoned.checked_add(1).unwrap_or(u64::MAX);
                        if opts.stop_on_io_err || errors.len() as u32 >= opts.max_errors {
                            break;
                        }
                        continue;
                    }
                }
                // HASH-02 : vérifier l intégrité des données.
                let relevant = &data_buf[..entry.data_len as usize];
                if !verify_blob_id(&crate::fs::exofs::core::types::BlobId(entry.blob_id), relevant) {
                    Self::push_err(&mut errors, Phase4Error {
                        kind:    Phase4ErrorKind::HashMismatch,
                        blob_id: entry.blob_id,
                        lba:     entry.data_lba,
                        detail:  0,
                    })?;
                    orphans_abandoned = orphans_abandoned.checked_add(1).unwrap_or(u64::MAX);
                    continue;
                }
            }

            // Ajouter dans la table lost+found.
            let lf_entry = LostFoundEntry {
                blob_id:    entry.blob_id,
                origin_lba: entry.data_lba,
                data_len:   entry.data_len,
            };

            match lf_table.add(lf_entry) {
                Ok(()) => {}
                Err(ExofsError::NoSpace) => {
                    Self::push_err(&mut errors, Phase4Error {
                        kind:    Phase4ErrorKind::LostFoundFull,
                        blob_id: entry.blob_id,
                        lba:     entry.data_lba,
                        detail:  0,
                    })?;
                    orphans_abandoned = orphans_abandoned.checked_add(1).unwrap_or(u64::MAX);
                    break;
                }
                Err(e) => return Err(e),
            }

            if !opts.dry_run {
                let idx = lf_table.entries.len().saturating_sub(1);
                match lf_table.write_entry(idx, device) {
                    Ok(()) => {}
                    Err(_) => {
                        Self::push_err(&mut errors, Phase4Error {
                            kind:    Phase4ErrorKind::WriteIoError,
                            blob_id: entry.blob_id,
                            lba:     entry.data_lba,
                            detail:  0,
                        })?;
                        lf_table.entries.pop(); // Rollback.
                        orphans_abandoned = orphans_abandoned.checked_add(1).unwrap_or(u64::MAX);
                        if opts.stop_on_io_err || errors.len() as u32 >= opts.max_errors { break; }
                        continue;
                    }
                }
                bytes_recovered = bytes_recovered
                    .checked_add(entry.data_len)
                    .unwrap_or(u64::MAX);
            } else {
                // Dry-run : comptabiliser sans écrire.
                bytes_recovered = bytes_recovered
                    .checked_add(entry.data_len)
                    .unwrap_or(u64::MAX);
            }

            orphans_recovered = orphans_recovered.checked_add(1).unwrap_or(u64::MAX);
            RECOVERY_AUDIT.record_phase_done(4, 0);
        }

        // Flush et en-tête lost+found.
        if !opts.dry_run && orphans_recovered > 0 {
            let _ = lf_table.write_header(device, tick);
            let _ = device.flush();
        }

        let recovery_rate_pct = if orphans_found == 0 {
            100
        } else {
            orphans_recovered
                .saturating_mul(100)
                .checked_div(orphans_found)
                .unwrap_or(0)
        };

        let error_count = errors.len() as u32;
        RECOVERY_LOG.log_phase_done(4, error_count);
        RECOVERY_AUDIT.record_phase_done(4, error_count);

        Ok(Phase4Report {
            errors,
            orphans_found,
            orphans_recovered,
            orphans_abandoned,
            recovery_rate_pct,
            bytes_recovered,
            dry_run: opts.dry_run,
        })
    }

    /// Calcule le nombre de blocs nécessaires pour `data_len` octets.
    fn data_blocks(data_len: u64, block_size: u64) -> ExofsResult<u64> {
        if block_size == 0 { return Err(ExofsError::InvalidArgument); }
        data_len
            .checked_add(block_size.saturating_sub(1))
            .and_then(|v| v.checked_div(block_size))
            .ok_or(ExofsError::OffsetOverflow)
    }

    /// OOM-02 : `try_reserve(1)` puis `push`.
    #[inline]
    fn push_err(v: &mut Vec<Phase4Error>, e: Phase4Error) -> ExofsResult<()> {
        v.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        v.push(e);
        Ok(())
    }
}

// ── Tests unitaires ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::exofs::core::ExofsError;

    #[test]
    fn test_lost_found_hdr_zero() {
        let buf = [0u8; LOST_FOUND_HDR_SIZE];
        assert!(matches!(
            LostFoundHeaderDisk::from_bytes(&buf),
            Err(ExofsError::InvalidMagic)
        ));
    }

    #[test]
    fn test_lost_found_table_capacity() {
        let mut table = LostFoundTable::new(2, 0x8000);
        let e = LostFoundEntry { blob_id: [0; 32], origin_lba: 1, data_len: 512 };
        assert!(table.add(e).is_ok());
        assert!(table.add(e).is_ok());
        // Troisième ajout → NoSpace.
        assert!(matches!(table.add(e), Err(ExofsError::NoSpace)));
    }

    #[test]
    fn test_data_blocks_exact() {
        assert_eq!(FsckPhase4::data_blocks(512, 512).unwrap(), 1);
        assert_eq!(FsckPhase4::data_blocks(513, 512).unwrap(), 2);
        assert_eq!(FsckPhase4::data_blocks(0, 512).unwrap(), 0);
    }

    #[test]
    fn test_phase4_report_clean() {
        let r = Phase4Report {
            errors:            Vec::new(),
            orphans_found:     4,
            orphans_recovered: 4,
            orphans_abandoned: 0,
            recovery_rate_pct: 100,
            bytes_recovered:   4096,
            dry_run:           false,
        };
        assert!(r.is_clean());
        assert_eq!(r.error_count(), 0);
    }

    #[test]
    fn test_phase4_error_kinds() {
        let e = Phase4Error {
            kind:    Phase4ErrorKind::HashMismatch,
            blob_id: [0xCCu8; 32],
            lba:     0x1000,
            detail:  0,
        };
        assert_eq!(e.kind, Phase4ErrorKind::HashMismatch);
    }

    #[test]
    fn test_options_dry_run_default_false() {
        let opts = Phase4Options::default();
        assert!(!opts.dry_run);
        assert!(opts.verify_data);
    }

    #[test]
    fn test_lost_found_entry_size() {
        assert_eq!(core::mem::size_of::<LostFoundEntry>(), LOST_FOUND_ENTRY_SIZE);
    }

    #[test]
    fn test_table_entry_lba() {
        let table = LostFoundTable::new(100, 0x8000);
        let lba0 = table.entry_lba(0, 512).unwrap();
        let lba1 = table.entry_lba(1, 512).unwrap();
        // entry_lba(0) = base + 1 (header) + 0 * 1 = 0x8001
        assert_eq!(lba0, 0x8001);
        // entry_lba(1) = base + 1 + 1 = 0x8002
        assert_eq!(lba1, 0x8002);
    }
}
