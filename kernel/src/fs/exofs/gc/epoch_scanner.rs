//! Scanner d'epochs pour le GC ExoFS.
//!
//! Parcourt la chaîne EpochRoot → EpochRecord → BlobIds pour construire
//! l'ensemble des blobs vivants à l'issue d'une passe de marquage.
//!
//! RÈGLE 13 : n'acquiert jamais EPOCH_COMMIT_LOCK.
//! RÈGLE 8  : vérifie le magic EN PREMIER dans tout parsing on-disk.
//! RÈGLE 14 : checked_add() pour tous calculs d'offset.

use alloc::vec::Vec;
use core::sync::atomic::Ordering;

use crate::fs::exofs::core::{BlobId, EpochId, FsError};
use crate::fs::exofs::epoch::{EpochRoot, EpochRecord, EPOCH_MAGIC};
use crate::fs::exofs::gc::tricolor::{BlobIndex, TricolorSet};
use crate::fs::exofs::storage::SuperBlock;

/// Résultat d'un scan d'epoch.
pub struct ScanResult {
    /// Index de tous les blobs trouvés vivants.
    pub index: BlobIndex,
    /// Ensemble tricolore initialisé avec les racines en gris.
    pub colors: TricolorSet,
    /// Epoch la plus ancienne scannée.
    pub min_epoch: EpochId,
    /// Epoch la plus récente scannée.
    pub max_epoch: EpochId,
}

/// Scanner itérant sur la chaîne d'epochs depuis le SuperBlock.
pub struct EpochScanner<'sb> {
    superblock: &'sb SuperBlock,
}

impl<'sb> EpochScanner<'sb> {
    pub fn new(superblock: &'sb SuperBlock) -> Self {
        Self { superblock }
    }

    /// Lance un scan complet de la chaîne d'epochs.
    ///
    /// Retourne un `ScanResult` prêt pour la phase Marking.
    pub fn scan_all(&self) -> Result<ScanResult, FsError> {
        let mut index = BlobIndex::new();
        let mut roots: Vec<(EpochId, u64)> = Vec::new(); // (epoch_id, disk_lba)

        // Collecte tous les EpochRoot depuis le superblock.
        self.superblock.iter_epoch_roots(|epoch_id, lba| {
            roots
                .try_reserve(1)
                .map_err(|_| FsError::OutOfMemory)?;
            roots.push((epoch_id, lba));
            Ok(())
        })?;

        if roots.is_empty() {
            // Aucune epoch : ensemble vide → GC ne libère rien.
            let colors = TricolorSet::new(0)?;
            return Ok(ScanResult {
                index,
                colors,
                min_epoch: EpochId(0),
                max_epoch: EpochId(0),
            });
        }

        let min_epoch = roots.iter().map(|(e, _)| *e).min().unwrap_or(EpochId(0));
        let max_epoch = roots.iter().map(|(e, _)| *e).max().unwrap_or(EpochId(0));

        // Première passe : collecter tous les BlobIds pour dimensionner l'index.
        for (_, lba) in &roots {
            self.scan_epoch_root_at(*lba, &mut index)?;
        }

        // Crée l'ensemble tricolore de la bonne taille.
        let n = index.len();
        let mut colors = TricolorSet::new(n)?;

        // Marque toutes les racines directement référencées comme grises.
        for i in 0..n {
            colors.mark_grey(i);
        }

        Ok(ScanResult { index, colors, min_epoch, max_epoch })
    }

    // -----------------------------------------------------------------------
    // Interne
    // -----------------------------------------------------------------------

    fn scan_epoch_root_at(
        &self,
        lba: u64,
        index: &mut BlobIndex,
    ) -> Result<(), FsError> {
        let root_bytes = self.superblock.read_block(lba)?;

        // RÈGLE 8 : magic EN PREMIER.
        let magic = u32::from_le_bytes(
            root_bytes
                .get(0..4)
                .and_then(|b| b.try_into().ok())
                .ok_or(FsError::CorruptData)?,
        );
        if magic != EPOCH_MAGIC {
            return Err(FsError::BadMagic);
        }

        let root = EpochRoot::from_bytes(&root_bytes)?;

        // Parcourt les records référencés par cette root.
        for record_lba in root.record_lbas() {
            self.scan_epoch_record_at(record_lba, index)?;
        }
        Ok(())
    }

    fn scan_epoch_record_at(
        &self,
        lba: u64,
        index: &mut BlobIndex,
    ) -> Result<(), FsError> {
        let rec_bytes = self.superblock.read_block(lba)?;

        // RÈGLE 8 : magic EN PREMIER.
        let magic = u32::from_le_bytes(
            rec_bytes
                .get(0..4)
                .and_then(|b| b.try_into().ok())
                .ok_or(FsError::CorruptData)?,
        );
        if magic != EPOCH_MAGIC {
            return Err(FsError::BadMagic);
        }

        let record = EpochRecord::from_bytes(&rec_bytes)?;

        for blob_id in record.blob_ids() {
            index.register(blob_id)?;
        }
        Ok(())
    }
}
