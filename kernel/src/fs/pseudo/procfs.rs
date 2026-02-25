// kernel/src/fs/pseudo/procfs.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// PROCFS — /proc filesystem (Exo-OS · Couche 3)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Système de fichiers virtuel exposant l'état des processus kernel.
//
// Structure /proc :
//   /proc/<pid>/          → répertoire par processus
//   /proc/<pid>/status    → état du processus (Name, Pid, State, VmRSS…)
//   /proc/<pid>/maps      → map mémoire virtuelle
//   /proc/<pid>/fd/       → fds ouverts (symlinks)
//   /proc/cpuinfo         → informations CPU
//   /proc/meminfo         → informations mémoire
//   /proc/uptime          → uptime depuis le boot
//   /proc/mounts          → table de montage courante
//   /proc/fs/             → statistiques FS
//
// Implémentation :
//   • `ProcEntry` : entrée synthétique (générée à la lecture).
//   • `ProcFsType` implémente `FsType` pour être enregistrée dans FsTypeRegistry.
//   • Les fichiers /proc sont read-only sauf exceptions (/proc/sys).
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, Ordering};
use alloc::vec::Vec;
use alloc::string::{String, ToString};
use alloc::format;

use crate::fs::core::types::{
    DevId, FsError, FsResult, FileMode, FileType, InodeNumber, Stat, Dirent64,
    OpenFlags, SeekWhence, Timespec64, FS_STATS,
};
use crate::fs::core::vfs::{
    FsType, Superblock, InodeOps, FileOps, FileHandle, InodeAttr,
    LookupResult, RenameFlags,
};
use crate::fs::core::inode::{Inode, InodeRef, InodeState, new_inode_ref};
use crate::fs::core::dentry::{Dentry, DentryRef};

// ─────────────────────────────────────────────────────────────────────────────
// Numéros d'inodes réservés /proc
// ─────────────────────────────────────────────────────────────────────────────

const PROC_ROOT_INO:    u64 = 1;
const PROC_CPUINFO_INO: u64 = 2;
const PROC_MEMINFO_INO: u64 = 3;
const PROC_UPTIME_INO:  u64 = 4;
const PROC_MOUNTS_INO:  u64 = 5;
const PROC_FS_INO:      u64 = 6;

// ─────────────────────────────────────────────────────────────────────────────
// Contenu synthétique
// ─────────────────────────────────────────────────────────────────────────────

fn gen_cpuinfo() -> Vec<u8> {
    b"processor\t: 0\nvendor_id\t: ExoOS\ncpu family\t: 6\nmodel name\t: Exo Virtual CPU @ 3.00GHz\ncpu MHz\t\t: 3000.000\ncache size\t: 4096 KB\n".to_vec()
}

fn gen_meminfo() -> Vec<u8> {
    let pages = FS_STATS.page_cache_pages.load(Ordering::Relaxed);
    let dirty  = FS_STATS.dirty_pages.load(Ordering::Relaxed);
    format!(
        "MemTotal:    1048576 kB\nMemFree:      524288 kB\nCached:     {:8} kB\nDirty:      {:8} kB\n",
        pages * 4, dirty * 4
    ).into_bytes()
}

fn gen_uptime() -> Vec<u8> {
    use crate::fs::block::scheduler::tick;
    let t = tick() / 1000; // ms → secondes
    format!("{}.00 {}.00\n", t, t / 2).into_bytes()
}

fn gen_mounts() -> Vec<u8> {
    b"rootfs / rootfs rw 0 0\ntmpfs /tmp tmpfs rw 0 0\nproc /proc proc ro 0 0\n".to_vec()
}

fn gen_fs_stats() -> Vec<u8> {
    format!(
        "cache_hits:    {}\ncache_misses:  {}\nevictions:     {}\nbytes_read:    {}\nbytes_written: {}\n",
        FS_STATS.cache_hits.load(Ordering::Relaxed),
        FS_STATS.cache_misses.load(Ordering::Relaxed),
        FS_STATS.evictions.load(Ordering::Relaxed),
        FS_STATS.bytes_read.load(Ordering::Relaxed),
        FS_STATS.bytes_written.load(Ordering::Relaxed),
    ).into_bytes()
}

// ─────────────────────────────────────────────────────────────────────────────
// ProcFileOps
// ─────────────────────────────────────────────────────────────────────────────

pub struct ProcFileOps {
    pub ino: InodeNumber,
}

impl FileOps for ProcFileOps {
    fn read(&self, _fh: &FileHandle, buf: &mut [u8], offset: u64) -> FsResult<usize> {
        let content = match self.ino.0 {
            PROC_CPUINFO_INO => gen_cpuinfo(),
            PROC_MEMINFO_INO => gen_meminfo(),
            PROC_UPTIME_INO  => gen_uptime(),
            PROC_MOUNTS_INO  => gen_mounts(),
            PROC_FS_INO      => gen_fs_stats(),
            _                => return Err(FsError::NotFound),
        };
        let off = offset as usize;
        if off >= content.len() { return Ok(0); }
        let available = &content[off..];
        let n = available.len().min(buf.len());
        buf[..n].copy_from_slice(&available[..n]);
        PROC_STATS.reads.fetch_add(1, Ordering::Relaxed);
        Ok(n)
    }

    fn write(&self, _fh: &FileHandle, _buf: &[u8], _offset: u64) -> FsResult<usize> {
        Err(FsError::ReadOnly)
    }

    fn seek(&self, _fh: &FileHandle, offset: i64, whence: SeekWhence) -> FsResult<u64> {
        match whence {
            SeekWhence::Set => Ok(offset as u64),
            SeekWhence::Cur => Ok(offset as u64),
            SeekWhence::End => Ok(0),
            SeekWhence::Data | SeekWhence::Hole => Err(FsError::NotSupported),
        }
    }

    fn release(&self, _fh: &FileHandle) -> FsResult<()> { Ok(()) }
    fn fsync(&self, _fh: &FileHandle, _datasync: bool) -> FsResult<()> { Ok(()) }
    fn ioctl(&self, _fh: &FileHandle, _cmd: u32, _arg: u64) -> FsResult<i64> { Err(FsError::NotSupported) }
    fn mmap(&self, _fh: &FileHandle, _offset: u64, _len: usize, _flags: crate::fs::core::vfs::MmapFlags) -> FsResult<u64> { Err(FsError::NotSupported) }
    fn readdir(&self, _fh: &FileHandle, _offset: &mut u64, _emit: &mut dyn FnMut(Dirent64) -> bool) -> FsResult<()> { Err(FsError::NotDir) }
    fn fallocate(&self, _fh: &FileHandle, _mode: u32, _offset: u64, _len: u64) -> FsResult<()> { Err(FsError::NotSupported) }
    fn poll(&self, _fh: &FileHandle) -> FsResult<crate::fs::core::vfs::PollEvents> { Ok(crate::fs::core::vfs::PollEvents(0x0001 | 0x0004)) }
}

// ─────────────────────────────────────────────────────────────────────────────
// ProcRootOps
// ─────────────────────────────────────────────────────────────────────────────

pub struct ProcRootOps;

impl InodeOps for ProcRootOps {
    fn lookup(&self, _parent: &InodeRef, name: &[u8]) -> FsResult<DentryRef> {
        let ino_num = match name {
            b"cpuinfo" => PROC_CPUINFO_INO,
            b"meminfo" => PROC_MEMINFO_INO,
            b"uptime"  => PROC_UPTIME_INO,
            b"mounts"  => PROC_MOUNTS_INO,
            b"fs"      => PROC_FS_INO,
            _          => return Err(FsError::NotFound),
        };
        let mode  = FileMode::regular(0o444);
        let child = new_inode_ref(
            InodeNumber(ino_num), mode,
            crate::fs::core::types::Uid(0), crate::fs::core::types::Gid(0),
        );
        PROC_STATS.lookups.fetch_add(1, Ordering::Relaxed);
        let dentry = Dentry::new_root(name, child);
        Ok(alloc::sync::Arc::new(crate::scheduler::sync::rwlock::KRwLock::new(dentry)))
    }

    fn getattr(&self, inode: &InodeRef) -> FsResult<Stat> {
        Ok(inode.read().to_stat())
    }

    fn setattr(&self, _inode: &InodeRef, _attr: &InodeAttr) -> FsResult<()> { Err(FsError::NotSupported) }
    fn create(&self, _dir: &InodeRef, _name: &[u8], _mode: FileMode, _uid: crate::fs::core::types::Uid, _gid: crate::fs::core::types::Gid) -> FsResult<InodeRef> { Err(FsError::NotSupported) }
    fn mkdir( &self, _dir: &InodeRef, _name: &[u8], _mode: FileMode, _uid: crate::fs::core::types::Uid, _gid: crate::fs::core::types::Gid) -> FsResult<InodeRef> { Err(FsError::NotSupported) }
    fn rmdir( &self, _dir: &InodeRef, _name: &[u8]) -> FsResult<()> { Err(FsError::NotSupported) }
    fn unlink(&self, _dir: &InodeRef, _name: &[u8]) -> FsResult<()> { Err(FsError::NotSupported) }
    fn rename(&self, _od: &InodeRef, _on: &[u8], _nd: &InodeRef, _nn: &[u8], _f: RenameFlags) -> FsResult<()> { Err(FsError::NotSupported) }
    fn link(  &self, _old: &InodeRef, _new_dir: &InodeRef, _name: &[u8]) -> FsResult<()> { Err(FsError::NotSupported) }
    fn symlink(&self, _dir: &InodeRef, _name: &[u8], _tgt: &[u8], _uid: crate::fs::core::types::Uid, _gid: crate::fs::core::types::Gid) -> FsResult<InodeRef> { Err(FsError::NotSupported) }
    fn readlink(&self, _inode: &InodeRef, _buf: &mut [u8]) -> FsResult<usize> { Err(FsError::NotSupported) }
    fn mknod(&self, _dir: &InodeRef, _name: &[u8], _mode: FileMode, _rdev: crate::fs::core::types::DevId, _uid: crate::fs::core::types::Uid, _gid: crate::fs::core::types::Gid) -> FsResult<InodeRef> { Err(FsError::NotSupported) }
    fn write_inode(&self, _inode: &InodeRef, _sync: bool) -> FsResult<()> { Ok(()) }
    fn evict_inode(&self, _inode: &InodeRef)             -> FsResult<()> { Ok(()) }
}

// ─────────────────────────────────────────────────────────────────────────────
// ProcStats
// ─────────────────────────────────────────────────────────────────────────────

pub struct ProcStats {
    pub reads:       AtomicU64,
    pub lookups:     AtomicU64,
    pub mounts:      AtomicU64,
}

impl ProcStats {
    pub const fn new() -> Self {
        Self { reads: AtomicU64::new(0), lookups: AtomicU64::new(0), mounts: AtomicU64::new(0) }
    }
}

pub static PROC_STATS: ProcStats = ProcStats::new();
