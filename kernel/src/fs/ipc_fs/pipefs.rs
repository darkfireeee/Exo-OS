// kernel/src/fs/ipc_fs/pipefs.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// PIPEFS — FS shim pour les pipes POSIX (Exo-OS · Couche 3)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Couche FS virtuelle au-dessus des pipes IPC.
//
// RÈGLE D'ARCHITECTURE : ipc_fs/ est l'unique point d'accès de fs/ vers ipc/.
// Aucun autre module fs/ ne doit importer crate::ipc::* directement.
//
// Le module délègue toutes les opérations de données vers l'IPC subsystem
// via un pointeur de fonction (capability bridge) enregistré au boot.
//
// Structure d'un pipe :
//   • Anneau circulaire de 64 KiB (PIPE_BUF_SIZE).
//   • Deux fds : lecteur (O_RDONLY) et écrivain (O_WRONLY).
//   • Sémantique POSIX : write atomique si len ≤ PIPE_BUF (4096).
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::boxed::Box;

use crate::fs::core::types::{FsError, FsResult, FileMode, FileType, InodeNumber, Stat, SeekWhence, Dirent64, FS_STATS};
use crate::fs::core::vfs::{FileHandle, FileOps};
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Taille du buffer interne d'un pipe (64 KiB).
pub const PIPE_BUF_SIZE: usize = 65536;
/// Taille maximale d'une écriture atomique (POSIX PIPE_BUF).
pub const PIPE_BUF: usize = 4096;

// ─────────────────────────────────────────────────────────────────────────────
// PipeBuffer — anneau circulaire
// ─────────────────────────────────────────────────────────────────────────────

struct PipeBuffer {
    buf:  Box<[u8; PIPE_BUF_SIZE]>,
    head: usize,   // prochaine lecture
    tail: usize,   // prochaine écriture
    len:  usize,   // octets disponibles
}

impl PipeBuffer {
    fn new() -> Self {
        Self {
            buf:  alloc::boxed::Box::new([0u8; PIPE_BUF_SIZE]),
            head: 0,
            tail: 0,
            len:  0,
        }
    }

    fn available(&self) -> usize { self.len }
    fn free_space(&self) -> usize { PIPE_BUF_SIZE - self.len }

    fn read(&mut self, out: &mut [u8]) -> usize {
        let n = out.len().min(self.len);
        for i in 0..n {
            out[i] = self.buf[self.head];
            self.head = (self.head + 1) % PIPE_BUF_SIZE;
        }
        self.len -= n;
        n
    }

    fn write(&mut self, data: &[u8]) -> usize {
        let n = data.len().min(self.free_space());
        for i in 0..n {
            self.buf[self.tail] = data[i];
            self.tail = (self.tail + 1) % PIPE_BUF_SIZE;
        }
        self.len += n;
        n
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PipeInner — état partagé entre les deux extrémités
// ─────────────────────────────────────────────────────────────────────────────

pub struct PipeInner {
    buffer:       SpinLock<PipeBuffer>,
    readers_open: AtomicUsize,
    writers_open: AtomicUsize,
    bytes_read:   AtomicU64,
    bytes_written:AtomicU64,
}

impl PipeInner {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            buffer:        SpinLock::new(PipeBuffer::new()),
            readers_open:  AtomicUsize::new(1),
            writers_open:  AtomicUsize::new(1),
            bytes_read:    AtomicU64::new(0),
            bytes_written: AtomicU64::new(0),
        })
    }

    pub fn write_data(&self, data: &[u8]) -> FsResult<usize> {
        if self.readers_open.load(Ordering::Relaxed) == 0 {
            return Err(FsError::BrokenPipe);
        }
        let mut buf = self.buffer.lock();
        if buf.free_space() < data.len() {
            return Err(FsError::Again); // EAGAIN en mode non-bloquant
        }
        let n = buf.write(data);
        self.bytes_written.fetch_add(n as u64, Ordering::Relaxed);
        PIPE_STATS.bytes_written.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }

    pub fn read_data(&self, out: &mut [u8]) -> FsResult<usize> {
        let mut buf = self.buffer.lock();
        if buf.available() == 0 {
            if self.writers_open.load(Ordering::Relaxed) == 0 { return Ok(0); } // EOF
            return Err(FsError::Again);
        }
        let n = buf.read(out);
        self.bytes_read.fetch_add(n as u64, Ordering::Relaxed);
        PIPE_STATS.bytes_read.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }

    pub fn close_reader(&self) { self.readers_open.fetch_sub(1, Ordering::Relaxed); }
    pub fn close_writer(&self) { self.writers_open.fetch_sub(1, Ordering::Relaxed); }
    pub fn available(&self)    -> usize { self.buffer.lock().available() }
}

// ─────────────────────────────────────────────────────────────────────────────
// PipeReadOps / PipeWriteOps — FileOps pour chaque extrémité
// ─────────────────────────────────────────────────────────────────────────────

pub struct PipeReadOps { pub inner: Arc<PipeInner> }
pub struct PipeWriteOps { pub inner: Arc<PipeInner> }

impl FileOps for PipeReadOps {
    fn read(&self, _fh: &FileHandle, buf: &mut [u8], _offset: u64) -> FsResult<usize> {
        self.inner.read_data(buf)
    }
    fn write(&self, _fh: &FileHandle, _buf: &[u8], _offset: u64) -> FsResult<usize> {
        Err(FsError::BadFd)
    }
    fn seek(&self, _fh: &FileHandle, _off: i64, _w: SeekWhence) -> FsResult<u64> {
        Err(FsError::NotSupported)
    }
    fn release(&self, _fh: &FileHandle) -> FsResult<()> {
        self.inner.close_reader();
        Ok(())
    }
    fn fsync(&self, _fh: &FileHandle, _: bool) -> FsResult<()> { Ok(()) }
    fn ioctl(&self, _fh: &FileHandle, _cmd: u32, _arg: u64) -> FsResult<i64> { Err(FsError::NotSupported) }
    fn mmap(&self, _fh: &FileHandle, _offset: u64, _len: usize, _flags: crate::fs::core::vfs::MmapFlags) -> FsResult<u64> { Err(FsError::NotSupported) }
    fn readdir(&self, _fh: &FileHandle, _offset: &mut u64, _emit: &mut dyn FnMut(Dirent64) -> bool) -> FsResult<()> { Err(FsError::NotDir) }
    fn fallocate(&self, _fh: &FileHandle, _mode: u32, _offset: u64, _len: u64) -> FsResult<()> { Err(FsError::NotSupported) }
    fn poll(&self, _fh: &FileHandle) -> FsResult<crate::fs::core::vfs::PollEvents> { Ok(crate::fs::core::vfs::PollEvents(0x0001)) } // POLLIN
}

impl FileOps for PipeWriteOps {
    fn read(&self, _fh: &FileHandle, _buf: &mut [u8], _offset: u64) -> FsResult<usize> {
        Err(FsError::BadFd)
    }
    fn write(&self, _fh: &FileHandle, buf: &[u8], _offset: u64) -> FsResult<usize> {
        self.inner.write_data(buf)
    }
    fn seek(&self, _fh: &FileHandle, _off: i64, _w: SeekWhence) -> FsResult<u64> {
        Err(FsError::NotSupported)
    }
    fn release(&self, _fh: &FileHandle) -> FsResult<()> {
        self.inner.close_writer();
        Ok(())
    }
    fn fsync(&self, _fh: &FileHandle, _: bool) -> FsResult<()> { Ok(()) }
    fn ioctl(&self, _fh: &FileHandle, _cmd: u32, _arg: u64) -> FsResult<i64> { Err(FsError::NotSupported) }
    fn mmap(&self, _fh: &FileHandle, _offset: u64, _len: usize, _flags: crate::fs::core::vfs::MmapFlags) -> FsResult<u64> { Err(FsError::NotSupported) }
    fn readdir(&self, _fh: &FileHandle, _offset: &mut u64, _emit: &mut dyn FnMut(Dirent64) -> bool) -> FsResult<()> { Err(FsError::NotDir) }
    fn fallocate(&self, _fh: &FileHandle, _mode: u32, _offset: u64, _len: u64) -> FsResult<()> { Err(FsError::NotSupported) }
    fn poll(&self, _fh: &FileHandle) -> FsResult<crate::fs::core::vfs::PollEvents> { Ok(crate::fs::core::vfs::PollEvents(0x0004)) } // POLLOUT
}

// ─────────────────────────────────────────────────────────────────────────────
// Création d'un pipe (retourne deux Arc<dyn FileOps>)
// ─────────────────────────────────────────────────────────────────────────────

pub fn create_pipe() -> (Arc<dyn FileOps>, Arc<dyn FileOps>) {
    let inner = PipeInner::new();
    PIPE_STATS.created.fetch_add(1, Ordering::Relaxed);
    FS_STATS.open_files.fetch_add(2, Ordering::Relaxed);
    (Arc::new(PipeReadOps { inner: inner.clone() }),
     Arc::new(PipeWriteOps { inner }))
}

// ─────────────────────────────────────────────────────────────────────────────
// PipeStats
// ─────────────────────────────────────────────────────────────────────────────

pub struct PipeStats {
    pub created:       AtomicU64,
    pub bytes_read:    AtomicU64,
    pub bytes_written: AtomicU64,
    pub broken_pipe:   AtomicU64,
}

impl PipeStats {
    pub const fn new() -> Self {
        Self {
            created:       AtomicU64::new(0),
            bytes_read:    AtomicU64::new(0),
            bytes_written: AtomicU64::new(0),
            broken_pipe:   AtomicU64::new(0),
        }
    }
}

pub static PIPE_STATS: PipeStats = PipeStats::new();
