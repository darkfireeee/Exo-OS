// kernel/src/fs/pseudo/devfs.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// DEVFS — /dev virtual filesystem (Exo-OS · Couche 3)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Expose les périphériques caractère et bloc dans /dev.
//
// Périphériques pré-registrés :
//   /dev/null      → discard all writes, read EOF
//   /dev/zero      → produce infinite 0 bytes
//   /dev/full      → write returns ENOSPC
//   /dev/random    → pseudo-random bytes (LCG kernel)
//   /dev/urandom   → idem (non-bloquant)
//   /dev/tty       → terminal courant
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, Ordering};
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::fs::core::types::{
    DevId, FsError, FsResult, FileMode, FileType, InodeNumber, Stat, SeekWhence, Dirent64,
};
use crate::fs::core::vfs::{FileHandle, FileOps, InodeOps};
use crate::fs::core::inode::{Inode, InodeRef, new_inode_ref};
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────────────────────
// Périphériques spéciaux
// ─────────────────────────────────────────────────────────────────────────────

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DevSpecial {
    Null    = 0,
    Zero    = 1,
    Full    = 2,
    Random  = 3,
    Urandom = 4,
    Tty     = 5,
}

// ─────────────────────────────────────────────────────────────────────────────
// Générateur LCG pour /dev/random (non-cryptographique)
// ─────────────────────────────────────────────────────────────────────────────

static RNG_STATE: AtomicU64 = AtomicU64::new(0xDEADBEEFCAFEBABE);

fn lcg_next() -> u8 {
    let s = RNG_STATE.fetch_add(6364136223846793005, Ordering::Relaxed);
    ((s ^ (s >> 33)) >> 24) as u8
}

// ─────────────────────────────────────────────────────────────────────────────
// DevFileOps
// ─────────────────────────────────────────────────────────────────────────────

pub struct DevFileOps {
    pub kind: DevSpecial,
}

impl FileOps for DevFileOps {
    fn read(&self, _fh: &FileHandle, buf: &mut [u8], _offset: u64) -> FsResult<usize> {
        match self.kind {
            DevSpecial::Null    => Ok(0),
            DevSpecial::Zero    => { buf.fill(0); Ok(buf.len()) }
            DevSpecial::Full    => Err(FsError::NoSpace),
            DevSpecial::Random | DevSpecial::Urandom => {
                for b in buf.iter_mut() { *b = lcg_next(); }
                DEV_STATS.random_bytes.fetch_add(buf.len() as u64, Ordering::Relaxed);
                Ok(buf.len())
            }
            DevSpecial::Tty => Err(FsError::Again),
        }
    }

    fn write(&self, _fh: &FileHandle, buf: &[u8], _offset: u64) -> FsResult<usize> {
        match self.kind {
            DevSpecial::Null | DevSpecial::Zero => {
                DEV_STATS.null_bytes.fetch_add(buf.len() as u64, Ordering::Relaxed);
                Ok(buf.len())
            }
            DevSpecial::Full  => Err(FsError::NoSpace),
            DevSpecial::Tty   => Ok(buf.len()),
            _                 => Ok(buf.len()),
        }
    }

    fn seek(&self, _fh: &FileHandle, _off: i64, _w: SeekWhence) -> FsResult<u64> {
        // La plupart des devices ne supportent pas seek.
        Err(FsError::NotSupported)
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
// DevfsEntry
// ─────────────────────────────────────────────────────────────────────────────

pub struct DevfsEntry {
    pub name: Vec<u8>,
    pub ino:  InodeNumber,
    pub kind: DevSpecial,
    pub rdev: DevId,
}

// ─────────────────────────────────────────────────────────────────────────────
// DevfsRegistry
// ─────────────────────────────────────────────────────────────────────────────

pub struct DevfsRegistry {
    entries: SpinLock<Vec<DevfsEntry>>,
    next_ino: AtomicU64,
}

impl DevfsRegistry {
    pub const fn new() -> Self {
        Self { entries: SpinLock::new(Vec::new()), next_ino: AtomicU64::new(100) }
    }

    fn register_builtin(&self) {
        let builtins = [
            (b"null"   as &[u8], DevSpecial::Null,    DevId(0x0101)),
            (b"zero"             , DevSpecial::Zero,    DevId(0x0105)),
            (b"full"             , DevSpecial::Full,    DevId(0x0107)),
            (b"random"           , DevSpecial::Random,  DevId(0x0108)),
            (b"urandom"          , DevSpecial::Urandom, DevId(0x0109)),
            (b"tty"              , DevSpecial::Tty,     DevId(0x0400)),
        ];
        let mut entries = self.entries.lock();
        for (name, kind, rdev) in &builtins {
            let ino = self.next_ino.fetch_add(1, Ordering::Relaxed);
            entries.push(DevfsEntry { name: name.to_vec(), ino: InodeNumber(ino), kind: *kind, rdev: *rdev });
        }
    }

    pub fn lookup(&self, name: &[u8]) -> Option<InodeNumber> {
        let entries = self.entries.lock();
        entries.iter().find(|e| e.name.as_slice() == name).map(|e| e.ino)
    }

    pub fn get_kind(&self, ino: InodeNumber) -> Option<DevSpecial> {
        let entries = self.entries.lock();
        entries.iter().find(|e| e.ino == ino).map(|e| e.kind)
    }
}

pub static DEVFS_REGISTRY: DevfsRegistry = DevfsRegistry::new();

pub fn devfs_init() {
    DEVFS_REGISTRY.register_builtin();
}

// ─────────────────────────────────────────────────────────────────────────────
// DevStats
// ─────────────────────────────────────────────────────────────────────────────

pub struct DevStats {
    pub null_bytes:   AtomicU64,
    pub zero_bytes:   AtomicU64,
    pub random_bytes: AtomicU64,
}

impl DevStats {
    pub const fn new() -> Self {
        Self {
            null_bytes:   AtomicU64::new(0),
            zero_bytes:   AtomicU64::new(0),
            random_bytes: AtomicU64::new(0),
        }
    }
}

pub static DEV_STATS: DevStats = DevStats::new();
