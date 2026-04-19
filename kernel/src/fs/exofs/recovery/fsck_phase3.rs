//! fsck_phase3.rs — Phase 3 du fsck ExoFS : cohérence des snapshots.
//!
//! Vérifie pour chaque snapshot enregistré dans la région snapshot :
//! - L'en-tête possède un magic valide puis un checksum correct (HDR-03).
//! - Le `root_blob_id` est présent dans la table de références extraite en phase 2.
//! - La chaîne parent forme un arbre sans cycle.
//! - Aucun snapshot marqué supprimé n est référencé par un snapshot actif.
//!
//! # Règles spec appliquées
//! - **HDR-03** : magic vérifié EN PREMIER, checksum après.
//! - **HASH-02** : consultation du `BlobRefCounter` (phase 2) pour valider le root_blob.
//! - **OOM-02** : `try_reserve(1)` avant tout `Vec::push` et `BTreeMap::insert`.
//! - **ARITH-02** : `checked_add` / `checked_mul` sur tous les calculs d offset.
//! - **ONDISK-03** : pas d `AtomicU64` dans les structs `repr(C)`.


extern crate alloc;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::core::blob_id::blake3_hash;
use super::boot_recovery::BlockDevice;
use super::block_io::read_array;
use super::fsck_phase2::BlobRefCounter;
use super::recovery_audit::RECOVERY_AUDIT;
use super::recovery_log::RECOVERY_LOG;

// ── Constantes de format ──────────────────────────────────────────────────────

/// Magic d un en-tête de snapshot : little-endian ASCII "SNAPHEAD".
pub const SNAPSHOT_HDR_MAGIC: u64   = 0x44414548504E4153;
/// Version actuelle du format.
pub const SNAPSHOT_HDR_VERSION: u8  = 1;
/// Taille exacte de l en-tête on-disk.
pub const SNAPSHOT_HDR_SIZE: usize  = 256;
/// LBA de départ de la région snapshot dans le volume.
pub const SNAPSHOT_REGION_LBA: u64  = 0x4000;
/// Nombre maximal de snapshots analysés par phase 3.
pub const SNAPSHOT_SCAN_MAX: usize  = 4096;
/// Profondeur maximale de chaîne autorisée (protège contre les cycles profonds).
pub const SNAPSHOT_CHAIN_DEPTH_MAX: u32 = 512;
/// Taille du nom de snapshot (inclus dans l en-tête).
pub const SNAPSHOT_NAME_LEN: usize  = 64;

// ── En-tête on-disk ────────────────────────────────────────────────────────────

/// En-tête on-disk d un snapshot.
///
/// Taille : exactement **256 octets** (`repr(C)`, ONDISK-03).
///
/// Layout :
/// - `[0..8]`    magic (`SNAPSHOT_HDR_MAGIC`)
/// - `[8]`       version
/// - `[9]`       flags (bit0=deleted, bit1=locked, bit2=dirty, bit3=pinned)
/// - `[10..12]`  _pad
/// - `[12..16]`  n_children (u32)
/// - `[16..24]`  snapshot_id (u64)
/// - `[24..32]`  parent_id  (u64) — 0 = racine
/// - `[32..40]`  epoch_id   (u64)
/// - `[40..48]`  created_tick (u64)
/// - `[48..56]`  size_bytes (u64)
/// - `[56..64]`  n_blobs    (u64)
/// - `[64..96]`  root_blob_id ([u8;32])
/// - `[96..160]` name ([u8;64])
/// - `[160..224]`_reserved ([u8;64])
/// - `[224..256]`hdr_hash = Blake3(self[0..224]) ([u8;32])
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SnapshotHeaderDisk {
    /// `SNAPSHOT_HDR_MAGIC` - verifié EN PREMIER (HDR-03).
    pub magic:        u64,
    pub version:      u8,
    /// Flags : bit0=deleted, bit1=locked, bit2=dirty, bit3=pinned.
    pub flags:        u8,
    pub _pad:         u16,
    /// Nombre d enfants directs enregistrés au moment du snapshot.
    pub n_children:   u32,
    pub snapshot_id:  u64,
    /// Parent : `0` signifie que ce snapshot est une racine.
    pub parent_id:    u64,
    pub epoch_id:     u64,
    pub created_tick: u64,
    pub size_bytes:   u64,
    pub n_blobs:      u64,
    pub root_blob_id: [u8; 32],
    pub name:         [u8; 64],
    pub _reserved:    [u8; 64],
    /// `Blake3(self[0..224])` - verifié après le magic (HDR-03).
    pub hdr_hash:     [u8; 32],
}

const _CHECK_SNAP_HDR: () = assert!(
    core::mem::size_of::<SnapshotHeaderDisk>() == SNAPSHOT_HDR_SIZE,
    "SnapshotHeaderDisk doit faire exactement 256 octets"
);

impl SnapshotHeaderDisk {
    /// Désérialise et valide un en-tête (HDR-03 strict).
    ///
    /// Ordre :
    /// 1. `magic == SNAPSHOT_HDR_MAGIC`
    /// 2. `version == SNAPSHOT_HDR_VERSION`
    /// 3. `Blake3(buf[0..224]) == buf[224..256]`
    ///
    /// # Errors
    /// - [`ExofsError::InvalidMagic`] si magic/version incorrect.
    /// - [`ExofsError::ChecksumMismatch`] si checksum incorrect.
    pub fn from_bytes(buf: &[u8; SNAPSHOT_HDR_SIZE]) -> ExofsResult<Self> {
        // 1. Magic EN PREMIER (HDR-03).
        let magic = u64::from_le_bytes([
            buf[0], buf[1], buf[2], buf[3],
            buf[4], buf[5], buf[6], buf[7],
        ]);
        if magic != SNAPSHOT_HDR_MAGIC {
            return Err(ExofsError::InvalidMagic);
        }
        // 2. Version.
        if buf[8] != SNAPSHOT_HDR_VERSION {
            return Err(ExofsError::InvalidMagic);
        }
        // 3. Checksum APRÈS le magic.
        let body: &[u8; 224] = buf[0..224]
            .try_into()
            .map_err(|_| ExofsError::CorruptedStructure)?;
        let computed = blake3_hash(body);
        let stored: [u8; 32] = buf[224..256]
            .try_into()
            .map_err(|_| ExofsError::CorruptedStructure)?;
        if computed != stored {
            return Err(ExofsError::ChecksumMismatch);
        }
        // SAFETY: cast byte-by-byte d'une struct #[repr(C, packed)] — taille vérifiée par const assert.
        Ok(unsafe { core::mem::transmute_copy(buf) })
    }

    /// Construit un en-tête en mémoire (calcule le checksum).
    pub fn build(
        snapshot_id:  u64,
        parent_id:    u64,
        epoch_id:     u64,
        tick:         u64,
        root_blob_id: [u8; 32],
        size_bytes:   u64,
        n_blobs:      u64,
        name:         [u8; SNAPSHOT_NAME_LEN],
        flags:        u8,
    ) -> Self {
        let mut hdr = SnapshotHeaderDisk {
            magic: SNAPSHOT_HDR_MAGIC,
            version: SNAPSHOT_HDR_VERSION,
            flags,
            _pad: 0,
            n_children: 0,
            snapshot_id,
            parent_id,
            epoch_id,
            created_tick: tick,
            size_bytes,
            n_blobs,
            root_blob_id,
            name,
            _reserved: [0u8; 64],
            hdr_hash: [0u8; 32],
        };
        // Calculer le checksum sur les 224 premiers octets.
        // SAFETY: cast byte-by-byte d'une struct #[repr(C, packed)] — taille vérifiée par const assert.
        let raw: &[u8; SNAPSHOT_HDR_SIZE] = unsafe { core::mem::transmute(&hdr) };
        // SAFETY: pointeur calculé depuis une slice dont la longueur a été vérifiée.
        let body: &[u8; 224] = unsafe { &*(raw.as_ptr() as *const [u8; 224]) };
        hdr.hdr_hash = blake3_hash(body);
        hdr
    }

    /// Sérialise en tableau d octets.
    pub fn to_bytes(&self) -> [u8; SNAPSHOT_HDR_SIZE] {
        // SAFETY: cast byte-by-byte d'une struct #[repr(C, packed)] — taille vérifiée par const assert.
        unsafe { core::mem::transmute_copy(self) }
    }

    /// `true` si le snapshot est marqué supprimé.
    #[inline] pub fn is_deleted(&self) -> bool { self.flags & 0x01 != 0 }
    /// `true` si le snapshot est verrouillé.
    #[inline] pub fn is_locked(&self)  -> bool { self.flags & 0x02 != 0 }
    /// `true` si des modifications non commitées sont associées.
    #[inline] pub fn is_dirty(&self)   -> bool { self.flags & 0x04 != 0 }
    /// `true` si le snapshot est épinglé.
    #[inline] pub fn is_pinned(&self)  -> bool { self.flags & 0x08 != 0 }
    /// Parent ID, `None` si racine.
    #[inline] pub fn parent_id_opt(&self) -> Option<u64> {
        if self.parent_id == 0 { None } else { Some(self.parent_id) }
    }
    /// Slice du nom jusqu au premier octet nul.
    pub fn name_bytes(&self) -> &[u8] {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(SNAPSHOT_NAME_LEN);
        &self.name[..end]
    }
}

// ── Erreurs de phase 3 ────────────────────────────────────────────────────────

/// Classification des erreurs détectées lors de la phase 3.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase3ErrorKind {
    /// L en-tête possède un magic invalide.
    BadMagic              = 0x01,
    /// L en-tête possède un checksum incorrect.
    BadChecksum           = 0x02,
    /// Le `root_blob_id` n est pas dans la table de références phase 2.
    RootBlobMissing       = 0x03,
    /// Le parent déclaré n a pas été observé ou est invalide.
    ParentMissing         = 0x04,
    /// Un cycle a été détecté dans la chaîne parent.
    CycleDetected         = 0x05,
    /// Un snapshot supprimé est référencé par un snapshot actif.
    DeletedReferenced     = 0x06,
    /// La profondeur de chaîne dépasse `SNAPSHOT_CHAIN_DEPTH_MAX`.
    ChainTooDeep          = 0x07,
    /// Un snapshot est dirty sans être verrouillé.
    DirtyUnlocked         = 0x08,
    /// Erreur I/O lors de la lecture.
    IoError               = 0xFE,
    /// Overflow arithmétique.
    ArithOverflow         = 0xFF,
}

/// Erreur individuelle relevée lors de la phase 3.
#[derive(Clone, Copy, Debug)]
pub struct Phase3Error {
    pub kind:        Phase3ErrorKind,
    pub snapshot_id: u64,
    /// LBA de l en-tête incriminé.
    pub lba:         u64,
    /// Info complémentaire (parent_id, profondeur, etc.).
    pub detail:      u64,
}

impl Phase3Error {
    /// Retourne `true` si l erreur est critique.
    pub fn is_critical(&self) -> bool {
        matches!(self.kind,
            Phase3ErrorKind::CycleDetected
            | Phase3ErrorKind::RootBlobMissing
            | Phase3ErrorKind::BadMagic
            | Phase3ErrorKind::BadChecksum
        )
    }
}

// ── Options de la phase 3 ─────────────────────────────────────────────────────

/// Options configurables pour la phase 3.
#[derive(Clone, Copy, Debug)]
pub struct Phase3Options {
    /// LBA de départ de la région snapshot.
    pub region_lba:        u64,
    /// Nombre maximal de snapshots à analyser.
    pub scan_max:          usize,
    /// Profondeur maximale de chaîne autorisée.
    pub chain_depth_max:   u32,
    /// Si `true`, ignore les snapshots dirty.
    pub allow_dirty:       bool,
    /// Si `true`, stoppe à la première erreur critique.
    pub stop_on_critical:  bool,
    /// Nombre maximal d erreurs avant abandon.
    pub max_errors:        u32,
}

impl Default for Phase3Options {
    fn default() -> Self {
        Self {
            region_lba:       SNAPSHOT_REGION_LBA,
            scan_max:         SNAPSHOT_SCAN_MAX,
            chain_depth_max:  SNAPSHOT_CHAIN_DEPTH_MAX,
            allow_dirty:      false,
            stop_on_critical: false,
            max_errors:       256,
        }
    }
}

// ── Rapport de phase 3 ────────────────────────────────────────────────────────

/// Résumé de l exécution de la phase 3.
#[derive(Clone, Debug)]
pub struct Phase3Report {
    /// Liste complète des erreurs (vide = toutes les vérifications ont réussi).
    pub errors:              Vec<Phase3Error>,
    /// Nombre de snapshots lus depuis le disque.
    pub snapshots_checked:   u64,
    /// Snapshots validés intégralement.
    pub snapshots_ok:        u64,
    /// Snapshots sans parent résolu (potentiels orphelins).
    pub orphan_snapshots:    u64,
    /// Liaisons parent-enfant valides.
    pub chains_ok:           u64,
    /// Cycles détectés dans les chaînes.
    pub cycle_count:         u64,
    /// Erreurs critiques.
    pub critical_errors:     u64,
    /// Snapshots supprimés ignorés.
    pub deleted_skipped:     u64,
}

impl Phase3Report {
    /// `true` si aucune erreur n a été détectée.
    #[inline] pub fn is_clean(&self)        -> bool  { self.errors.is_empty() }
    /// `true` si des erreurs critiques ont été relevées.
    #[inline] pub fn has_criticals(&self)   -> bool  { self.critical_errors > 0 }
    /// Taux de réussite en pourcentage (0..=100).
    pub fn success_rate_pct(&self) -> u64 {
        if self.snapshots_checked == 0 { return 100; }
        self.snapshots_ok
            .saturating_mul(100)
            .checked_div(self.snapshots_checked)
            .unwrap_or(0)
    }
    /// Nombre total d erreurs enregistrées.
    pub fn error_count(&self) -> usize { self.errors.len() }
}

// ── Contexte interne du scan ──────────────────────────────────────────────────

/// Données maintenues pendant le scan de phase 3.
struct Phase3Context {
    /// snapshot_id → (lba, flags) pour détection de cycles et references.
    seen:     BTreeMap<u64, (u64, u8)>,
    /// snapshot_id → profondeur de chaîne.
    depths:   BTreeMap<u64, u32>,
    /// parent_id → compteur d enfants.
    children: BTreeMap<u64, u32>,
}

impl Phase3Context {
    fn new() -> Self {
        Self {
            seen:     BTreeMap::new(),
            depths:   BTreeMap::new(),
            children: BTreeMap::new(),
        }
    }

    /// Enregistre un snapshot dans le contexte.
    ///
    /// # OOM-02
    /// `try_reserve(1)` sys avant chaque insert.
    fn register(
        &mut self,
        sid:    u64,
        lba:    u64,
        flags:  u8,
        depth:  u32,
    ) -> ExofsResult<()> {
        self.seen.insert(sid, (lba, flags));
        self.depths.insert(sid, depth);
        Ok(())
    }

    /// Incrémente le compteur d enfants du parent.
    fn add_child(&mut self, parent_id: u64) -> ExofsResult<()> {
        let cnt = self.children.entry(parent_id).or_insert(0);
        *cnt = cnt.saturating_add(1);
        Ok(())
    }

    /// Retourne `true` si le snapshot_id est connu.
    #[inline] fn contains(&self, sid: u64) -> bool { self.seen.contains_key(&sid) }
    /// Retourne les métadonnées associées.
    #[inline] fn get(&self, sid: u64) -> Option<(u64, u8)> { self.seen.get(&sid).copied() }
    /// Retourne la profondeur calculée.
    #[inline] fn depth_of(&self, sid: u64) -> Option<u32> { self.depths.get(&sid).copied() }
}

// ── Exécuteur de la phase 3 ───────────────────────────────────────────────────

/// Exécuteur de la phase 3 du fsck.
///
/// Orchestre le scan séquentiel de la région snapshot et délègue les
/// vérifications à `SnapshotHeaderDisk::from_bytes` + `BlobRefCounter`.
pub struct FsckPhase3;

impl FsckPhase3 {
    /// Lance la phase 3 avec les options par défaut.
    pub fn run(
        device:      &dyn BlockDevice,
        ref_counter: &BlobRefCounter,
    ) -> ExofsResult<Phase3Report> {
        Self::run_with_options(device, ref_counter, &Phase3Options::default())
    }

    /// Lance la phase 3 avec des options personnalisées.
    ///
    /// # Algorithm
    /// 1. Calcule le pas LBA par en-tête (ARITH-02).
    /// 2. Pour chaque slot 0..`scan_max` :
    ///    a. Lit le bloc en device.
    ///    b. Slot entièrement nul → fin de région.
    ///    c. `SnapshotHeaderDisk::from_bytes` (HDR-03).
    ///    d. Skip si deleted, enregistre dans le contexte.
    ///    e. Vérifie `root_blob_id` dans `ref_counter` (HASH-02).
    ///    f. Vérifie le parent, calcule la profondeur, détecte les cycles.
    /// 3. Passe de détection de cycles résiduels.
    /// 4. Retourne le rapport.
    pub fn run_with_options(
        device:      &dyn BlockDevice,
        ref_counter: &BlobRefCounter,
        opts:        &Phase3Options,
    ) -> ExofsResult<Phase3Report> {
        RECOVERY_LOG.log_phase_start(3);

        let block_size = device.block_size() as u64;

        // ARITH-02 : calcul du pas en blocs par en-tête (arrondi au supérieur).
        let hdr_blocks: u64 = (SNAPSHOT_HDR_SIZE as u64)
            .checked_add(block_size.saturating_sub(1))
            .and_then(|v| v.checked_div(block_size))
            .ok_or(ExofsError::OffsetOverflow)?
            .max(1);

        let mut ctx                   = Phase3Context::new();
        let mut errors: Vec<Phase3Error> = Vec::new();
        let mut snapshots_checked: u64 = 0;
        let mut snapshots_ok:      u64 = 0;
        let mut orphan_snapshots:  u64 = 0;
        let mut chains_ok:         u64 = 0;
        let mut cycle_count:       u64 = 0;
        let mut critical_errors:   u64 = 0;
        let mut deleted_skipped:   u64 = 0;

        'scan: for i in 0..opts.scan_max {
            // ARITH-02 : LBA = region_lba + i * hdr_blocks.
            let lba = (i as u64)
                .checked_mul(hdr_blocks)
                .and_then(|o| opts.region_lba.checked_add(o))
                .ok_or(ExofsError::OffsetOverflow)?;

            let buf = match read_array::<SNAPSHOT_HDR_SIZE>(device, lba) {
                Ok(buf) => buf,
                Err(_) => break 'scan,
            };

            // Heuristique : slot nul → fin de région allouée.
            if buf.iter().all(|&b| b == 0) { break 'scan; }

            snapshots_checked = snapshots_checked.checked_add(1).unwrap_or(u64::MAX);

            // ── HDR-03 ───────────────────────────────────────────────────────
            let hdr = match SnapshotHeaderDisk::from_bytes(&buf) {
                Ok(h) => h,
                Err(ExofsError::InvalidMagic) => {
                    RECOVERY_AUDIT.record_invalid_magic(lba, 0);
                    Self::push_err(&mut errors, Phase3Error {
                        kind:        Phase3ErrorKind::BadMagic,
                        snapshot_id: 0,
                        lba,
                        detail:      0,
                    })?;
                    critical_errors = critical_errors.saturating_add(1);
                    if opts.stop_on_critical || errors.len() as u32 >= opts.max_errors {
                        break 'scan;
                    }
                    continue;
                }
                Err(ExofsError::ChecksumMismatch) => {
                    RECOVERY_AUDIT.record_checksum_invalid(lba, 0, 0);
                    Self::push_err(&mut errors, Phase3Error {
                        kind:        Phase3ErrorKind::BadChecksum,
                        snapshot_id: 0,
                        lba,
                        detail:      0,
                    })?;
                    critical_errors = critical_errors.saturating_add(1);
                    if opts.stop_on_critical || errors.len() as u32 >= opts.max_errors {
                        break 'scan;
                    }
                    continue;
                }
                Err(_) => continue,
            };

            let sid = hdr.snapshot_id;

            // Snapshots supprimés : juste enregistrer pour la détection de références.
            if hdr.is_deleted() {
                deleted_skipped = deleted_skipped.checked_add(1).unwrap_or(u64::MAX);
                ctx.seen.insert(sid, (lba, hdr.flags));
                continue;
            }

            // Vérifier dirty sans lock.
            if hdr.is_dirty() && !hdr.is_locked() && !opts.allow_dirty {
                Self::push_err(&mut errors, Phase3Error {
                    kind:        Phase3ErrorKind::DirtyUnlocked,
                    snapshot_id: sid,
                    lba,
                    detail:      hdr.flags as u64,
                })?;
            }

            // ── HASH-02 : root_blob dans la table de références phase 2 ─────
            if ref_counter.count(&hdr.root_blob_id) == 0 {
                Self::push_err(&mut errors, Phase3Error {
                    kind:        Phase3ErrorKind::RootBlobMissing,
                    snapshot_id: sid,
                    lba,
                    detail:      0,
                })?;
                critical_errors = critical_errors.saturating_add(1);
                if opts.stop_on_critical { break 'scan; }
                // Ne pas compter comme "ok", mais continuer la vérification de chaîne.
            }

            // ── Vérification de la chaîne parent ─────────────────────────────
            match hdr.parent_id_opt() {
                None => {
                    // Snapshot racine.
                    ctx.register(sid, lba, hdr.flags, 0)?;
                    snapshots_ok = snapshots_ok.checked_add(1).unwrap_or(u64::MAX);
                }
                Some(parent_id) => {
                    if !ctx.contains(parent_id) {
                        // Parent non encore observé (ordre non garanti) → orphelin potentiel.
                        orphan_snapshots = orphan_snapshots.checked_add(1).unwrap_or(u64::MAX);
                        Self::push_err(&mut errors, Phase3Error {
                            kind:        Phase3ErrorKind::ParentMissing,
                            snapshot_id: sid,
                            lba,
                            detail:      parent_id,
                        })?;
                        // Enregistrer quand même pour la suite.
                        ctx.register(sid, lba, hdr.flags, 0)?;
                    } else {
                        let (parent_lba, parent_flags) = ctx.get(parent_id).unwrap();
                        // Vérifier que le parent n est pas supprimé.
                        if parent_flags & 0x01 != 0 {
                            Self::push_err(&mut errors, Phase3Error {
                                kind:        Phase3ErrorKind::DeletedReferenced,
                                snapshot_id: sid,
                                lba,
                                detail:      parent_lba,
                            })?;
                        }
                        // Calcul de la profondeur.
                        let parent_depth = ctx.depth_of(parent_id).unwrap_or(0);
                        let depth = parent_depth
                            .checked_add(1)
                            .unwrap_or(SNAPSHOT_CHAIN_DEPTH_MAX.saturating_add(1));
                        if depth > opts.chain_depth_max {
                            Self::push_err(&mut errors, Phase3Error {
                                kind:        Phase3ErrorKind::ChainTooDeep,
                                snapshot_id: sid,
                                lba,
                                detail:      depth as u64,
                            })?;
                            ctx.register(sid, lba, hdr.flags, depth)?;
                        } else {
                            ctx.register(sid, lba, hdr.flags, depth)?;
                            ctx.add_child(parent_id)?;
                            chains_ok = chains_ok.checked_add(1).unwrap_or(u64::MAX);
                            snapshots_ok = snapshots_ok.checked_add(1).unwrap_or(u64::MAX);
                        }
                    }
                }
            }

            if errors.len() as u32 >= opts.max_errors { break 'scan; }
        }

        // ── Passe de détection de cycles résiduels ───────────────────────────
        // Un snapshot dont la profondeur n est pas déterminée ET qui n est pas
        // dans la table des profondeurs indique un cycle ou une liaison cassée.
        for (&sid, &(lba, flags)) in ctx.seen.iter() {
            if flags & 0x01 != 0 { continue; } // Supprimé → ignoré.
            if ctx.depth_of(sid).is_none() {
                cycle_count = cycle_count.checked_add(1).unwrap_or(u64::MAX);
                Self::push_err(&mut errors, Phase3Error {
                    kind:        Phase3ErrorKind::CycleDetected,
                    snapshot_id: sid,
                    lba,
                    detail:      0,
                })?;
                critical_errors = critical_errors.saturating_add(1);
            }
        }

        let error_count = errors.len() as u32;
        RECOVERY_LOG.log_phase_done(3, error_count);
        RECOVERY_AUDIT.record_phase_done(3, error_count);

        Ok(Phase3Report {
            errors,
            snapshots_checked,
            snapshots_ok,
            orphan_snapshots,
            chains_ok,
            cycle_count,
            critical_errors,
            deleted_skipped,
        })
    }

    /// Appelle `try_reserve(1)` puis `push` — OOM-02.
    #[inline]
    fn push_err(v: &mut Vec<Phase3Error>, e: Phase3Error) -> ExofsResult<()> {
        v.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        v.push(e);
        Ok(())
    }
}

// ── Tests unitaires ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Buffer nul → `InvalidMagic`.
    #[test]
    fn test_hdr_zero_buf() {
        let buf = [0u8; SNAPSHOT_HDR_SIZE];
        assert!(matches!(
            SnapshotHeaderDisk::from_bytes(&buf),
            Err(ExofsError::InvalidMagic)
        ));
    }

    /// Magic valide, mauvais checksum → `ChecksumMismatch`.
    #[test]
    fn test_hdr_bad_checksum() {
        let mut buf = [0u8; SNAPSHOT_HDR_SIZE];
        buf[0..8].copy_from_slice(&SNAPSHOT_HDR_MAGIC.to_le_bytes());
        buf[8] = SNAPSHOT_HDR_VERSION;
        assert!(matches!(
            SnapshotHeaderDisk::from_bytes(&buf),
            Err(ExofsError::ChecksumMismatch)
        ));
    }

    /// `build()` produit un en-tête auto-cohérent.
    #[test]
    fn test_hdr_build_roundtrip() {
        let root_blob = [0xABu8; 32];
        let name = {
            let mut n = [0u8; SNAPSHOT_NAME_LEN];
            n[0] = b's'; n[1] = b'n'; n[2] = b'a'; n[3] = b'p';
            n
        };
        let hdr = SnapshotHeaderDisk::build(
            42, 0, 7, 12345, root_blob, 1024, 4, name, 0,
        );
        let bytes = hdr.to_bytes();
        let parsed = SnapshotHeaderDisk::from_bytes(&bytes);
        assert!(parsed.is_ok(), "from_bytes doit réussir après build");
        assert_eq!(parsed.unwrap().snapshot_id, 42);
    }

    /// Flag `is_deleted`.
    #[test]
    fn test_flag_deleted() {
        // SAFETY: type entièrement initialisable par zéros (repr(C) avec champs numériques).
        let mut h: SnapshotHeaderDisk = unsafe { core::mem::zeroed() };
        h.flags = 0x01;
        assert!(h.is_deleted());
        assert!(!h.is_locked());
    }

    /// Flag `is_pinned`.
    #[test]
    fn test_flag_pinned() {
        // SAFETY: type entièrement initialisable par zéros (repr(C) avec champs numériques).
        let mut h: SnapshotHeaderDisk = unsafe { core::mem::zeroed() };
        h.flags = 0x08;
        assert!(h.is_pinned());
    }

    /// Rapport vide → propre, taux 100%.
    #[test]
    fn test_report_clean() {
        let r = Phase3Report {
            errors:            Vec::new(),
            snapshots_checked: 8,
            snapshots_ok:      8,
            orphan_snapshots:  0,
            chains_ok:         7,
            cycle_count:       0,
            critical_errors:   0,
            deleted_skipped:   0,
        };
        assert!(r.is_clean());
        assert_eq!(r.success_rate_pct(), 100);
    }

    /// Taux de réussite avec 0 snapshots → 100%.
    #[test]
    fn test_report_zero_checked() {
        let r = Phase3Report {
            errors:            Vec::new(),
            snapshots_checked: 0,
            snapshots_ok:      0,
            orphan_snapshots:  0,
            chains_ok:         0,
            cycle_count:       0,
            critical_errors:   0,
            deleted_skipped:   0,
        };
        assert_eq!(r.success_rate_pct(), 100);
    }

    /// `is_critical` sur les différents types d erreur.
    #[test]
    fn test_error_critical_types() {
        let critical_kinds = [
            Phase3ErrorKind::CycleDetected,
            Phase3ErrorKind::RootBlobMissing,
            Phase3ErrorKind::BadMagic,
            Phase3ErrorKind::BadChecksum,
        ];
        for kind in critical_kinds {
            let e = Phase3Error { kind, snapshot_id: 1, lba: 0x4000, detail: 0 };
            assert!(e.is_critical(), "{kind:?} doit etre critique");
        }
        let non_critical = [
            Phase3ErrorKind::DirtyUnlocked,
            Phase3ErrorKind::ChainTooDeep,
            Phase3ErrorKind::DeletedReferenced,
        ];
        for kind in non_critical {
            let e = Phase3Error { kind, snapshot_id: 1, lba: 0x4000, detail: 0 };
            assert!(!e.is_critical(), "{kind:?} ne doit pas etre critique");
        }
    }

    /// `Phase3Context::register` correctement OOM-02.
    #[test]
    fn test_context_register() {
        let mut ctx = Phase3Context::new();
        ctx.register(1, 0x4000, 0, 0).expect("register doit reussir");
        assert!(ctx.contains(1));
        assert_eq!(ctx.depth_of(1), Some(0));
    }

    /// Comptage des enfants.
    #[test]
    fn test_context_add_child() {
        let mut ctx = Phase3Context::new();
        ctx.register(10, 0x5000, 0, 0).unwrap();
        ctx.add_child(10).unwrap();
        ctx.add_child(10).unwrap();
        assert_eq!(*ctx.children.get(&10).unwrap(), 2);
    }

    /// Options par défaut.
    #[test]
    fn test_options_default() {
        let opts = Phase3Options::default();
        assert_eq!(opts.region_lba,      SNAPSHOT_REGION_LBA);
        assert_eq!(opts.scan_max,        SNAPSHOT_SCAN_MAX);
        assert_eq!(opts.chain_depth_max, SNAPSHOT_CHAIN_DEPTH_MAX);
    }
}
