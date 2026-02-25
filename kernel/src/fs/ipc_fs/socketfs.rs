// kernel/src/fs/ipc_fs/socketfs.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// SOCKETFS — FS shim pour les sockets AF_UNIX (Exo-OS · Couche 3)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Implémente les inodes VFS pour les sockets de domaine Unix (AF_UNIX).
// Le transfert de données utilise le même anneau circulaire que pipefs,
// via un canal bidirectionnel (deux PipeInner : rx et tx).
//
// États :
//   Unbound   → create_socketfs_inode()
//   Bound     → bind() — associe un nom dans le namespace FS
//   Listening → listen() — attend des connexions entrantes
//   Connected → connect() / accept()
//   Closed    → close() / shutdown()
//
// RÈGLE D'ARCHITECTURE : aucun import de crate::ipc::* hors de ce module.
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::string::String;

use crate::fs::core::types::{FsError, FsResult, InodeNumber, SeekWhence, Dirent64, FS_STATS};
use crate::fs::core::vfs::{FileHandle, FileOps};
use crate::fs::ipc_fs::pipefs::PipeInner;
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────────────────────
// UnixSocketState
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum UnixSocketState {
    Unbound   = 0,
    Bound     = 1,
    Listening = 2,
    Connected = 3,
    Closed    = 4,
}

impl UnixSocketState {
    fn from_u8(v: u8) -> Self {
        match v { 1 => Self::Bound, 2 => Self::Listening,
                  3 => Self::Connected, 4 => Self::Closed, _ => Self::Unbound }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// UnixSocketInner — état partagé entre les deux extrémités d'un socketpair
// ─────────────────────────────────────────────────────────────────────────────

pub struct UnixSocketInner {
    state:      AtomicU8,
    path:       SpinLock<Option<String>>,
    // Pour les sockets connectées : deux canaux (full-duplex)
    send:       SpinLock<Option<Arc<PipeInner>>>,
    recv:       SpinLock<Option<Arc<PipeInner>>>,
    // File d'attente de connexions en attente (Listening)
    backlog:    SpinLock<Vec<Arc<UnixSocketInner>>>,
    backlog_max:AtomicU32,
    // Identifiant unique de socket
    id:         u64,
}

static SOCK_ID: AtomicU64 = AtomicU64::new(1);

impl UnixSocketInner {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state:       AtomicU8::new(UnixSocketState::Unbound as u8),
            path:        SpinLock::new(None),
            send:        SpinLock::new(None),
            recv:        SpinLock::new(None),
            backlog:     SpinLock::new(Vec::new()),
            backlog_max: AtomicU32::new(128),
            id:          SOCK_ID.fetch_add(1, Ordering::Relaxed),
        })
    }

    pub fn state(&self) -> UnixSocketState {
        UnixSocketState::from_u8(self.state.load(Ordering::Acquire))
    }

    pub fn bind(&self, path: String) -> FsResult<()> {
        let mut p = self.path.lock();
        if p.is_some() { return Err(FsError::AlreadyExists); }
        *p = Some(path);
        self.state.store(UnixSocketState::Bound as u8, Ordering::Release);
        SOCK_STATS.bind_calls.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    pub fn listen(&self, backlog: usize) -> FsResult<()> {
        if self.state() != UnixSocketState::Bound { return Err(FsError::InvalidArgument); }
        self.backlog_max.store(backlog as u32, Ordering::Relaxed);
        self.state.store(UnixSocketState::Listening as u8, Ordering::Release);
        SOCK_STATS.listen_calls.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    pub fn accept(&self) -> FsResult<Arc<UnixSocketInner>> {
        if self.state() != UnixSocketState::Listening { return Err(FsError::InvalidArgument); }
        let mut bl = self.backlog.lock();
        if bl.is_empty() { return Err(FsError::Again); }
        let peer = bl.remove(0);
        SOCK_STATS.accept_calls.fetch_add(1, Ordering::Relaxed);
        Ok(peer)
    }

    pub fn connect(&self, listener: &Arc<UnixSocketInner>) -> FsResult<()> {
        if listener.state() != UnixSocketState::Listening { return Err(FsError::ConnectionRefused); }
        let max = listener.backlog_max.load(Ordering::Relaxed) as usize;
        let mut bl = listener.backlog.lock();
        if bl.len() >= max { return Err(FsError::Again); }

        // Crée un canal bidirectionnel
        let ch_a = PipeInner::new();
        let ch_b = PipeInner::new();

        *self.send.lock() = Some(ch_a.clone());
        *self.recv.lock() = Some(ch_b.clone());

        // La connexion côté pair (créée ici comme proxy) sera récupérée via accept()
        let peer = UnixSocketInner::new();
        *peer.send.lock() = Some(ch_b);
        *peer.recv.lock() = Some(ch_a);
        peer.state.store(UnixSocketState::Connected as u8, Ordering::Release);

        self.state.store(UnixSocketState::Connected as u8, Ordering::Release);
        bl.push(peer);
        SOCK_STATS.connect_calls.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    pub fn send_data(&self, data: &[u8]) -> FsResult<usize> {
        let guard = self.send.lock();
        let pipe = guard.as_ref().ok_or(FsError::NotConnected)?;
        let n = pipe.write_data(data)?;
        SOCK_STATS.bytes_sent.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }

    pub fn recv_data(&self, buf: &mut [u8]) -> FsResult<usize> {
        let guard = self.recv.lock();
        let pipe = guard.as_ref().ok_or(FsError::NotConnected)?;
        let n = pipe.read_data(buf)?;
        SOCK_STATS.bytes_recv.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }

    pub fn close(&self) {
        self.state.store(UnixSocketState::Closed as u8, Ordering::Release);
        *self.send.lock() = None;
        *self.recv.lock() = None;
        SOCK_STATS.closed.fetch_add(1, Ordering::Relaxed);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// socketpair() — crée deux sockets connectées
// ─────────────────────────────────────────────────────────────────────────────

pub fn socketpair() -> (Arc<UnixSocketInner>, Arc<UnixSocketInner>) {
    let a = UnixSocketInner::new();
    let b = UnixSocketInner::new();

    let ch_a = PipeInner::new();
    let ch_b = PipeInner::new();

    *a.send.lock() = Some(ch_a.clone());
    *a.recv.lock() = Some(ch_b.clone());
    *b.send.lock() = Some(ch_b);
    *b.recv.lock() = Some(ch_a);

    a.state.store(UnixSocketState::Connected as u8, Ordering::Release);
    b.state.store(UnixSocketState::Connected as u8, Ordering::Release);

    SOCK_STATS.created.fetch_add(2, Ordering::Relaxed);
    FS_STATS.open_files.fetch_add(2, Ordering::Relaxed);
    (a, b)
}

// ─────────────────────────────────────────────────────────────────────────────
// UnixSocketOps — FileOps VFS
// ─────────────────────────────────────────────────────────────────────────────

pub struct UnixSocketOps { pub inner: Arc<UnixSocketInner> }

impl FileOps for UnixSocketOps {
    fn read(&self, _fh: &FileHandle, buf: &mut [u8], _off: u64) -> FsResult<usize> {
        self.inner.recv_data(buf)
    }
    fn write(&self, _fh: &FileHandle, buf: &[u8], _off: u64) -> FsResult<usize> {
        self.inner.send_data(buf)
    }
    fn seek(&self, _fh: &FileHandle, _off: i64, _w: SeekWhence) -> FsResult<u64> {
        Err(FsError::NotSupported)
    }
    fn release(&self, _fh: &FileHandle) -> FsResult<()> {
        self.inner.close();
        Ok(())
    }
    fn fsync(&self, _fh: &FileHandle, _: bool) -> FsResult<()> { Ok(()) }
    fn ioctl(&self, _fh: &FileHandle, _cmd: u32, _arg: u64) -> FsResult<i64> { Err(FsError::NotSupported) }
    fn mmap(&self, _fh: &FileHandle, _offset: u64, _len: usize, _flags: crate::fs::core::vfs::MmapFlags) -> FsResult<u64> { Err(FsError::NotSupported) }
    fn readdir(&self, _fh: &FileHandle, _offset: &mut u64, _emit: &mut dyn FnMut(Dirent64) -> bool) -> FsResult<()> { Err(FsError::NotDir) }
    fn fallocate(&self, _fh: &FileHandle, _mode: u32, _offset: u64, _len: u64) -> FsResult<()> { Err(FsError::NotSupported) }
    fn poll(&self, _fh: &FileHandle) -> FsResult<crate::fs::core::vfs::PollEvents> { Ok(crate::fs::core::vfs::PollEvents(0x0001 | 0x0004)) }
}

// ─────────────────────────────────────────────────────────────────────────────
// Registre de sockets AF_UNIX nommées
// ─────────────────────────────────────────────────────────────────────────────

struct SockEntry {
    path:   String,
    socket: Arc<UnixSocketInner>,
}

pub struct UnixSocketRegistry {
    sockets: SpinLock<Vec<SockEntry>>,
}

impl UnixSocketRegistry {
    pub const fn new() -> Self { Self { sockets: SpinLock::new(Vec::new()) } }

    pub fn register(&self, path: String, sock: Arc<UnixSocketInner>) -> FsResult<()> {
        let mut guard = self.sockets.lock();
        if guard.iter().any(|e| e.path == path) { return Err(FsError::AlreadyExists); }
        guard.push(SockEntry { path, socket: sock });
        Ok(())
    }

    pub fn lookup(&self, path: &str) -> Option<Arc<UnixSocketInner>> {
        let guard = self.sockets.lock();
        guard.iter().find(|e| e.path == path).map(|e| e.socket.clone())
    }

    pub fn unregister(&self, path: &str) {
        let mut guard = self.sockets.lock();
        guard.retain(|e| e.path != path);
    }
}

pub static UNIX_SOCK_REGISTRY: UnixSocketRegistry = UnixSocketRegistry::new();

// ─────────────────────────────────────────────────────────────────────────────
// SockStats
// ─────────────────────────────────────────────────────────────────────────────

pub struct SockStats {
    pub created:      AtomicU64,
    pub closed:       AtomicU64,
    pub bind_calls:   AtomicU64,
    pub listen_calls: AtomicU64,
    pub connect_calls:AtomicU64,
    pub accept_calls: AtomicU64,
    pub bytes_sent:   AtomicU64,
    pub bytes_recv:   AtomicU64,
}

impl SockStats {
    pub const fn new() -> Self {
        Self {
            created:       AtomicU64::new(0),
            closed:        AtomicU64::new(0),
            bind_calls:    AtomicU64::new(0),
            listen_calls:  AtomicU64::new(0),
            connect_calls: AtomicU64::new(0),
            accept_calls:  AtomicU64::new(0),
            bytes_sent:    AtomicU64::new(0),
            bytes_recv:    AtomicU64::new(0),
        }
    }
}

pub static SOCK_STATS: SockStats = SockStats::new();
