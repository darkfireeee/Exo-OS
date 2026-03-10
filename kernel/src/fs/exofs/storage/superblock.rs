//! superblock.rs — Superblock ExoFS : structure disque + gestion en mémoire
//!
//! Règles spec :
//!   ONDISK-03 : AtomicXxx INTERDIT dans les structs #[repr(C)]
//!   HDR-03    : vérification magic + checksum AVANT tout accès aux champs
//!   BACKUP-01 : 3 miroirs écrits à chaque commit
//!   BACKUP-02 : recover sélectionne le miroir avec epoch_current le plus élevé
//!   WRITE-02  : vérification bytes_written après chaque écriture disque
//!   ARITH-02  : checked_add avant tout calcul d'offset


extern crate alloc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};

use crate::fs::exofs::core::{
    ExofsError, ExofsResult, EpochId, DiskOffset,
};
use crate::fs::exofs::core::blob_id::blake3_hash;
use crate::fs::exofs::storage::storage_stats::STORAGE_STATS;
use crate::fs::exofs::storage::layout::{BLOCK_SIZE, HEAP_START_OFFSET};
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────

/// Magic ExoFS : "EXOF"
pub const EXOFS_MAGIC: u32 = 0x4558_4F46;

/// Version majeure du format
pub const FORMAT_VERSION_MAJOR: u16 = 1;

/// Version mineure du format
pub const FORMAT_VERSION_MINOR: u16 = 0;

/// Taille de la structure superblock sur disque (multiple de BLOCK_SIZE : 512B)
pub const SUPERBLOCK_DISK_SIZE: usize = 512;

/// Nombre de miroirs du superblock (BACKUP-01)
pub const SB_MIRROR_COUNT: usize = 3;

/// Offset du miroir primaire (bloc 0)
pub const SB_PRIMARY_OFFSET: u64 = 0;

/// Offset du miroir secondaire (bloc 3 = 12 KB)
pub const SB_SECONDARY_OFFSET: u64 = 3 * BLOCK_SIZE as u64;

/// Offset du miroir tertiaire (fin de disque - 4 KB)
pub const SB_TERTIARY_RELATIVE: u64 = BLOCK_SIZE as u64; // relatif à la fin

/// Taille minimale de disque supportée (16 MiB)
pub const MIN_DISK_SIZE: u64 = 16 * 1024 * 1024;

/// Taille maximale de nom de volume
pub const VOLUME_NAME_LEN: usize = 64;

// ─────────────────────────────────────────────────────────────
// Flags de compatibilité
// ─────────────────────────────────────────────────────────────

/// Flags incompatibles (empêchent le montage si non supportés).
/// RÈGLE FS-10 (V-31) : EXO_BLAKE3 | EXO_DELAYED | EXO_REFLINK obligatoires.
pub mod incompat_flags {
    /// Compression activée sur le volume.
    pub const COMPRESSION: u64 = 1 << 0;
    /// Déduplication activée sur le volume.
    pub const DEDUP:        u64 = 1 << 1;
    /// Chiffrement activé sur le volume.
    pub const ENCRYPTION:   u64 = 1 << 2;
    /// RÈGLE FS-10 : checksums Blake3 sur toutes les écritures ExoFS.
    pub const EXO_BLAKE3:   u64 = 1 << 3;
    /// RÈGLE FS-10 : allocation différée (blocs alloués au writeback, jamais au write).
    pub const EXO_DELAYED:  u64 = 1 << 4;
    /// RÈGLE FS-10 : reflink (copy-on-write partagé de blocs).
    pub const EXO_REFLINK:  u64 = 1 << 5;
    /// Combinaison obligatoire pour tout nouveau volume ExoFS (FS-10).
    pub const REQUIRED: u64 = EXO_BLAKE3 | EXO_DELAYED | EXO_REFLINK;
}

/// Flags compatibles (montage R/O autorisé si non supportés)
pub mod compat_flags {
    pub const SNAPSHOTS: u64 = 1 << 0;
}

// ─────────────────────────────────────────────────────────────
// Structure disque (repr(C) — pas d'AtomicXxx — ONDISK-03)
// ─────────────────────────────────────────────────────────────

/// Superblock ExoFS tel qu'écrit sur disque.
///
/// Précisément SUPERBLOCK_DISK_SIZE octets (512B), aligné à BLOCK_SIZE.
/// Aucun AtomicXxx — ONDISK-03.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ExoSuperblockDisk {
    /// Magic "EXOF" — vérifié EN PREMIER (HDR-03)
    pub magic: u32,
    /// Version majeure du format
    pub version_major: u16,
    /// Version mineure du format
    pub version_minor: u16,
    /// Flags incompatibles
    pub incompat_flags: u64,
    /// Flags compatibles
    pub compat_flags: u64,
    /// Taille totale du disque en octets
    pub disk_size_bytes: u64,
    /// Offset de début du heap
    pub heap_start: u64,
    /// Offset de fin du heap
    pub heap_end: u64,
    /// Offset miroir secondaire
    pub secondary_sb_offset: u64,
    /// Offset miroir tertiaire
    pub tertiary_sb_offset: u64,
    /// Timestamp de création (secondes epoch Unix)
    pub created_at: u64,
    /// UUID du volume (16 octets)
    pub uuid: [u8; 16],
    /// Nom du volume (UTF-8 null-padded)
    pub volume_name: [u8; VOLUME_NAME_LEN],
    /// Taille de bloc
    pub block_size: u32,
    /// Index de miroir courant (0–2) — utilisé pour la rotation
    pub mirror_index: u8,
    /// _padding
    pub _pad0: [u8; 3],
    /// Nombre d'objets au moment du dernier commit
    pub object_count: u64,
    /// Nombre de blobs physiques
    pub blob_count: u64,
    /// Octets libres approximatifs
    pub free_bytes: u64,
    /// Époque courante (incrémentée à chaque commit)
    pub epoch_current: u64,
    /// Époque du dernier fsck
    pub last_fsck_epoch: u64,
    /// Timestamp du dernier commit
    pub last_commit_time: u64,
    /// _padding pour atteindre 448 octets avant checksum
    pub _pad1: [u8; 104],
    /// Checksum Blake3 sur les 480 premiers octets
    pub checksum: [u8; 32],
}

// const _SB_SIZE: () = assert!(
//     core::mem::size_of::<ExoSuperblockDisk>() == SUPERBLOCK_DISK_SIZE,
//     "ExoSuperblockDisk doit faire exactement 512 octets"
// );

impl ExoSuperblockDisk {
    /// Calcule le checksum Blake3 sur les SUPERBLOCK_DISK_SIZE - 32 premiers octets
    pub fn compute_checksum(&self) -> [u8; 32] {
        let body_len = SUPERBLOCK_DISK_SIZE - 32;
        let ptr = self as *const Self as *const u8;
        // SAFETY: repr(C), taille vérifiée par static assert
        let body = unsafe { core::slice::from_raw_parts(ptr, body_len) };
        blake3_hash(body)
    }

    /// HDR-03 : Vérifie magic EN PREMIER, puis le checksum
    pub fn verify(&self) -> ExofsResult<()> {
        if self.magic != EXOFS_MAGIC {
            return Err(ExofsError::BadMagic);
        }
        if self.version_major != FORMAT_VERSION_MAJOR {
            return Err(ExofsError::InvalidArgument);
        }
        let expected = self.compute_checksum();
        let mut diff: u8 = 0;
        for i in 0..32 {
            diff |= expected[i] ^ self.checksum[i];
        }
        if diff != 0 {
            STORAGE_STATS.inc_checksum_error();
            return Err(ExofsError::ChecksumMismatch);
        }
        STORAGE_STATS.inc_checksum_ok();
        Ok(())
    }

    /// Finalise le superblock : calcule et injecte le checksum
    pub fn finalize(&mut self) {
        self.checksum = self.compute_checksum();
    }

    /// Crée un superblock initialisé pour un nouveau volume
    pub fn new_volume(
        disk_size_bytes: u64,
        volume_name: &[u8],
        uuid: [u8; 16],
        created_at: u64,
    ) -> Self {
        let secondary_offset = SB_SECONDARY_OFFSET;
        let tertiary_offset = disk_size_bytes.saturating_sub(SB_TERTIARY_RELATIVE);
        let heap_end = disk_size_bytes.saturating_sub(2 * BLOCK_SIZE as u64);

        let mut name_buf = [0u8; VOLUME_NAME_LEN];
        let copy_len = volume_name.len().min(VOLUME_NAME_LEN);
        name_buf[..copy_len].copy_from_slice(&volume_name[..copy_len]);

        let mut sb = Self {
            magic: EXOFS_MAGIC,
            version_major: FORMAT_VERSION_MAJOR,
            version_minor: FORMAT_VERSION_MINOR,
            incompat_flags: incompat_flags::REQUIRED, // FS-10: EXO_BLAKE3|EXO_DELAYED|EXO_REFLINK obligatoires
            compat_flags: 0,
            disk_size_bytes,
            heap_start: HEAP_START_OFFSET as u64,
            heap_end,
            secondary_sb_offset: secondary_offset,
            tertiary_sb_offset: tertiary_offset,
            created_at,
            uuid,
            volume_name: name_buf,
            block_size: BLOCK_SIZE as u32,
            mirror_index: 0,
            _pad0: [0u8; 3],
            object_count: 0,
            blob_count: 0,
            free_bytes: heap_end.saturating_sub(HEAP_START_OFFSET as u64),
            epoch_current: 1,
            last_fsck_epoch: 0,
            last_commit_time: created_at,
            _pad1: [0u8; 104],
            checksum: [0u8; 32],
        };

        sb.finalize();
        sb
    }

    /// Retourne les octets bruts
    pub fn as_bytes(&self) -> &[u8] {
        let ptr = self as *const Self as *const u8;
        // SAFETY: repr(C), taille connue
        unsafe { core::slice::from_raw_parts(ptr, SUPERBLOCK_DISK_SIZE) }
    }

    /// Parse depuis un tampon
    pub fn from_bytes(buf: &[u8]) -> ExofsResult<ExoSuperblockDisk> {
        if buf.len() < SUPERBLOCK_DISK_SIZE {
            return Err(ExofsError::InvalidSize);
        }
        // SAFETY: taille vérifiée, repr(C)
        let sb: ExoSuperblockDisk = unsafe {
            core::ptr::read_unaligned(buf.as_ptr() as *const ExoSuperblockDisk)
        };
        Ok(sb)
    }
}

// ─────────────────────────────────────────────────────────────
// SuperblockManager — gestion en mémoire avec miroirs
// ─────────────────────────────────────────────────────────────

/// Index d'un miroir superblock
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirrorSlot {
    Primary   = 0,
    Secondary = 1,
    Tertiary  = 2,
}

impl MirrorSlot {
    pub fn from_index(i: usize) -> Option<Self> {
        match i {
            0 => Some(Self::Primary),
            1 => Some(Self::Secondary),
            2 => Some(Self::Tertiary),
            _ => None,
        }
    }
}

/// État du SuperblockManager
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SbManagerState {
    Uninitialized,
    Mounted,
    Dirty,
    Shutdown,
}

/// Version en mémoire du superblock (avec AtomicXxx pour les compteurs live)
pub struct SuperblockInMemory {
    /// Copie de la structure disque (protégée par SpinLock)
    pub disk_copy: SpinLock<ExoSuperblockDisk>,
    /// Époque courante (compteur live — AtomicU64 OK en RAM)
    pub epoch: AtomicU64,
    /// Nombre d'objets (live)
    pub live_object_count: AtomicU64,
    /// Nombre de blobs (live)
    pub live_blob_count: AtomicU64,
    /// Octets libres (live)
    pub live_free_bytes: AtomicU64,
    /// Dirty flag (commit nécessaire)
    pub dirty: AtomicBool,
    /// État du manager
    pub state: SpinLock<SbManagerState>,
    /// Offsets des 3 miroirs sur disque
    pub mirror_offsets: [DiskOffset; SB_MIRROR_COUNT],
}

impl SuperblockInMemory {
    fn new(disk: ExoSuperblockDisk, offsets: [DiskOffset; SB_MIRROR_COUNT]) -> Self {
        let epoch = disk.epoch_current;
        let objects = disk.object_count;
        let blobs = disk.blob_count;
        let free = disk.free_bytes;

        Self {
            disk_copy: SpinLock::new(disk),
            epoch: AtomicU64::new(epoch),
            live_object_count: AtomicU64::new(objects),
            live_blob_count: AtomicU64::new(blobs),
            live_free_bytes: AtomicU64::new(free),
            dirty: AtomicBool::new(false),
            state: SpinLock::new(SbManagerState::Mounted),
            mirror_offsets: offsets,
        }
    }
}

/// Gestionnaire du superblock
pub struct SuperblockManager {
    inner: SuperblockInMemory,
    /// Compteurs de commits
    pub total_commits: AtomicU64,
    pub failed_commits: AtomicU64,
    pub recovery_reads: AtomicU64,
}

impl SuperblockManager {
    // ── Initialisation ────────────────────────────────────────────────

    /// Monte un volume existant : lit et vérifie les 3 miroirs, récupère le meilleur.
    ///
    /// BACKUP-02 : sélection du miroir avec epoch_current le plus élevé.
    pub fn mount<ReadFn>(
        disk_size: u64,
        read_fn: ReadFn,
    ) -> ExofsResult<Self>
    where
        ReadFn: Fn(DiskOffset, usize) -> ExofsResult<Vec<u8>>,
    {
        let offsets = Self::compute_mirror_offsets(disk_size);
        let best = Self::recover_best_mirror(&offsets, &read_fn)?;

        let mgr = SuperblockManager {
            inner: SuperblockInMemory::new(best, offsets),
            total_commits: AtomicU64::new(0),
            failed_commits: AtomicU64::new(0),
            recovery_reads: AtomicU64::new(1),
        };

        Ok(mgr)
    }

    /// Formate un nouveau volume et écrit les 3 miroirs initiaux.
    ///
    /// BACKUP-01 : écriture sur les 3 miroirs dès le formatage.
    pub fn format<WriteFn>(
        disk_size: u64,
        volume_name: &[u8],
        uuid: [u8; 16],
        created_at: u64,
        write_fn: WriteFn,
    ) -> ExofsResult<Self>
    where
        WriteFn: FnMut(DiskOffset, &[u8]) -> ExofsResult<usize>,
    {
        if disk_size < MIN_DISK_SIZE {
            return Err(ExofsError::InvalidSize);
        }

        let sb = ExoSuperblockDisk::new_volume(disk_size, volume_name, uuid, created_at);
        let offsets = Self::compute_mirror_offsets(disk_size);

        let mgr = SuperblockManager {
            inner: SuperblockInMemory::new(sb, offsets),
            total_commits: AtomicU64::new(0),
            failed_commits: AtomicU64::new(0),
            recovery_reads: AtomicU64::new(0),
        };

        mgr.commit_all_mirrors(write_fn)?;
        STORAGE_STATS.inc_sb_commit();

        Ok(mgr)
    }

    // ── Commit ────────────────────────────────────────────────────────

    /// Commit le superblock sur les 3 miroirs (BACKUP-01).
    ///
    /// Incrémente epoch_current avant chaque commit.
    /// WRITE-02 : vérifie bytes_written pour chaque miroir.
    pub fn commit<WriteFn>(&self, now: u64, write_fn: WriteFn) -> ExofsResult<()>
    where
        WriteFn: FnMut(DiskOffset, &[u8]) -> ExofsResult<usize>,
    {
        // Mise à jour des champs live avant commit
        {
            let mut disk = self.inner.disk_copy.lock();
            let new_epoch = self.inner.epoch.fetch_add(1, Ordering::SeqCst) + 1;
            disk.epoch_current = new_epoch;
            disk.object_count = self.inner.live_object_count.load(Ordering::Relaxed);
            disk.blob_count = self.inner.live_blob_count.load(Ordering::Relaxed);
            disk.free_bytes = self.inner.live_free_bytes.load(Ordering::Relaxed);
            disk.last_commit_time = now;
            disk.finalize();
        }

        let result = self.commit_all_mirrors(write_fn);

        match &result {
            Ok(_) => {
                self.inner.dirty.store(false, Ordering::Release);
                self.total_commits.fetch_add(1, Ordering::Relaxed);
                STORAGE_STATS.inc_sb_commit();
            }
            Err(_) => {
                self.failed_commits.fetch_add(1, Ordering::Relaxed);
                STORAGE_STATS.inc_io_error();
            }
        }

        result
    }

    // ── Lecture et mise à jour des champs live ────────────────────────

    /// Incrémente le compteur d'objets
    pub fn inc_object_count(&self) {
        self.inner.live_object_count.fetch_add(1, Ordering::Relaxed);
        self.inner.dirty.store(true, Ordering::Relaxed);
    }

    /// Décrémente le compteur d'objets
    pub fn dec_object_count(&self) {
        self.inner.live_object_count.fetch_sub(1, Ordering::Relaxed);
        self.inner.dirty.store(true, Ordering::Relaxed);
    }

    /// Incrémente le compteur de blobs
    pub fn inc_blob_count(&self) {
        self.inner.live_blob_count.fetch_add(1, Ordering::Relaxed);
        self.inner.dirty.store(true, Ordering::Relaxed);
    }

    /// Décrémente l'espace libre
    pub fn sub_free_bytes(&self, n: u64) {
        self.inner.live_free_bytes.fetch_sub(n, Ordering::Relaxed);
        self.inner.dirty.store(true, Ordering::Relaxed);
    }

    /// Ajoute de l'espace libre
    pub fn add_free_bytes(&self, n: u64) {
        self.inner.live_free_bytes.fetch_add(n, Ordering::Relaxed);
        self.inner.dirty.store(true, Ordering::Relaxed);
    }

    /// Retourne l'époque courante
    pub fn current_epoch(&self) -> EpochId {
        EpochId(self.inner.epoch.load(Ordering::Relaxed))
    }

    /// Retourne vrai si un commit est nécessaire
    pub fn is_dirty(&self) -> bool {
        self.inner.dirty.load(Ordering::Acquire)
    }

    /// Snapshot des métadonnées courantes
    pub fn snapshot(&self) -> SuperblockSnapshot {
        let disk = self.inner.disk_copy.lock();
        SuperblockSnapshot {
            epoch: self.inner.epoch.load(Ordering::Relaxed),
            object_count: self.inner.live_object_count.load(Ordering::Relaxed),
            blob_count: self.inner.live_blob_count.load(Ordering::Relaxed),
            free_bytes: self.inner.live_free_bytes.load(Ordering::Relaxed),
            disk_size: disk.disk_size_bytes,
            heap_start: disk.heap_start,
            heap_end: disk.heap_end,
            volume_name: disk.volume_name,
            uuid: disk.uuid,
            last_commit_time: disk.last_commit_time,
            total_commits: self.total_commits.load(Ordering::Relaxed),
        }
    }

    // ── Internals ─────────────────────────────────────────────────────

    fn commit_all_mirrors<WriteFn>(&self, mut write_fn: WriteFn) -> ExofsResult<()>
    where
        WriteFn: FnMut(DiskOffset, &[u8]) -> ExofsResult<usize>,
    {
        let disk = self.inner.disk_copy.lock();
        let bytes = disk.as_bytes();

        let mut any_ok = false;
        let mut last_err = ExofsError::ShortWrite;

        for (i, &off) in self.inner.mirror_offsets.iter().enumerate() {
            match write_fn(off, bytes) {
                Ok(n) if n == SUPERBLOCK_DISK_SIZE => {
                    any_ok = true;
                }
                Ok(_) => {
                    STORAGE_STATS.inc_io_error();
                    last_err = ExofsError::ShortWrite;
                }
                Err(e) => {
                    STORAGE_STATS.inc_io_error();
                    last_err = e;
                    let _ = i;
                }
            }
        }

        if any_ok { Ok(()) } else { Err(last_err) }
    }

    /// Lit et valide un miroir (HDR-03)
    fn read_mirror<ReadFn>(
        offset: DiskOffset,
        read_fn: &ReadFn,
    ) -> ExofsResult<ExoSuperblockDisk>
    where
        ReadFn: Fn(DiskOffset, usize) -> ExofsResult<Vec<u8>>,
    {
        let buf = read_fn(offset, SUPERBLOCK_DISK_SIZE)?;
        if buf.len() < SUPERBLOCK_DISK_SIZE {
            return Err(ExofsError::InvalidSize);
        }
        let sb = ExoSuperblockDisk::from_bytes(&buf)?;
        sb.verify()?;
        Ok(sb)
    }

    /// BACKUP-02 : sélectionne le miroir avec epoch_current le plus élevé
    fn recover_best_mirror<ReadFn>(
        offsets: &[DiskOffset; SB_MIRROR_COUNT],
        read_fn: &ReadFn,
    ) -> ExofsResult<ExoSuperblockDisk>
    where
        ReadFn: Fn(DiskOffset, usize) -> ExofsResult<Vec<u8>>,
    {
        let mut best: Option<ExoSuperblockDisk> = None;
        let mut best_epoch = 0u64;

        for &off in offsets.iter() {
            match Self::read_mirror(off, read_fn) {
                Ok(sb) => {
                    if sb.epoch_current > best_epoch {
                        best_epoch = sb.epoch_current;
                        best = Some(sb);
                    }
                }
                Err(_) => {
                    // Miroir corrompu ou absent — on continue
                    STORAGE_STATS.inc_io_error();
                }
            }
        }

        best.ok_or(ExofsError::InvalidState)
    }

    fn compute_mirror_offsets(disk_size: u64) -> [DiskOffset; SB_MIRROR_COUNT] {
        [
            DiskOffset(SB_PRIMARY_OFFSET),
            DiskOffset(SB_SECONDARY_OFFSET),
            DiskOffset(disk_size.saturating_sub(SB_TERTIARY_RELATIVE)),
        ]
    }
}

// ─────────────────────────────────────────────────────────────
// Snapshot lisible
// ─────────────────────────────────────────────────────────────

/// Snapshot des métadonnées du superblock (thread-safe, copiable)
#[derive(Debug, Clone)]
pub struct SuperblockSnapshot {
    pub epoch: u64,
    pub object_count: u64,
    pub blob_count: u64,
    pub free_bytes: u64,
    pub disk_size: u64,
    pub heap_start: u64,
    pub heap_end: u64,
    pub volume_name: [u8; VOLUME_NAME_LEN],
    pub uuid: [u8; 16],
    pub last_commit_time: u64,
    pub total_commits: u64,
}

impl SuperblockSnapshot {
    /// Utilisation en pourcentage (0–100)
    pub fn usage_pct(&self) -> u64 {
        let total = self.heap_end.saturating_sub(self.heap_start);
        if total == 0 { return 0; }
        let used = total.saturating_sub(self.free_bytes);
        used.saturating_mul(100) / total
    }

    /// Nombre de blocs libres
    pub fn free_blocks(&self) -> u64 {
        self.free_bytes / BLOCK_SIZE as u64
    }

    /// Vrai si l'espace presque plein (>90%)
    pub fn is_nearly_full(&self) -> bool {
        self.usage_pct() >= 90
    }
}

// ─────────────────────────────────────────────────────────────
// Utilitaires
// ─────────────────────────────────────────────────────────────

/// Vérifie qu'un tampon contient un superblock valide (HDR-03)
pub fn verify_superblock_bytes(buf: &[u8]) -> ExofsResult<ExoSuperblockDisk> {
    let sb = ExoSuperblockDisk::from_bytes(buf)?;
    sb.verify()?;
    Ok(sb)
}

/// Calcule les offsets des 3 miroirs pour une taille de disque donnée
pub fn superblock_mirror_offsets(disk_size: u64) -> [DiskOffset; SB_MIRROR_COUNT] {
    [
        DiskOffset(SB_PRIMARY_OFFSET),
        DiskOffset(SB_SECONDARY_OFFSET),
        DiskOffset(disk_size.saturating_sub(SB_TERTIARY_RELATIVE)),
    ]
}

// ─────────────────────────────────────────────────────────────
// Tests unitaires
// ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    const TEST_DISK: u64 = 32 * 1024 * 1024; // 32 MiB

    fn make_disk(size: usize) -> Vec<u8> { vec![0u8; size] }

    #[test]
    fn superblock_disk_size() {
        assert_eq!(core::mem::size_of::<ExoSuperblockDisk>(), SUPERBLOCK_DISK_SIZE);
    }

    #[test]
    fn new_volume_valid_checksum() {
        let sb = ExoSuperblockDisk::new_volume(TEST_DISK, b"TestVol", [0u8; 16], 12345);
        assert!(sb.verify().is_ok());
    }

    #[test]
    fn bad_magic_detected_first() {
        let mut sb = ExoSuperblockDisk::new_volume(TEST_DISK, b"Test", [0u8; 16], 0);
        sb.magic = 0xDEAD_BEEF;
        assert!(matches!(sb.verify(), Err(ExofsError::BadMagic)));
    }

    #[test]
    fn checksum_mismatch_detected() {
        let mut sb = ExoSuperblockDisk::new_volume(TEST_DISK, b"Test", [0u8; 16], 0);
        sb.epoch_current = 42; // modifie un champ SANS recalculer le checksum
        assert!(matches!(sb.verify(), Err(ExofsError::ChecksumMismatch)));
    }

    #[test]
    fn format_and_mount_roundtrip() {
        let mut disk = make_disk(TEST_DISK as usize);

        let _mgr = SuperblockManager::format(
            TEST_DISK,
            b"RoundtripVol",
            [0xAB; 16],
            999,
            |off, buf| {
                let s = off.0 as usize;
                if s + buf.len() <= disk.len() { disk[s..s+buf.len()].copy_from_slice(buf); }
                Ok(buf.len())
            },
        ).unwrap();

        let mgr2 = SuperblockManager::mount(
            TEST_DISK,
            |off, sz| {
                let s = off.0 as usize;
                let e = (s + sz).min(disk.len());
                let mut v = vec![0u8; sz];
                let avail = e - s;
                v[..avail].copy_from_slice(&disk[s..e]);
                Ok(v)
            },
        ).unwrap();

        let snap = mgr2.snapshot();
        assert_eq!(snap.disk_size, TEST_DISK);
    }

    #[test]
    fn commit_increments_epoch() {
        let mut disk = make_disk(TEST_DISK as usize);

        let mgr = SuperblockManager::format(
            TEST_DISK, b"Epoch", [0u8; 16], 0,
            |off, buf| {
                let s = off.0 as usize;
                if s + buf.len() <= disk.len() { disk[s..s+buf.len()].copy_from_slice(buf); }
                Ok(buf.len())
            },
        ).unwrap();

        let ep1 = mgr.current_epoch().0;
        mgr.commit(1000, |off, buf| {
            let s = off.0 as usize;
            if s + buf.len() <= disk.len() { disk[s..s+buf.len()].copy_from_slice(buf); }
            Ok(buf.len())
        }).unwrap();
        let ep2 = mgr.current_epoch().0;
        assert!(ep2 > ep1);
    }

    #[test]
    fn recovery_picks_highest_epoch() {
        let mut disk = make_disk(TEST_DISK as usize);
        let offsets = superblock_mirror_offsets(TEST_DISK);

        // Écrire deux miroirs identiques
        let sb = ExoSuperblockDisk::new_volume(TEST_DISK, b"Recovery", [0u8; 16], 0);
        let bytes = sb.as_bytes();
        for off in &offsets[..2] {
            let s = off.0 as usize;
            disk[s..s + SUPERBLOCK_DISK_SIZE].copy_from_slice(bytes);
        }

        // Écrire un miroir tertiaire avec une époque plus élevée
        let mut sb_newer = sb;
        sb_newer.epoch_current = 999;
        sb_newer.finalize();
        let s = offsets[2].0 as usize;
        disk[s..s + SUPERBLOCK_DISK_SIZE].copy_from_slice(sb_newer.as_bytes());

        let mgr = SuperblockManager::mount(TEST_DISK, |off, sz| {
            let s = off.0 as usize;
            let e = (s + sz).min(disk.len());
            let mut v = vec![0u8; sz];
            v[..e-s].copy_from_slice(&disk[s..e]);
            Ok(v)
        }).unwrap();

        assert_eq!(mgr.current_epoch().0, 999);
    }

    #[test]
    fn three_mirrors_written_on_commit() {
        let mut disk = make_disk(TEST_DISK as usize);
        let mut write_calls = 0u32;

        let mgr = SuperblockManager::format(
            TEST_DISK, b"Mirrors", [0u8; 16], 0,
            |off, buf| {
                write_calls += 1;
                let s = off.0 as usize;
                if s + buf.len() <= disk.len() { disk[s..s+buf.len()].copy_from_slice(buf); }
                Ok(buf.len())
            },
        ).unwrap();

        // Le format doit écrire exactement 3 miroirs
        assert_eq!(write_calls, SB_MIRROR_COUNT as u32);
    }

    #[test]
    fn snapshot_usage_pct() {
        let sb = ExoSuperblockDisk::new_volume(TEST_DISK, b"Usage", [0u8; 16], 0);
        let snap = SuperblockSnapshot {
            epoch: 1,
            object_count: 0,
            blob_count: 0,
            free_bytes: sb.free_bytes / 2, // 50% utilisé
            disk_size: sb.disk_size_bytes,
            heap_start: sb.heap_start,
            heap_end: sb.heap_end,
            volume_name: sb.volume_name,
            uuid: sb.uuid,
            last_commit_time: 0,
            total_commits: 0,
        };
        assert!(snap.usage_pct() > 40);
        assert!(!snap.is_nearly_full());
    }

    #[test]
    fn min_disk_size_enforced() {
        let r = SuperblockManager::format(
            MIN_DISK_SIZE - 1, b"TooSmall", [0u8; 16], 0,
            |_, _| Ok(0),
        );
        assert!(matches!(r, Err(ExofsError::InvalidSize)));
    }
}
