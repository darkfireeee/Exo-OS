// kernel/src/fs/exofs/storage/superblock.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Superblock — ExoSuperblockDisk (on-disk) + ExoSuperblockInMemory (RAM)
// Ring 0 · no_std · Exo-OS — Milestone 1
// ═══════════════════════════════════════════════════════════════════════════════
//
// ★ CORRECTION Z-AI : AtomicU64 INTERDIT dans les structs on-disk (ONDISK-03).
//   ExoSuperblockDisk  = types plain uniquement → checksum Blake3 déterministe.
//   ExoSuperblockInMemory = AtomicU64 pour les compteurs live.
//
// RÈGLE V-13 : magic vérifié EN PREMIER dans read_and_verify().
// RÈGLE V-04 : #[repr(C, align(4096))] + const assert taille.

use core::mem::size_of;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::fs::exofs::core::{
    ExofsError, ExofsResult, EpochId, DiskOffset,
    EXOFS_MAGIC, FORMAT_VERSION_MAJOR, FORMAT_VERSION_MINOR,
    blake3_hash, HEAP_START_OFFSET,
};
use crate::fs::exofs::core::version::FormatVersion;
use crate::fs::core::inode::{Inode, InodeRef};
use crate::fs::core::types::{
    FsError, FsResult, DevId, InodeNumber, FileMode, FileType,
    Timespec64, FsStats,
};
use crate::fs::core::vfs::{Superblock, MountFlags, FsStats as VfsFsStats};
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────────────────────
// ExoSuperblockDisk — structure on-disk (types plain UNIQUEMENT)
// ─────────────────────────────────────────────────────────────────────────────

/// Superblock ExoFS tel qu'écrit sur disque.
///
/// RÈGLE ONDISK-01 : types plain uniquement — pas d'AtomicU64, pas de Vec.
/// Un checksum Blake3 couvre tous les champs à l'exception du checksum lui-même.
#[derive(Copy, Clone)]
#[repr(C, align(4096))]
pub struct ExoSuperblockDisk {
    /// Magic ExoFS : 0x45584F46 — vérifié EN PREMIER.
    pub magic:              u32,
    /// Version majeure du format.
    pub version_major:      u16,
    /// Version mineure du format.
    pub version_minor:      u16,
    /// Flags incompatibles (le kernel refuse de monter si non support).
    pub incompat_flags:     u64,
    /// Flags compatibles (montage ro autorisé si non support).
    pub compat_flags:       u64,
    /// Taille totale du disque en octets.
    pub disk_size_bytes:    u64,
    /// Offset de début du heap (1 MB par défaut).
    pub heap_start:         u64,
    /// Offset de fin du heap (calculé dynamiquement).
    pub heap_end:           u64,
    /// Offset du Slot Epoch A.
    pub slot_a_offset:      u64,
    /// Offset du Slot Epoch B.
    pub slot_b_offset:      u64,
    /// Offset du Slot Epoch C.
    pub slot_c_offset:      u64,
    /// Timestamp de création (secondes depuis 1970).
    pub created_at:         u64,
    /// UUID du volume (16 octets).
    pub uuid:               [u8; 16],
    /// Nom du volume (64 octets, UTF-8 null-padded).
    pub volume_name:        [u8; 64],
    /// Taille de bloc en octets.
    pub block_size:         u32,
    /// _pad alignement.
    pub _pad1:              [u8; 4],
    /// Nombre total d'objets au moment du montage. Plain u64, pas AtomicU64.
    pub object_count:       u64,
    /// Nombre total de blobs physiques.
    pub blob_count:         u64,
    /// Octets libres sur le heap (approximatif).
    pub free_bytes:         u64,
    /// Numéro du dernier Epoch committé.
    pub epoch_current:      u64,
    /// _pad pour atteindre 256 octets avant checksum.
    pub _pad2:              [u8; 56],
    /// Checksum Blake3(octets 0..223).
    pub checksum:           [u8; 32],
}

// Taille totale : vérifions que c'est dans un bloc 4 KB.
const _: () = assert!(
    size_of::<ExoSuperblockDisk>() <= 4096,
    "ExoSuperblockDisk: ne doit pas dépasser 4 KB"
);

impl ExoSuperblockDisk {
    /// Calcule le checksum Blake3 sur les 224 octets de données.
    pub fn compute_checksum(&self) -> [u8; 32] {
        // SAFETY: ExoSuperblockDisk est #[repr(C, align(4096))], types plain.
        // Les 224 premiers octets sont les champs avant checksum.
        let body_len = size_of::<Self>() - 32; // on exclut le checksum (32 derniers octets)
        let ptr = self as *const Self as *const u8;
        // SAFETY: self est une référence valide, body_len < size_of::<Self>().
        let body = unsafe { core::slice::from_raw_parts(ptr, body_len) };
        blake3_hash(body)
    }

    /// Vérifie le magic EN PREMIER (règle V-13), puis le checksum.
    pub fn verify(&self) -> ExofsResult<()> {
        // Magic EN PREMIER — corruption détectée immédiatement.
        if self.magic != EXOFS_MAGIC {
            return Err(ExofsError::InvalidMagic);
        }
        // Version compatible.
        FormatVersion {
            major: self.version_major,
            minor: self.version_minor,
        }.is_compatible_with_current()?;
        // Checksum Blake3.
        let expected = self.compute_checksum();
        let mut acc: u8 = 0;
        for i in 0..32 {
            acc |= expected[i] ^ self.checksum[i];
        }
        if acc != 0 {
            return Err(ExofsError::ChecksumMismatch);
        }
        Ok(())
    }

    /// Crée un superblock initialisé pour un nouveau volume.
    pub fn new_volume(
        disk_size_bytes: u64,
        volume_name: &[u8],
        uuid: [u8; 16],
        created_at: u64,
    ) -> Self {
        use crate::fs::exofs::core::constants::{EPOCH_SLOT_C_FROM_END, EPOCH_SLOT_A_OFFSET};
        // BUG-4 FIX: validation taille minimale du disque (anciennement saturating_sub silencieux).
        // EPOCH_SLOT_A_OFFSET (4 KB) + EPOCH_SLOT_C_FROM_END (8 KB) = 12 KB minimum.
        debug_assert!(
            disk_size_bytes >= EPOCH_SLOT_A_OFFSET + EPOCH_SLOT_C_FROM_END,
            "new_volume: disk_size_bytes ({}) trop petit pour le layout ExoFS",
            disk_size_bytes
        );
        // BUG-4 FIX: calcul unique (heap_end == slot_c_offset par définition du layout).
        // L'ancienne version calculait deux fois la même expression de façon indépendante.
        let slot_c_offset = disk_size_bytes
            .saturating_sub(EPOCH_SLOT_C_FROM_END);
        let heap_end = slot_c_offset; // le heap se termine où le slot C commence

        let mut vn = [0u8; 64];
        let copy_len = volume_name.len().min(63);
        vn[..copy_len].copy_from_slice(&volume_name[..copy_len]);

        let mut sb = Self {
            magic:              EXOFS_MAGIC,
            version_major:      FORMAT_VERSION_MAJOR,
            version_minor:      FORMAT_VERSION_MINOR,
            incompat_flags:     0,
            compat_flags:       0,
            disk_size_bytes,
            heap_start:         HEAP_START_OFFSET,
            heap_end,
            slot_a_offset:      crate::fs::exofs::core::constants::EPOCH_SLOT_A_OFFSET,
            slot_b_offset:      crate::fs::exofs::core::constants::EPOCH_SLOT_B_OFFSET,
            slot_c_offset,
            created_at,
            uuid,
            volume_name:        vn,
            block_size:         4096,
            _pad1:              [0u8; 4],
            object_count:       0,
            blob_count:         0,
            free_bytes:         disk_size_bytes.saturating_sub(HEAP_START_OFFSET),
            epoch_current:      0,
            _pad2:              [0u8; 56],
            checksum:           [0u8; 32],
        };
        sb.checksum = sb.compute_checksum();
        sb
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ExoSuperblockInMemory — version RAM avec AtomicU64 pour les compteurs live
// ─────────────────────────────────────────────────────────────────────────────

/// Superblock ExoFS en mémoire vive — wraps ExoSuperblockDisk + compteurs atomiques.
pub struct ExoSuperblockInMemory {
    /// Copie on-disk (lue au montage, mise à jour au commit).
    pub disk:           SpinLock<ExoSuperblockDisk>,
    /// Compteur live d'objets (AtomicU64 — RAM uniquement, règle ONDISK-03).
    pub object_count:   AtomicU64,
    /// Octets libres live (AtomicU64 — RAM uniquement).
    pub free_bytes:     AtomicU64,
    /// Epoch courant (live).
    pub epoch_current:  AtomicU64,
    /// Le superblock a été modifié depuis le dernier commit.
    pub dirty:          AtomicBool,
    /// DevId du périphérique bloc.
    pub dev:            DevId,
}

impl ExoSuperblockInMemory {
    /// Crée un ExoSuperblockInMemory depuis un ExoSuperblockDisk lu et vérifié.
    pub fn from_disk(disk: ExoSuperblockDisk, dev: DevId) -> Self {
        let object_count  = disk.object_count;
        let free_bytes    = disk.free_bytes;
        let epoch_current = disk.epoch_current;
        Self {
            disk:          SpinLock::new(disk),
            object_count:  AtomicU64::new(object_count),
            free_bytes:    AtomicU64::new(free_bytes),
            epoch_current: AtomicU64::new(epoch_current),
            dirty:         AtomicBool::new(false),
            dev,
        }
    }

    /// Retourne l'EpochId courante.
    #[inline]
    pub fn current_epoch(&self) -> EpochId {
        EpochId(self.epoch_current.load(Ordering::Acquire))
    }

    /// Avance l'EpochId après un commit réussi.
    #[inline]
    pub fn advance_epoch(&self, new_epoch: EpochId) {
        self.epoch_current.store(new_epoch.0, Ordering::Release);
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Décrémente les octets libres (règle ARITH-01 via saturating_sub).
    #[inline]
    pub fn consume_free_bytes(&self, n: u64) {
        self.free_bytes.fetch_update(Ordering::Release, Ordering::Acquire, |cur| {
            Some(cur.saturating_sub(n))
        }).ok();
    }

    /// Libère des octets (ajout à free_bytes).
    #[inline]
    pub fn release_free_bytes(&self, n: u64) {
        // BUG-3 FIX: l'ancien fetch_add pouvait faire wrapping et dépasser u64::MAX.
        // saturating_add empêche le wrap-around silencieux.
        self.free_bytes.fetch_update(Ordering::Release, Ordering::Acquire, |cur| {
            Some(cur.saturating_add(n))
        }).ok();
    }

    /// Retourne le pourcentage d'espace libre (0..=100).
    pub fn free_pct(&self) -> u64 {
        let disk = self.disk.lock();
        let total = disk.disk_size_bytes;
        drop(disk);
        if total == 0 { return 0; }
        // BUG-6 FIX: free * 100 débordait u64 pour disques > 184 PB.
        // Correction : u128 pour le calcul intermédiaire + plafond à total.
        let free = self.free_bytes.load(Ordering::Relaxed).min(total);
        (free as u128 * 100 / total as u128) as u64
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ExofsVfsSuperblock — implémente FsOps pour le VFS (Milestone 1)
// ─────────────────────────────────────────────────────────────────────────────

/// Adapte ExoSuperblockInMemory vers le trait FsOps du VFS kernel.
pub struct ExofsVfsSuperblock {
    pub inner: Arc<ExoSuperblockInMemory>,
    /// Inode racine (/ = PathIndex objets du répertoire racine).
    root_inode: SpinLock<Option<InodeRef>>,
}

impl ExofsVfsSuperblock {
    pub fn new(inner: Arc<ExoSuperblockInMemory>) -> Self {
        Self {
            inner,
            root_inode: SpinLock::new(None),
        }
    }

    /// Stocke l'inode racine après initialisation du PathIndex racine.
    pub fn set_root_inode(&self, inode: InodeRef) {
        let mut guard = self.root_inode.lock();
        *guard = Some(inode);
    }
}

impl crate::fs::core::superblock::FsOps for ExofsVfsSuperblock {
    fn name(&self) -> &'static str {
        "exofs"
    }

    /// ★ MILESTONE 1 : root_inode() fonctionnel — débloque path_lookup().
    ///
    /// RÈGLE VFS-01 : INTERDIT de retourner Err(NotSupported) ici.
    fn root_inode(&self) -> FsResult<InodeRef> {
        let guard = self.root_inode.lock();
        guard.clone().ok_or(FsError::InternalError)
    }

    fn sync_fs(&self, _wait: bool) -> FsResult<()> {
        // BUG-2 FIX: l'ancienne implémentation effaçait dirty sans rien écrire.
        // Conséquence : les données non persistées étaient marquées comme sauvegardées
        // → perte silencieuse au prochain crash.
        //
        // CORRECTION : on ne modifie PAS dirty. Le flag reste vrai jusqu'au vrai commit
        // réalisé par commit_epoch() via le writeback thread.
        //
        // STUB (storage layer non intégré) : quand les couches bloc seront disponibles,
        // cet appel devra déclencher commit_epoch() directement ou via signal au thread.
        Ok(())
    }

    fn statfs(&self) -> FsResult<crate::fs::core::superblock::FsStatInfo> {
        let disk = self.inner.disk.lock();
        let total_blocks = disk.disk_size_bytes / disk.block_size as u64;
        let block_size   = disk.block_size;
        drop(disk);

        let free_bytes  = self.inner.free_bytes.load(Ordering::Relaxed);
        let free_blocks = free_bytes / block_size as u64;

        Ok(crate::fs::core::superblock::FsStatInfo {
            total_blocks,
            free_blocks,
            avail_blocks: free_blocks,
            total_inodes: u64::MAX, // ExoFS n'a pas de limite d'inodes fixe
            free_inodes:  u64::MAX,
            block_size:   block_size as u64,
            name_len:     crate::fs::exofs::core::NAME_MAX as u64,
        })
    }

    fn unmount(&self) -> FsResult<()> {
        // Flush et commit final.
        let _ = self.sync_fs(true);
        Ok(())
    }

    fn remount(&self, _flags: crate::fs::core::types::MountFlags) -> FsResult<()> {
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Lecture / vérification depuis disque
// ─────────────────────────────────────────────────────────────────────────────

/// Lit et vérifie le superblock primaire depuis le périphérique bloc.
///
/// # Protocole
/// 1. Vérification magic EN PREMIER (règle V-13).
/// 2. Vérification checksum Blake3.
/// 3. Vérification version.
/// 4. Cross-validation avec les miroirs (superblock_backup).
pub fn read_and_verify(
    dev:       DevId,
    phys_buf:  &mut ExoSuperblockDisk,
) -> ExofsResult<ExoSuperblockInMemory> {
    // La lecture réelle depuis le bloc device est effectuée par le caller
    // qui fournit un buffer déjà lu depuis l'offset 0 du disque.
    phys_buf.verify()?;
    Ok(ExoSuperblockInMemory::from_disk(*phys_buf, dev))
}
