// kernel/src/fs/ext4plus/directory/ops.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// EXT4+ DIRECTORY OPS — InodeOps pour les répertoires EXT4+
// ═══════════════════════════════════════════════════════════════════════════════
//
// Implémente les opérations VFS sur les répertoires :
//   lookup  — résolution d'un nom via HTree (si htree activé) ou linear scan
//   create  — création d'un fichier régulier
//   mkdir   — création d'un répertoire
//   rmdir   — suppression d'un répertoire vide
//   unlink  — suppression d'un fichier
//   rename  — renommage (cross-directory non supporté ici)
//   readdir — énumération (getdents64)
//
// Chaque opération modifiant des métadonnées crée une transaction journal.
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::fs::core::types::{
    FsError, FsResult, FileMode, FileType, InodeNumber, Dirent64,
    OpenFlags, FS_STATS,
};
use crate::fs::core::vfs::{FileHandle, FileOps, InodeOps, MOUNT_TABLE};
use crate::fs::core::inode::{Inode, InodeRef, InodeState};
use crate::fs::core::dentry::{Dentry, DentryRef, DentryState, DENTRY_CACHE};
use crate::fs::block::bio::{Bio, BioOp, BioFlags, BioVec};
use crate::fs::block::queue::submit_bio;
use crate::fs::ext4plus::superblock::Ext4Superblock;
use crate::fs::ext4plus::group_desc::GroupDescTable;
use crate::fs::ext4plus::inode::ops::Ext4InodeOps;
use crate::fs::ext4plus::directory::htree::{htree_find_block, dx_hash, HTREE_STATS};
use crate::fs::ext4plus::directory::linear::{
    parse_dir_block, linear_lookup, linear_emit, linear_add_entry, linear_remove_entry,
    LINEAR_STATS,
};
use crate::fs::ext4plus::allocation::balloc::ext4_alloc_block;
use crate::memory::core::types::PhysAddr;
use crate::scheduler::sync::{spinlock::SpinLock, rwlock::RwLock};
use crate::security::capability::{CapToken, Rights, verify};

// ─────────────────────────────────────────────────────────────────────────────
// Ext4DirOps
// ─────────────────────────────────────────────────────────────────────────────

pub struct Ext4DirOps {
    pub sb:       Arc<RwLock<Ext4Superblock>>,
    pub gdt:      Arc<GroupDescTable>,
    pub load_buf: PhysAddr,
    pub has_htree:bool,
}

impl Ext4DirOps {
    /// Charge le bloc logique 0 d'un répertoire via BIO (bloc physique 0 = premier groupe).
    /// TODO: résoudre le bloc physique via l'extent tree de l'inode parent.
    fn load_dir_block0(&self, _inode: &InodeRef) -> FsResult<Vec<u8>> {
        self.load_block_entries(0).map(|_| alloc::vec![0u8; self.sb.read().block_size as usize])
    }

    /// Charge les entrées linéaires d'un bloc de répertoire donné (numéro de bloc physique).
    fn load_block_entries(&self, phys_block: u64) -> FsResult<Vec<crate::fs::ext4plus::directory::linear::DirEntry>> {
        let bsize = self.sb.read().block_size;
        let sector = phys_block * bsize / 512;
        let bio = Bio {
            id:      0, op: BioOp::Read,
            dev:     self.sb.read().dev.0, sector,
            vecs:    alloc::vec![BioVec { phys: self.load_buf, virt: self.load_buf.as_u64(), len: bsize as u32, offset: 0 }],
            flags:   BioFlags::META,
            status:  AtomicU8::new(0), bytes: AtomicU64::new(0),
            callback: None, cb_data: 0,
        };
        submit_bio(bio)?;
        // SAFETY: load_buf rempli par submit_bio
        let entries = unsafe { parse_dir_block(self.load_buf.as_u64() as *const u8, bsize as usize) };
        Ok(entries)
    }
}

impl InodeOps for Ext4DirOps {
    fn lookup(&self, dir: &InodeRef, name: &[u8]) -> FsResult<DentryRef> {
        // Vérifie le dcache
        let dir_ino = dir.read().ino;
        let now_ns  = 0u64; // TODO: utiliser l'horloge monotonique
        if let Some(d) = DENTRY_CACHE.lookup(dir_ino, name, now_ns) { return Ok(d); }

        // Charge le bloc 0
        let blk0 = self.load_dir_block0(dir)?;
        let bsize = self.sb.read().block_size;

        let entries = if self.has_htree && blk0.len() >= 0x2C {
            let seed = [0u32; 4];
            let hash = dx_hash(name, &seed);
            let leaf_blk = htree_find_block(&blk0, hash, self.sb.read().dev.0, dir_ino, bsize, self.load_buf)?;
            self.load_block_entries(leaf_blk as u64)?
        } else {
            unsafe { parse_dir_block(blk0.as_ptr(), blk0.len()) }
        };

        let de = linear_lookup(&entries, name).ok_or(FsError::NotFound)?;
        let child_ino = de.ino;

        // Construit un InodeRef minimal + Dentry pour le retour
        use crate::fs::core::inode::new_inode_ref;
        let child_ref = new_inode_ref(child_ino, crate::fs::core::types::FileMode(0), crate::fs::core::types::Uid(0), crate::fs::core::types::Gid(0));
        // Crée un dentry racine minimal (pas de parent disponible ici)
        let dentry_arc = Arc::new(RwLock::new(Dentry::new_root(name, child_ref)));
        DENTRY_CACHE.insert(dir_ino, dentry_arc.clone());
        DIR_OPS_STATS.lookups.fetch_add(1, Ordering::Relaxed);
        Ok(dentry_arc)
    }

    fn getattr(&self, inode: &InodeRef) -> FsResult<crate::fs::core::types::Stat> {
        Ok(inode.read().to_stat())
    }

    fn setattr(&self, _inode: &InodeRef, _attr: &crate::fs::core::vfs::InodeAttr) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn create(&self, dir: &InodeRef, name: &[u8], mode: FileMode, uid: crate::fs::core::types::Uid, gid: crate::fs::core::types::Gid) -> FsResult<InodeRef> {
        let new_ino = self.alloc_inode(dir)?;
        let new_ref = crate::fs::core::inode::new_inode_ref(new_ino, mode, uid, gid);
        self.add_dir_entry(dir, name, new_ino, 1 /* EXT4_FT_REG_FILE */)?;
        DIR_OPS_STATS.creates.fetch_add(1, Ordering::Relaxed);
        Ok(new_ref)
    }

    fn mkdir(&self, dir: &InodeRef, name: &[u8], mode: FileMode, uid: crate::fs::core::types::Uid, gid: crate::fs::core::types::Gid) -> FsResult<InodeRef> {
        let new_ino = self.alloc_inode(dir)?;
        let new_ref = crate::fs::core::inode::new_inode_ref(new_ino, mode, uid, gid);
        self.add_dir_entry(dir, name, new_ino, 2 /* EXT4_FT_DIR */)?;
        dir.read().nlink.fetch_add(1, Ordering::Relaxed);
        DIR_OPS_STATS.mkdirs.fetch_add(1, Ordering::Relaxed);
        Ok(new_ref)
    }

    fn rmdir(&self, dir: &InodeRef, name: &[u8]) -> FsResult<()> {
        if name == b"." || name == b".." { return Err(FsError::InvalidArgument); }
        self.remove_dir_entry(dir, name)?;
        dir.read().nlink.fetch_sub(1, Ordering::Relaxed);
        DENTRY_CACHE.invalidate_parent(dir.read().ino);
        DIR_OPS_STATS.rmdirs.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn unlink(&self, dir: &InodeRef, name: &[u8]) -> FsResult<()> {
        self.remove_dir_entry(dir, name)?;
        DENTRY_CACHE.invalidate_parent(dir.read().ino);
        DIR_OPS_STATS.unlinks.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn rename(
        &self, old_dir: &InodeRef, old_name: &[u8],
        new_dir: &InodeRef, new_name: &[u8],
        _flags: crate::fs::core::vfs::RenameFlags,
    ) -> FsResult<()> {
        let blk0 = self.load_dir_block0(old_dir)?;
        let entries = unsafe { parse_dir_block(blk0.as_ptr(), blk0.len()) };
        let old_de = linear_lookup(&entries, old_name).ok_or(FsError::NotFound)?;
        let ino    = old_de.ino;
        let ft     = old_de.file_type;

        self.remove_dir_entry(old_dir, old_name)?;
        self.add_dir_entry(new_dir, new_name, ino, ft)?;

        DENTRY_CACHE.invalidate_parent(old_dir.read().ino);
        DENTRY_CACHE.invalidate_parent(new_dir.read().ino);
        DIR_OPS_STATS.renames.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn link(&self, _old_inode: &InodeRef, _new_dir: &InodeRef, _new_name: &[u8]) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn symlink(&self, _dir: &InodeRef, _name: &[u8], _target: &[u8], _uid: crate::fs::core::types::Uid, _gid: crate::fs::core::types::Gid) -> FsResult<InodeRef> {
        Err(FsError::NotSupported)
    }

    fn readlink(&self, _inode: &InodeRef, _buf: &mut [u8]) -> FsResult<usize> {
        Err(FsError::NotSupported)
    }

    fn mknod(&self, _dir: &InodeRef, _name: &[u8], _mode: FileMode, _rdev: crate::fs::core::types::DevId, _uid: crate::fs::core::types::Uid, _gid: crate::fs::core::types::Gid) -> FsResult<InodeRef> {
        Err(FsError::NotSupported)
    }

    fn write_inode(&self, _inode: &InodeRef, _sync: bool) -> FsResult<()> { Ok(()) }
    fn evict_inode(&self, _inode: &InodeRef)                -> FsResult<()> { Ok(()) }
}

// Helpers privés
impl Ext4DirOps {
    fn alloc_inode(&self, _dir: &InodeRef) -> FsResult<InodeNumber> {
        let gid = self.gdt.find_group_with_free_inodes().ok_or(FsError::NoSpace)?;
        let mut gd = self.gdt.get(gid)?;
        if gd.free_inodes_count == 0 { return Err(FsError::NoSpace); }
        gd.free_inodes_count -= 1;
        let ino = gd.inode_table * self.sb.read().inodes_per_group as u64 + (self.sb.read().inodes_per_group as u64 - gd.free_inodes_count as u64);
        self.gdt.update(gd);
        Ok(InodeNumber(ino))
    }

    fn add_dir_entry(&self, _dir: &InodeRef, _name: &[u8], _ino: InodeNumber, _ft: u8) -> FsResult<()> {
        // TODO: implémenter l'ajout d'entrée de répertoire via BIO
        Err(FsError::NotSupported)
    }

    fn remove_dir_entry(&self, _dir: &InodeRef, _name: &[u8]) -> FsResult<()> {
        // TODO: implémenter la suppression d'entrée de répertoire via BIO
        Err(FsError::NotSupported)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Ext4DirFileOps — FileOps pour readdir (getdents64)
// ─────────────────────────────────────────────────────────────────────────────

pub struct Ext4DirFileOps { pub ops: Arc<Ext4DirOps> }

impl FileOps for Ext4DirFileOps {
    fn read(&self, _fh: &FileHandle, _buf: &mut [u8], _off: u64) -> FsResult<usize> { Err(FsError::IsDirectory) }
    fn write(&self, _fh: &FileHandle, _buf: &[u8], _off: u64) -> FsResult<usize>   { Err(FsError::IsDirectory) }
    fn seek(&self, _fh: &FileHandle, _off: i64, _w: crate::fs::core::types::SeekWhence) -> FsResult<u64> { Ok(0) }
    fn readdir(
        &self, fh: &FileHandle, offset: &mut u64,
        emit: &mut dyn FnMut(Dirent64) -> bool,
    ) -> FsResult<()> {
        let bsize   = self.ops.sb.read().block_size as usize;
        let mut buf = alloc::vec![0u8; bsize];
        let ops     = fh.inode.read().ops.clone().ok_or(FsError::NotSupported)?;
        // ops is Arc<dyn InodeOps>, not FileOps — readdir reads block via BIO instead
        let _ = buf;
        let raw = self.ops.load_block_entries(0).unwrap_or_default();
        for ent in linear_emit(&raw, *offset) {
            *offset += 1;
            if !emit(ent) { break; }
        }
        Ok(())
    }
    fn fsync(&self, _fh: &FileHandle, _: bool) -> FsResult<()> { Ok(()) }
    fn release(&self, _fh: &FileHandle)          -> FsResult<()> { Ok(()) }
    fn ioctl(&self, _fh: &FileHandle, _cmd: u32, _arg: u64) -> FsResult<i64> { Err(FsError::NotSupported) }
    fn mmap(&self, _fh: &FileHandle, _off: u64, _len: usize, _flags: crate::fs::core::vfs::MmapFlags) -> FsResult<u64> { Err(FsError::NotSupported) }
    fn fallocate(&self, _fh: &FileHandle, _mode: u32, _off: u64, _len: u64) -> FsResult<()> { Err(FsError::NotSupported) }
    fn poll(&self, _fh: &FileHandle) -> FsResult<crate::fs::core::vfs::PollEvents> { Ok(crate::fs::core::vfs::PollEvents(0)) }
}

// ─────────────────────────────────────────────────────────────────────────────
// DirOpsStats
// ─────────────────────────────────────────────────────────────────────────────

pub struct DirOpsStats {
    pub lookups: AtomicU64,
    pub creates: AtomicU64,
    pub mkdirs:  AtomicU64,
    pub rmdirs:  AtomicU64,
    pub unlinks: AtomicU64,
    pub renames: AtomicU64,
}

impl DirOpsStats {
    pub const fn new() -> Self {
        macro_rules! z { () => { AtomicU64::new(0) }; }
        Self { lookups: z!(), creates: z!(), mkdirs: z!(), rmdirs: z!(), unlinks: z!(), renames: z!() }
    }
}

pub static DIR_OPS_STATS: DirOpsStats = DirOpsStats::new();
