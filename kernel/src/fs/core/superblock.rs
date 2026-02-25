// kernel/src/fs/core/superblock.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// VFS SUPERBLOCK GÉNÉRIQUE — abstraction montage  (Exo-OS · Couche 3)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Ce fichier définit le superblock GÉNÉRIQUE du VFS, distinct des superblocks
// propres à chaque FS (ext4plus/superblock.rs, drivers/ext4/superblock.rs, etc.).
//
// Chaque filesystem monté dans le VFS possède une instance `VfsSuperblock`.
// Le superblock concret du FS (ext4, fat32) est stocké dans `fs_data`.
//
// Cycle de vie :
//   vfs_mount() → crée VfsSuperblock → appelle FsType::mount() → instancie fs_data
//   vfs_umount()→ appelle VfsSuperblock::sync_fs() + unmount() → libère fs_data
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use alloc::sync::Arc;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use crate::fs::core::types::{DevId, FsError, FsResult, InodeNumber, MountFlags, FileMode};
use crate::fs::core::inode::InodeRef;
use crate::scheduler::sync::spinlock::SpinLock;
use crate::scheduler::sync::rwlock::RwLock;

// ─────────────────────────────────────────────────────────────────────────────
// MountMode — mode de montage détecté au moment du mount
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MountMode {
    /// Lecture + écriture (journal propre, flags incompat connus).
    ReadWrite,
    /// Lecture seule (journal sale ou RO_COMPAT non supporté).
    ReadOnly,
    /// Refus de montage (flag INCOMPAT inconnu ou fs corrompu).
    Refused,
}

// ─────────────────────────────────────────────────────────────────────────────
// FsOps — table d'opérations fournie par chaque driver FS
// ─────────────────────────────────────────────────────────────────────────────

/// Callbacks implémentés par chaque driver de système de fichiers.
pub trait FsOps: Send + Sync {
    /// Retourne le nom du FS (ex: "ext4plus", "ext4", "fat32").
    fn name(&self) -> &'static str;

    /// Retourne l'inode racine du FS monté.
    fn root_inode(&self) -> FsResult<InodeRef>;

    /// Synchronise le FS sur disque (flush journal + métadonnées).
    fn sync_fs(&self, wait: bool) -> FsResult<()>;

    /// Statistiques d'utilisation (blocs totaux/libres, inodes totaux/libres).
    fn statfs(&self) -> FsResult<FsStatInfo>;

    /// Démontage propre : flush + libération des ressources.
    fn unmount(&self) -> FsResult<()>;

    /// Remonte avec de nouveaux flags (ex: ro → rw).
    fn remount(&self, flags: MountFlags) -> FsResult<()> {
        let _ = flags;
        Err(FsError::NotSupported)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// FsStatInfo — utilisé par statfs()
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Default)]
pub struct FsStatInfo {
    pub f_type:    u64,   // magic du FS
    pub f_bsize:   u64,   // taille de bloc préférée
    pub f_blocks:  u64,   // blocs totaux
    pub f_bfree:   u64,   // blocs libres
    pub f_bavail:  u64,   // blocs disponibles pour non-root
    pub f_files:   u64,   // inodes totaux
    pub f_ffree:   u64,   // inodes libres
    pub f_namelen: u32,   // longueur max d'un nom de fichier
    pub f_frsize:  u32,   // taille de fragment
}

// ─────────────────────────────────────────────────────────────────────────────
// VfsSuperblock — superblock générique du VFS
// ─────────────────────────────────────────────────────────────────────────────

/// Superblock générique du VFS, partagé entre tous les filesystems.
/// Distinct du superblock on-disk de chaque FS.
pub struct VfsSuperblock {
    /// Identifiant du périphérique bloc.
    pub dev:         DevId,
    /// Flags de montage (MNT_RDONLY, MNT_NOEXEC, etc.).
    pub flags:       MountFlags,
    /// Taille de bloc du FS en octets.
    pub block_size:  u32,
    /// Espace utilisateur maximum pour les inodes (s_inode_size).
    pub inode_size:  u16,
    /// Nom du point de montage (ex: "/", "/mnt/usb").
    pub mount_point: SpinLock<String>,
    /// Nom du FS (ex: "ext4plus", "ext4", "fat32").
    pub fs_type:     &'static str,
    /// Système de fichiers en lecture seule ?
    pub readonly:    AtomicBool,
    /// FS correctement démonté ?
    pub clean:       AtomicBool,
    /// Borne supérieure du numéro d'inode.
    pub max_inode:   InodeNumber,
    /// Compteur de références (montages liés, dentries).
    pub ref_count:   AtomicU32,
    /// Opérations spécifiques au FS.
    pub ops:         Box<dyn FsOps>,
    /// Inodes "dirty" en attente de writeback.
    pub dirty_inodes: SpinLock<Vec<InodeRef>>,
    /// Statistiques du superblock.
    pub stats:       SbStats,
}

impl VfsSuperblock {
    /// Crée un nouveau VfsSuperblock.
    pub fn new(
        dev:         DevId,
        flags:       MountFlags,
        block_size:  u32,
        inode_size:  u16,
        mount_point: String,
        fs_type:     &'static str,
        readonly:    bool,
        max_inode:   InodeNumber,
        ops:         Box<dyn FsOps>,
    ) -> Arc<Self> {
        Arc::new(Self {
            dev,
            flags,
            block_size,
            inode_size,
            mount_point: SpinLock::new(mount_point),
            fs_type,
            readonly:    AtomicBool::new(readonly),
            clean:       AtomicBool::new(false),
            max_inode,
            ref_count:   AtomicU32::new(1),
            ops,
            dirty_inodes: SpinLock::new(Vec::new()),
            stats: SbStats::new(),
        })
    }

    /// Retourne l'inode racine du FS.
    pub fn root_inode(&self) -> FsResult<InodeRef> {
        self.ops.root_inode()
    }

    /// Synchronise : flush des inodes dirty + journal.
    pub fn sync_fs(&self, wait: bool) -> FsResult<()> {
        // Flush des inodes dirty.
        let dirty: Vec<InodeRef> = {
            let mut lock = self.dirty_inodes.lock();
            core::mem::take(&mut *lock)
        };
        self.stats.sync_dirty_inodes.fetch_add(dirty.len() as u64, Ordering::Relaxed);
        // Délègue au FS concret.
        self.ops.sync_fs(wait)?;
        self.stats.syncs.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Statistiques d'espace disque.
    pub fn statfs(&self) -> FsResult<FsStatInfo> {
        self.ops.statfs()
    }

    /// Démonte proprement.
    pub fn unmount(&self) -> FsResult<()> {
        self.sync_fs(true)?;
        self.ops.unmount()?;
        self.clean.store(true, Ordering::Release);
        self.stats.unmounts.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Marque un inode comme dirty (en attente writeback).
    pub fn mark_inode_dirty(&self, inode: InodeRef) {
        self.dirty_inodes.lock().push(inode);
        self.stats.dirty_marks.fetch_add(1, Ordering::Relaxed);
    }

    /// Le FS est-il en lecture seule ?
    #[inline]
    pub fn is_readonly(&self) -> bool { self.readonly.load(Ordering::Relaxed) }

    /// Incrément du compteur de références.
    pub fn inc_ref(&self) { self.ref_count.fetch_add(1, Ordering::Relaxed); }

    /// Décrément. Retourne true si le compteur atteint zéro.
    pub fn dec_ref(&self) -> bool {
        self.ref_count.fetch_sub(1, Ordering::Release) == 1
    }
}

pub type VfsSuperblockRef = Arc<VfsSuperblock>;

// ─────────────────────────────────────────────────────────────────────────────
// Registre des superblocks montés
// ─────────────────────────────────────────────────────────────────────────────

/// Table des superblocks actifs (un par partition montée).
pub struct SuperblockTable {
    inner: SpinLock<Vec<VfsSuperblockRef>>,
}

impl SuperblockTable {
    pub const fn new() -> Self {
        Self { inner: SpinLock::new(Vec::new()) }
    }

    /// Enregistre un nouveau superblock (lors du montage).
    pub fn register(&self, sb: VfsSuperblockRef) {
        self.inner.lock().push(sb);
        SB_TABLE_STATS.total_mounts.fetch_add(1, Ordering::Relaxed);
    }

    /// Retire un superblock (lors du démontage).
    pub fn unregister(&self, dev: DevId) -> Option<VfsSuperblockRef> {
        let mut lock = self.inner.lock();
        if let Some(pos) = lock.iter().position(|s| s.dev == dev) {
            SB_TABLE_STATS.total_unmounts.fetch_add(1, Ordering::Relaxed);
            Some(lock.remove(pos))
        } else {
            None
        }
    }

    /// Cherche un superblock par DevId.
    pub fn find_by_dev(&self, dev: DevId) -> Option<VfsSuperblockRef> {
        self.inner.lock().iter().find(|s| s.dev == dev).cloned()
    }

    /// Cherche un superblock par point de montage.
    pub fn find_by_mount(&self, path: &str) -> Option<VfsSuperblockRef> {
        self.inner.lock().iter()
            .find(|s| *s.mount_point.lock() == path)
            .cloned()
    }

    /// Nombre de partitions montées.
    pub fn count(&self) -> usize { self.inner.lock().len() }

    /// Synchronise tous les FS (utilisé lors du shutdown/reboot).
    pub fn sync_all(&self, wait: bool) {
        let sbs: Vec<VfsSuperblockRef> = self.inner.lock().clone();
        for sb in sbs { let _ = sb.sync_fs(wait); }
        SB_TABLE_STATS.sync_all_calls.fetch_add(1, Ordering::Relaxed);
    }
}

/// Registre global des superblocks montés.
pub static SUPERBLOCK_TABLE: SuperblockTable = SuperblockTable::new();

// ─────────────────────────────────────────────────────────────────────────────
// SbStats — instrumentation
// ─────────────────────────────────────────────────────────────────────────────

pub struct SbStats {
    pub syncs:           AtomicU64,
    pub unmounts:        AtomicU64,
    pub dirty_marks:     AtomicU64,
    pub sync_dirty_inodes: AtomicU64,
}

impl SbStats {
    pub const fn new() -> Self {
        macro_rules! z { () => { AtomicU64::new(0) }; }
        Self { syncs: z!(), unmounts: z!(), dirty_marks: z!(), sync_dirty_inodes: z!() }
    }
}

/// Statistiques globales du registre de superblocks.
pub struct SbTableStats {
    pub total_mounts:   AtomicU64,
    pub total_unmounts: AtomicU64,
    pub sync_all_calls: AtomicU64,
}

impl SbTableStats {
    pub const fn new() -> Self {
        macro_rules! z { () => { AtomicU64::new(0) }; }
        Self { total_mounts: z!(), total_unmounts: z!(), sync_all_calls: z!() }
    }
}

pub static SB_TABLE_STATS: SbTableStats = SbTableStats::new();
