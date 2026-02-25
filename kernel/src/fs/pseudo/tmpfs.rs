// kernel/src/fs/pseudo/tmpfs.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// TMPFS — Filesystem RAM temporaire (Exo-OS · Couche 3)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Filesystem entièrement en mémoire vive, sans backing store persistant.
// Utilisé pour /tmp, /run, les pipes nommés, les sockets, etc.
//
// Architecture :
//   • `TmpfsInode` : inode avec données stockées dans un Vec<u8> (pas de page cache !).
//   • `TmpfsDir` : répertoire avec table de noms → ino.
//   • `TmpfsFileOps` : lit/écrit directement dans le Vec<u8>.
//   • `TMPFS_STATE` : état global (inodes, limite de taille).
//
// Limite : configurable via le paramètre de montage `size=N`.
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::collections::BTreeMap;

use crate::fs::core::types::{
    DevId, FsError, FsResult, FileMode, FileType, InodeNumber, Stat, SeekWhence, Dirent64,
};
use crate::fs::core::vfs::{FileHandle, FileOps, InodeOps};
use crate::fs::core::inode::{Inode, InodeRef, InodeState, new_inode_ref};
use crate::scheduler::sync::spinlock::SpinLock;
use crate::scheduler::sync::rwlock::RwLock;

// ─────────────────────────────────────────────────────────────────────────────
// TmpfsData — données d'un fichier régulier
// ─────────────────────────────────────────────────────────────────────────────

pub struct TmpfsData {
    data: RwLock<Vec<u8>>,
}

impl TmpfsData {
    pub fn new() -> Self { Self { data: RwLock::new(Vec::new()) } }

    pub fn read(&self, buf: &mut [u8], offset: u64) -> usize {
        let data = self.data.read();
        let off  = offset as usize;
        if off >= data.len() { return 0; }
        let n = (data.len() - off).min(buf.len());
        buf[..n].copy_from_slice(&data[off..off+n]);
        n
    }

    pub fn write(&self, buf: &[u8], offset: u64) -> usize {
        let mut data = self.data.write();
        let off  = offset as usize;
        let need = off + buf.len();
        if need > data.len() { data.resize(need, 0); }
        data[off..off+buf.len()].copy_from_slice(buf);
        buf.len()
    }

    pub fn len(&self) -> u64 { self.data.read().len() as u64 }
    pub fn truncate(&self, size: u64) { self.data.write().resize(size as usize, 0); }
}

// ─────────────────────────────────────────────────────────────────────────────
// TmpfsDir
// ─────────────────────────────────────────────────────────────────────────────

pub struct TmpfsDir {
    /// Nom → InodeNumber.
    entries: SpinLock<BTreeMap<Vec<u8>, InodeNumber>>,
}

impl TmpfsDir {
    pub fn new() -> Self { Self { entries: SpinLock::new(BTreeMap::new()) } }
    pub fn insert(&self, name: Vec<u8>, ino: InodeNumber) { self.entries.lock().insert(name, ino); }
    pub fn remove(&self, name: &[u8]) { self.entries.lock().remove(name); }
    pub fn lookup(&self, name: &[u8]) -> Option<InodeNumber> { self.entries.lock().get(name).copied() }
    pub fn count(&self) -> usize { self.entries.lock().len() }
}

// ─────────────────────────────────────────────────────────────────────────────
// TmpfsNode — inode + données
// ─────────────────────────────────────────────────────────────────────────────

pub enum TmpfsNodeData {
    Regular(TmpfsData),
    Directory(TmpfsDir),
    Symlink(Vec<u8>),
}

pub struct TmpfsNode {
    pub ino:  InodeNumber,
    pub mode: FileMode,
    pub data: TmpfsNodeData,
}

impl TmpfsNode {
    pub fn new_file(ino: InodeNumber, mode: FileMode) -> Arc<Self> {
        Arc::new(Self { ino, mode, data: TmpfsNodeData::Regular(TmpfsData::new()) })
    }
    pub fn new_dir(ino: InodeNumber, mode: FileMode) -> Arc<Self> {
        Arc::new(Self { ino, mode, data: TmpfsNodeData::Directory(TmpfsDir::new()) })
    }
    pub fn new_symlink(ino: InodeNumber, target: Vec<u8>) -> Arc<Self> {
        let mode = FileMode::symlink();
        Arc::new(Self { ino, mode, data: TmpfsNodeData::Symlink(target) })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TmpfsFileOps
// ─────────────────────────────────────────────────────────────────────────────

pub struct TmpfsFileOps { pub node: Arc<TmpfsNode> }

impl FileOps for TmpfsFileOps {
    fn read(&self, _fh: &FileHandle, buf: &mut [u8], offset: u64) -> FsResult<usize> {
        if let TmpfsNodeData::Regular(ref data) = self.node.data {
            let n = data.read(buf, offset);
            TMPFS_STATS.bytes_read.fetch_add(n as u64, Ordering::Relaxed);
            return Ok(n);
        }
        Err(FsError::IsDir)
    }

    fn write(&self, _fh: &FileHandle, buf: &[u8], offset: u64) -> FsResult<usize> {
        if let TmpfsNodeData::Regular(ref data) = self.node.data {
            let state = TMPFS_STATE.lock();
            let used  = TMPFS_STATS.bytes_used.load(Ordering::Relaxed);
            if used + buf.len() as u64 > state.max_size { return Err(FsError::NoSpace); }
            drop(state);
            let n = data.write(buf, offset);
            TMPFS_STATS.bytes_written.fetch_add(n as u64, Ordering::Relaxed);
            TMPFS_STATS.bytes_used.fetch_add(n as u64, Ordering::Relaxed);
            return Ok(n);
        }
        Err(FsError::IsDir)
    }

    fn seek(&self, _fh: &FileHandle, offset: i64, whence: SeekWhence) -> FsResult<u64> {
        match whence {
            SeekWhence::Set => Ok(offset as u64),
            SeekWhence::Cur => Ok(offset as u64),
            SeekWhence::End => {
                if let TmpfsNodeData::Regular(ref data) = self.node.data {
                    return Ok((data.len() as i64 + offset) as u64);
                }
                Ok(0)
            }
            SeekWhence::Data | SeekWhence::Hole => Err(FsError::NotSupported),
        }
    }

    fn release(&self, _fh: &FileHandle) -> FsResult<()> { Ok(()) }
    fn fsync(&self, _fh: &FileHandle, _: bool) -> FsResult<()> { Ok(()) }
    fn ioctl(&self, _fh: &FileHandle, _cmd: u32, _arg: u64) -> FsResult<i64> { Err(FsError::NotSupported) }
    fn mmap(&self, _fh: &FileHandle, _offset: u64, _len: usize, _flags: crate::fs::core::vfs::MmapFlags) -> FsResult<u64> { Err(FsError::NotSupported) }
    fn readdir(&self, _fh: &FileHandle, _offset: &mut u64, _emit: &mut dyn FnMut(Dirent64) -> bool) -> FsResult<()> { Err(FsError::NotDir) }
    fn fallocate(&self, _fh: &FileHandle, _mode: u32, _offset: u64, _len: u64) -> FsResult<()> { Err(FsError::NotSupported) }
    fn poll(&self, _fh: &FileHandle) -> FsResult<crate::fs::core::vfs::PollEvents> { Ok(crate::fs::core::vfs::PollEvents(0x0001 | 0x0004)) }
}

// ─────────────────────────────────────────────────────────────────────────────
// TmpfsState global
// ─────────────────────────────────────────────────────────────────────────────

pub struct TmpfsStateInner {
    pub max_size:  u64,
    pub nodes:     BTreeMap<u64, Arc<TmpfsNode>>,
    next_ino:      u64,
}

impl TmpfsStateInner {
    pub fn new(max_size: u64) -> Self {
        Self { max_size, nodes: BTreeMap::new(), next_ino: 1 }
    }

    pub fn alloc_ino(&mut self) -> InodeNumber {
        let ino = self.next_ino;
        self.next_ino += 1;
        InodeNumber(ino)
    }
}

pub static TMPFS_STATE: SpinLock<TmpfsStateInner> = SpinLock::new(TmpfsStateInner {
    max_size: 256 * 1024 * 1024, // 256 MiB défaut
    nodes:    BTreeMap::new(),
    next_ino: 1,
});

pub fn tmpfs_init(max_size: u64) {
    let mut s = TMPFS_STATE.lock();
    s.max_size = max_size;
    TMPFS_STATS.max_size.store(max_size, Ordering::Relaxed);
}

// ─────────────────────────────────────────────────────────────────────────────
// TmpfsStats
// ─────────────────────────────────────────────────────────────────────────────

pub struct TmpfsStats {
    pub bytes_read:    AtomicU64,
    pub bytes_written: AtomicU64,
    pub bytes_used:    AtomicU64,
    pub max_size:      AtomicU64,
    pub file_count:    AtomicU64,
}

impl TmpfsStats {
    pub const fn new() -> Self {
        Self {
            bytes_read:    AtomicU64::new(0),
            bytes_written: AtomicU64::new(0),
            bytes_used:    AtomicU64::new(0),
            max_size:      AtomicU64::new(256 * 1024 * 1024),
            file_count:    AtomicU64::new(0),
        }
    }
}

pub static TMPFS_STATS: TmpfsStats = TmpfsStats::new();
