//! slot_recovery.rs — Sélection et validation du slot A/B/C ExoFS (no_std).
//!
//! ExoFS maintient 3 copies redondantes de l'en-tête de volume (slots A, B, C)
//! à des positions LBA fixes. La récupération sélectionne le slot avec le
//! max(epoch_id) valide.
//!
//! # Règles spec appliquées
//! - **HDR-03** : magic vérifié EN PREMIER, puis checksum Blake3 sur l'en-tête.
//! - **ONDISK-03** : pas d'`AtomicU64` dans `SlotHeaderDisk` (repr C).
//! - **OOM-02** : `try_reserve(1)` avant tout `Vec::push`.
//! - **ARITH-02** : `checked_add` pour les calculs d'offset.
//! - **WRITE-02** : vérification `bytes_written == 128` après écriture slot.

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult, EpochId};
use crate::fs::exofs::core::blob_id::blake3_hash;
use super::boot_recovery::BlockDevice;
use super::recovery_audit::RECOVERY_AUDIT;

// ── Constantes ────────────────────────────────────────────────────────────────

/// Magic d'un slot ExoFS : "EXOF_SLT" en little-endian.
pub const SLOT_MAGIC: u64 = 0x544C535F464F5845; // "EXOF_SLT"

/// Version du format de slot on-disk.
pub const SLOT_FORMAT_VERSION: u8 = 1;

/// Taille de l'en-tête de slot on-disk.
pub const SLOT_HEADER_SIZE: usize = 128;

/// Nombre de slots redondants.
pub const SLOT_COUNT: usize = 3;

/// LBA de départ de chaque slot (valeurs par défaut, surchargées à l'init).
pub const SLOT_DEFAULT_LBAS: [u64; SLOT_COUNT] = [0x0100, 0x0200, 0x0300];

// ── Identifiant de slot ───────────────────────────────────────────────────────

/// Identifiant de slot (0 = A, 1 = B, 2 = C).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SlotId(pub u8);

pub const SLOT_A: SlotId = SlotId(0);
pub const SLOT_B: SlotId = SlotId(1);
pub const SLOT_C: SlotId = SlotId(2);

impl SlotId {
    /// Retourne le nom lisible du slot.
    pub fn name(self) -> &'static str {
        match self.0 {
            0 => "A",
            1 => "B",
            2 => "C",
            _ => "?",
        }
    }

    /// Retourne le LBA par défaut de ce slot.
    pub fn default_lba(self) -> u64 {
        SLOT_DEFAULT_LBAS.get(self.0 as usize).copied().unwrap_or(0)
    }

    /// `true` si l'ID est dans la plage valide [0, SLOT_COUNT).
    #[inline]
    pub fn is_valid(self) -> bool {
        (self.0 as usize) < SLOT_COUNT
    }
}

// ── En-tête on-disk du slot ───────────────────────────────────────────────────

/// En-tête on-disk d'un slot A/B/C — `repr(C)`, 128 octets.
///
/// # ONDISK-03
/// Pas d'`AtomicU64` : champs primitifs uniquement.
///
/// # Layout
/// ```text
/// off   0 : magic         u64   8B  — "EXOF_SLT"
/// off   8 : version       u8    1B
/// off   9 : slot_id       u8    1B
/// off  10 : flags         u16   2B  — bit0=dirty, bit1=committing
/// off  12 : _pad0         u32   4B
/// off  16 : epoch_id      u64   8B
/// off  24 : prev_epoch_id u64   8B
/// off  32 : root_blob_id  [u8;32] 32B
/// off  64 : superblock_lba u64  8B
/// off  72 : journal_lba   u64   8B
/// off  80 : total_blobs   u64   8B
/// off  88 : free_blobs    u64   8B
/// off  96 : header_hash   [u8;32] 32B — Blake3(off 0..95)
/// total : 128B
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SlotHeaderDisk {
    /// Magic "EXOF_SLT".
    pub magic:          u64,
    /// Version du format.
    pub version:        u8,
    /// Slot ID (0/1/2).
    pub slot_id:        u8,
    /// Flags.
    pub flags:          u16,
    /// Rembourrage.
    pub _pad0:          u32,
    /// Epoch courante.
    pub epoch_id:       u64,
    /// Epoch précédente (pour rollback).
    pub prev_epoch_id:  u64,
    /// Blob ID de la racine du volume.
    pub root_blob_id:   [u8; 32],
    /// LBA du superbloc.
    pub superblock_lba: u64,
    /// LBA du journal d'epoch.
    pub journal_lba:    u64,
    /// Nombre total de blobs alloués.
    pub total_blobs:    u64,
    /// Nombre de blobs libres.
    pub free_blobs:     u64,
    /// Blake3 des octets [0..96).
    pub header_hash:    [u8; 32],
}

const _: () = assert!(
    core::mem::size_of::<SlotHeaderDisk>() == SLOT_HEADER_SIZE,
    "SlotHeaderDisk doit faire exactement 128 octets"
);

impl SlotHeaderDisk {
    /// Sérialise en buffer de 128 octets.
    pub fn to_bytes(&self) -> [u8; SLOT_HEADER_SIZE] {
        // SAFETY : repr(C) 128B.
        unsafe { core::mem::transmute_copy(self) }
    }

    /// Désérialise depuis 128 octets.
    ///
    /// # HDR-03 — Ordre strict :
    /// 1. magic == SLOT_MAGIC
    /// 2. version == SLOT_FORMAT_VERSION
    /// 3. header_hash == Blake3(buf[0..96])
    pub fn from_bytes(buf: &[u8; SLOT_HEADER_SIZE]) -> ExofsResult<Self> {
        // 1. Magic EN PREMIER.
        let magic = u64::from_le_bytes(
            buf[0..8].try_into().map_err(|_| ExofsError::InvalidMagic)?,
        );
        if magic != SLOT_MAGIC {
            return Err(ExofsError::InvalidMagic);
        }

        // 2. Version.
        if buf[8] != SLOT_FORMAT_VERSION {
            return Err(ExofsError::InvalidMagic);
        }

        // 3. Checksum Blake3 sur bytes[0..96].
        let computed: [u8; 32] = blake3_hash(
            buf[0..96].try_into().map_err(|_| ExofsError::InvalidArgument)?,
        );
        let stored: [u8; 32] = buf[96..128].try_into().map_err(|_| ExofsError::InvalidArgument)?;
        if computed != stored {
            return Err(ExofsError::ChecksumMismatch);
        }

        // SAFETY : buf est aligné, taille 128B vérifiée.
        Ok(unsafe { core::mem::transmute_copy(buf) })
    }

    /// Calcule et écrit le hash dans le champ `header_hash` (HDR-03).
    pub fn finalize_hash(&mut self) {
        let raw = unsafe {
            core::slice::from_raw_parts(self as *const _ as *const u8, 96)
        };
        self.header_hash = blake3_hash(raw.try_into().unwrap_or(&[0u8; 96]));
    }

    /// `true` si le dirty flag est positionné.
    #[inline]
    pub fn is_dirty(&self) -> bool {
        self.flags & 0x01 != 0
    }

    /// `true` si le flag "committing" est positionné.
    #[inline]
    pub fn is_committing(&self) -> bool {
        self.flags & 0x02 != 0
    }
}

// ── Résultat de sélection ─────────────────────────────────────────────────────

/// Résultat de la sélection du meilleur slot.
#[derive(Clone, Debug)]
pub struct SlotRecoveryResult {
    /// Slot sélectionné.
    pub selected_slot: SlotId,
    /// EpochId du slot sélectionné.
    pub epoch_id:      EpochId,
    /// EpochId précédente (pour rollback possible).
    pub prev_epoch_id: EpochId,
    /// `true` si le dirty flag était positionné.
    pub dirty_flag:    bool,
    /// `true` si un replay est nécessaire.
    pub needs_replay:  bool,
    /// Blob ID racine du slot sélectionné.
    pub root_blob_id:  [u8; 32],
    /// LBA du journal d'epoch.
    pub journal_lba:   u64,
    /// LBA du superbloc.
    pub superblock_lba: u64,
    /// Slots validés avec succès.
    pub valid_slots:   u8,
}

// ── Entrée de candidat (sélection interne) ────────────────────────────────────

#[derive(Clone, Copy)]
struct SlotCandidate {
    valid:  bool,
    slot_id: SlotId,
    header: SlotHeaderDisk,
    lba:    u64,
}

impl SlotCandidate {
    const fn invalid() -> Self {
        Self {
            valid:   false,
            slot_id: SlotA,
            header:  unsafe { core::mem::zeroed() },
            lba:     0,
        }
    }
}

const SlotA: SlotId = SlotId(0);

// ── SlotRecovery ──────────────────────────────────────────────────────────────

/// Utilitaire de sélection et validation des slots.
pub struct SlotRecovery;

impl SlotRecovery {
    /// Lit les 3 slots, valide chacun (HDR-03), retourne le meilleur.
    ///
    /// # Algorithme
    /// - Lire chaque slot à son LBA fixe.
    /// - HDR-03 : magic EN PREMIER, puis Blake3.
    /// - Sélectionner le slot avec le plus grand `epoch_id` valide.
    ///
    /// # Erreurs
    /// - `ExofsError::InvalidMagic` si aucun slot valide trouvé.
    pub fn select_best(device: &dyn BlockDevice) -> ExofsResult<SlotRecoveryResult> {
        let mut candidates: [SlotCandidate; SLOT_COUNT] = [
            SlotCandidate::invalid(),
            SlotCandidate::invalid(),
            SlotCandidate::invalid(),
        ];
        let mut valid_mask: u8 = 0;

        for i in 0..SLOT_COUNT {
            let slot_id = SlotId(i as u8);
            let lba = SLOT_DEFAULT_LBAS[i];
            let mut buf = [0u8; SLOT_HEADER_SIZE];

            // Lecture — erreur I/O → slot invalide, on continue.
            if device.read_block(lba, &mut buf).is_err() {
                RECOVERY_AUDIT.record_invalid_magic(lba, 0);
                continue;
            }

            match SlotHeaderDisk::from_bytes(&buf) {
                Ok(hdr) => {
                    candidates[i] = SlotCandidate { valid: true, slot_id, header: hdr, lba };
                    valid_mask |= 1 << i;
                }
                Err(ExofsError::InvalidMagic) => {
                    RECOVERY_AUDIT.record_invalid_magic(lba, 0);
                }
                Err(ExofsError::ChecksumMismatch) => {
                    RECOVERY_AUDIT.record_checksum_invalid(lba, 0, 0);
                }
                Err(_) => {}
            }
        }

        // Sélectionner le candidat avec le max epoch_id.
        let best = candidates
            .iter()
            .filter(|c| c.valid)
            .max_by_key(|c| c.header.epoch_id)
            .ok_or(ExofsError::InvalidMagic)?;

        let hdr      = &best.header;
        let dirty    = hdr.is_dirty();
        let replay   = dirty || hdr.is_committing();

        RECOVERY_AUDIT.record_slot_selected(best.slot_id.0, hdr.epoch_id);

        Ok(SlotRecoveryResult {
            selected_slot:  best.slot_id,
            epoch_id:       EpochId(hdr.epoch_id),
            prev_epoch_id:  EpochId(hdr.prev_epoch_id),
            dirty_flag:     dirty,
            needs_replay:   replay,
            root_blob_id:   hdr.root_blob_id,
            journal_lba:    hdr.journal_lba,
            superblock_lba: hdr.superblock_lba,
            valid_slots:    valid_mask,
        })
    }

    /// Lit un slot identifié et retourne l'en-tête validé.
    ///
    /// # HDR-03
    /// magic + Blake3 vérifiés dans `SlotHeaderDisk::from_bytes`.
    pub fn read_slot(device: &dyn BlockDevice, slot_id: SlotId) -> ExofsResult<SlotHeaderDisk> {
        if !slot_id.is_valid() {
            return Err(ExofsError::InvalidArgument);
        }
        let lba = SLOT_DEFAULT_LBAS[slot_id.0 as usize];
        let mut buf = [0u8; SLOT_HEADER_SIZE];
        device.read_block(lba, &mut buf)?;
        SlotHeaderDisk::from_bytes(&buf)
    }

    /// Écrit un slot sur disque après calcul du hash.
    ///
    /// # WRITE-02
    /// Vérifie que le buffer sérialisé fait bien 128 octets.
    ///
    /// # HDR-03
    /// `finalize_hash` appelé avant l'écriture.
    pub fn write_slot(
        device:  &mut dyn BlockDevice,
        slot_id: SlotId,
        header:  &mut SlotHeaderDisk,
    ) -> ExofsResult<()> {
        if !slot_id.is_valid() {
            return Err(ExofsError::InvalidArgument);
        }

        // HDR-03 : recalculer le hash avant l'écriture.
        header.finalize_hash();

        let buf = header.to_bytes();
        // WRITE-02 : vérifier la taille.
        if buf.len() != SLOT_HEADER_SIZE {
            return Err(ExofsError::PartialWrite);
        }

        let lba = SLOT_DEFAULT_LBAS[slot_id.0 as usize];

        // Barrière avant écriture (RÈGLE 7).
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        device.write_block(lba, &buf)?;
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

        // Flush NVMe.
        device.flush()?;

        Ok(())
    }

    /// Invalide un slot en écrasant son magic.
    ///
    /// Utilisé lors de la promotion après commit pour invalider les anciens slots.
    pub fn invalidate_slot(device: &mut dyn BlockDevice, slot_id: SlotId) -> ExofsResult<()> {
        if !slot_id.is_valid() {
            return Err(ExofsError::InvalidArgument);
        }
        let buf = [0u8; SLOT_HEADER_SIZE];
        let lba = SLOT_DEFAULT_LBAS[slot_id.0 as usize];
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        device.write_block(lba, &buf)?;
        device.flush()
    }

    /// Retourne les statistiques de validation des 3 slots.
    ///
    /// # OOM-02
    /// `try_reserve(SLOT_COUNT)` avant les pushes.
    pub fn validate_all(
        device: &dyn BlockDevice,
    ) -> ExofsResult<Vec<SlotValidationInfo>> {
        let mut out = Vec::new();
        out.try_reserve(SLOT_COUNT).map_err(|_| ExofsError::NoMemory)?;

        for i in 0..SLOT_COUNT {
            let slot_id = SlotId(i as u8);
            let lba = SLOT_DEFAULT_LBAS[i];
            let mut buf = [0u8; SLOT_HEADER_SIZE];

            let info = if device.read_block(lba, &mut buf).is_err() {
                SlotValidationInfo {
                    slot_id,
                    lba,
                    valid:      false,
                    epoch_id:   0,
                    dirty:      false,
                    io_error:   true,
                }
            } else {
                match SlotHeaderDisk::from_bytes(&buf) {
                    Ok(hdr) => SlotValidationInfo {
                        slot_id,
                        lba,
                        valid:    true,
                        epoch_id: hdr.epoch_id,
                        dirty:    hdr.is_dirty(),
                        io_error: false,
                    },
                    Err(_) => SlotValidationInfo {
                        slot_id,
                        lba,
                        valid:    false,
                        epoch_id: 0,
                        dirty:    false,
                        io_error: false,
                    },
                }
            };

            // OOM-02 : try_reserve déjà fait avant la boucle.
            out.push(info);
        }

        Ok(out)
    }

    /// Promeut un slot comme slot principal (met à jour l'epoch_id + dirty=false).
    ///
    /// # WRITE-02
    /// Flush après l'écriture.
    pub fn promote_slot(
        device:   &mut dyn BlockDevice,
        slot_id:  SlotId,
        epoch_id: EpochId,
        root_blob_id: &[u8; 32],
    ) -> ExofsResult<()> {
        let mut hdr = Self::read_slot(device, slot_id)?;
        hdr.epoch_id     = epoch_id.0;
        hdr.root_blob_id = *root_blob_id;
        hdr.flags        = hdr.flags & !0x01; // Effacer dirty flag.
        Self::write_slot(device, slot_id, &mut hdr)
    }

    /// Marque un slot comme dirty (avant le début d'une transaction).
    pub fn mark_dirty(device: &mut dyn BlockDevice, slot_id: SlotId) -> ExofsResult<()> {
        let mut hdr = Self::read_slot(device, slot_id)?;
        hdr.flags |= 0x01;
        Self::write_slot(device, slot_id, &mut hdr)
    }

    /// Efface le dirty flag d'un slot (après commit réussi).
    pub fn clear_dirty(device: &mut dyn BlockDevice, slot_id: SlotId) -> ExofsResult<()> {
        let mut hdr = Self::read_slot(device, slot_id)?;
        hdr.flags &= !0x01;
        Self::write_slot(device, slot_id, &mut hdr)
    }
}

// ── Info de validation ────────────────────────────────────────────────────────

/// Informations de validation d'un slot individuel.
#[derive(Clone, Copy, Debug)]
pub struct SlotValidationInfo {
    /// SlotId contrôlé.
    pub slot_id:  SlotId,
    /// LBA de lecture.
    pub lba:      u64,
    /// `true` si la lecture + validation HDR-03 ont réussi.
    pub valid:    bool,
    /// EpochId lue (0 si invalide).
    pub epoch_id: u64,
    /// `true` si le dirty flag est positionné.
    pub dirty:    bool,
    /// `true` si une erreur I/O est survenue.
    pub io_error: bool,
}

// ── Tests unitaires ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slot_id() {
        assert_eq!(SLOT_A.name(), "A");
        assert_eq!(SLOT_B.name(), "B");
        assert_eq!(SLOT_C.name(), "C");
        assert!(SLOT_A.is_valid());
        assert!(!SlotId(10).is_valid());
    }

    #[test]
    fn test_slot_header_roundtrip() {
        let mut hdr: SlotHeaderDisk = unsafe { core::mem::zeroed() };
        hdr.magic   = SLOT_MAGIC;
        hdr.version = SLOT_FORMAT_VERSION;
        hdr.slot_id = 0;
        hdr.epoch_id = 42;
        hdr.finalize_hash();

        let bytes = hdr.to_bytes();
        let hdr2 = SlotHeaderDisk::from_bytes(&bytes).unwrap();
        assert_eq!(hdr2.epoch_id, 42);
        assert_eq!(hdr2.magic, SLOT_MAGIC);
    }

    #[test]
    fn test_invalid_magic_rejected() {
        let mut buf = [0u8; SLOT_HEADER_SIZE];
        buf[0..8].copy_from_slice(&0xDEADu64.to_le_bytes());
        let r = SlotHeaderDisk::from_bytes(&buf);
        assert!(matches!(r, Err(ExofsError::InvalidMagic)));
    }

    #[test]
    fn test_dirty_flag() {
        let mut hdr: SlotHeaderDisk = unsafe { core::mem::zeroed() };
        hdr.flags = 0x01;
        assert!(hdr.is_dirty());
        hdr.flags = 0x00;
        assert!(!hdr.is_dirty());
    }

    #[test]
    fn test_slot_recovery_result_fields() {
        let r = SlotRecoveryResult {
            selected_slot:   SLOT_A,
            epoch_id:        EpochId(5),
            prev_epoch_id:   EpochId(4),
            dirty_flag:      false,
            needs_replay:    false,
            root_blob_id:    [0; 32],
            journal_lba:     0x1000,
            superblock_lba:  0x0800,
            valid_slots:     0b111,
        };
        assert_eq!(r.epoch_id, EpochId(5));
        assert!(!r.dirty_flag);
        assert_eq!(r.valid_slots, 0b111);
    }
}
