// ipc/channel/streaming.rs — Canal de streaming continu pour Exo-OS
//
// Conçu pour les transferts de grandes quantités de données (flux vidéo, audio,
// DMA buffers). Utilise le ZeroCopyRing pour partager des pages physiques entre
// producteur et consommateur sans aucune copie.
//
// Architecture :
//   - ZeroCopyRing interne (512 slots de ZeroCopyRef)
//   - Chaque ZeroCopyRef référence une page physique pré-allouée
//   - Producteur : prend un buffer libre, remplit, envoie via push()
//   - Consommateur : reçoit via pop(), consomme, libère via release()
//   - Flow control : le producteur ne peut pas outrepasser le consommateur
//     (ring plein = backpressure, pas de drop)
//
// Granularité : configurable de 4 Ko (PAGE_SIZE) à 2 Mo (HUGE_PAGE)

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use core::mem::MaybeUninit;

use crate::ipc::core::types::{ChannelId, IpcError, MessageId, alloc_channel_id, alloc_message_id};
use crate::ipc::core::constants::SHM_POOL_PAGES;
use crate::ipc::ring::zerocopy::{ZeroCopyRing, ZeroCopyBuffer, ZeroCopyRef};
use crate::ipc::stats::counters::{IPC_STATS, StatEvent};
use crate::scheduler::sync::spinlock::SpinLock;

// ---------------------------------------------------------------------------
// Granularité des buffers streaming
// ---------------------------------------------------------------------------

/// Taille granule d'un buffer streaming (octets)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum StreamGranule {
    /// 4 KiB — page standard
    Page4K = 4096,
    /// 16 KiB — 4 pages consécutives
    Page16K = 16384,
    /// 64 KiB
    Page64K = 65536,
    /// 256 KiB
    Page256K = 262144,
    /// 2 MiB — huge page
    HugePage2M = 2097152,
}

impl StreamGranule {
    pub fn bytes(self) -> usize {
        self as usize
    }
}

// ---------------------------------------------------------------------------
// Pool de buffers streaming pré-alloués
// ---------------------------------------------------------------------------

/// Nombre maximal de buffers dans le pool d'un canal streaming
pub const STREAM_POOL_SIZE: usize = SHM_POOL_PAGES; // 256

/// Structure d'un buffer streaming en pool.
/// Les buffers ne sont pas vraiment alloués ici (noyau no_std sans allocateur
/// général) — on simule des adresses physiques monotones croissantes.
/// La mémoire physique réelle est supposée réservée par le memory manager.
#[repr(C, align(64))]
pub struct StreamBuffer {
    /// Référence zero-copy vers la page physique
    pub zc: ZeroCopyBuffer,
    /// Taille utile actuelle (remplie par le producteur)
    pub data_len: AtomicUsize,
    /// Buffer disponible dans le pool (non acquis)
    pub available: AtomicU32,
    _pad: [u8; 28],
}

// SAFETY: ZeroCopyBuffer est Sync via refcount atomique
unsafe impl Sync for StreamBuffer {}
unsafe impl Send for StreamBuffer {}

impl StreamBuffer {
    pub const fn new_uninit() -> Self {
        Self {
            zc: ZeroCopyBuffer::new_uninit(),
            data_len: AtomicUsize::new(0),
            available: AtomicU32::new(1), // disponible dès la création
            _pad: [0u8; 28],
        }
    }

    /// Initialise le buffer avec une adresse physique simulée.
    pub fn init(&mut self, phys_addr: u64, capacity: usize) {
        self.zc = ZeroCopyBuffer::init(phys_addr, capacity as u32, 0);
        self.available.store(1, Ordering::Release);
    }

    /// Acquiert le buffer pour le producteur.
    /// Retourne `true` si acquis, `false` si déjà en usage.
    pub fn acquire(&self) -> bool {
        self.available
            .compare_exchange(1, 0, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// Libère le buffer après consommation.
    pub fn release(&self) {
        self.data_len.store(0, Ordering::Relaxed);
        self.available.store(1, Ordering::Release);
    }

    pub fn is_available(&self) -> bool {
        self.available.load(Ordering::Acquire) != 0
    }
}

// ---------------------------------------------------------------------------
// Pool de buffers du canal streaming
// ---------------------------------------------------------------------------

/// Pool statique de buffers streaming
#[repr(C, align(64))]
pub struct StreamPool {
    buffers: [StreamBuffer; STREAM_POOL_SIZE],
    total: usize,
    base_phys: AtomicU64,
}

// SAFETY: accès protégé par les atomiques de StreamBuffer
unsafe impl Sync for StreamPool {}
unsafe impl Send for StreamPool {}

impl StreamPool {
    pub const fn new() -> Self {
        const INIT_BUF: StreamBuffer = StreamBuffer::new_uninit();
        Self {
            buffers: [INIT_BUF; STREAM_POOL_SIZE],
            total: 0,
            base_phys: AtomicU64::new(0),
        }
    }

    /// Initialise le pool avec une adresse physique de base.
    /// Chaque buffer occupe `granule` octets d'espace physique contigu.
    pub fn init(&mut self, base_phys: u64, granule: StreamGranule, count: usize) {
        let count = count.min(STREAM_POOL_SIZE);
        let granule_bytes = granule.bytes();
        let base = base_phys;
        self.base_phys.store(base, Ordering::Relaxed);

        for i in 0..count {
            let phys = base + (i * granule_bytes) as u64;
            self.buffers[i].init(phys, granule_bytes);
        }
        self.total = count;
    }

    /// Alloue un buffer libre. Retourne son index ou `None`.
    pub fn alloc_buffer(&self) -> Option<usize> {
        for i in 0..self.total {
            if self.buffers[i].acquire() {
                return Some(i);
            }
        }
        None
    }

    /// Libère le buffer à l'index `idx`.
    pub fn free_buffer(&self, idx: usize) -> bool {
        if idx < self.total {
            self.buffers[idx].release();
            true
        } else {
            false
        }
    }

    /// Retourne un ZeroCopyRef vers le buffer `idx`.
    pub fn make_ref(&self, idx: usize, data_len: usize) -> Option<ZeroCopyRef> {
        if idx < self.total {
            let buf = &self.buffers[idx];
            buf.data_len.store(data_len, Ordering::Relaxed);
            Some(buf.zc.to_ref())
        } else {
            None
        }
    }

    pub fn buffer_count(&self) -> usize {
        self.total
    }

    pub fn available_count(&self) -> usize {
        (0..self.total)
            .filter(|&i| self.buffers[i].is_available())
            .count()
    }
}

// ---------------------------------------------------------------------------
// Statistiques du canal streaming
// ---------------------------------------------------------------------------

#[repr(C, align(64))]
pub struct StreamStats {
    pub pushes_ok: AtomicU64,
    pub pushes_full: AtomicU64,
    pub pops_ok: AtomicU64,
    pub pops_empty: AtomicU64,
    pub bytes_transferred: AtomicU64,
    pub pool_allocs: AtomicU64,
    pub pool_frees: AtomicU64,
    pub backpressure_events: AtomicU64,
    _pad: [u8; 0],
}

impl StreamStats {
    pub const fn new() -> Self {
        Self {
            pushes_ok: AtomicU64::new(0),
            pushes_full: AtomicU64::new(0),
            pops_ok: AtomicU64::new(0),
            pops_empty: AtomicU64::new(0),
            bytes_transferred: AtomicU64::new(0),
            pool_allocs: AtomicU64::new(0),
            pool_frees: AtomicU64::new(0),
            backpressure_events: AtomicU64::new(0),
            _pad: [],
        }
    }

    pub fn snapshot(&self) -> StreamStatsSnapshot {
        StreamStatsSnapshot {
            pushes_ok: self.pushes_ok.load(Ordering::Relaxed),
            pushes_full: self.pushes_full.load(Ordering::Relaxed),
            pops_ok: self.pops_ok.load(Ordering::Relaxed),
            pops_empty: self.pops_empty.load(Ordering::Relaxed),
            bytes_transferred: self.bytes_transferred.load(Ordering::Relaxed),
            pool_allocs: self.pool_allocs.load(Ordering::Relaxed),
            pool_frees: self.pool_frees.load(Ordering::Relaxed),
            backpressure_events: self.backpressure_events.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct StreamStatsSnapshot {
    pub pushes_ok: u64,
    pub pushes_full: u64,
    pub pops_ok: u64,
    pub pops_empty: u64,
    pub bytes_transferred: u64,
    pub pool_allocs: u64,
    pub pool_frees: u64,
    pub backpressure_events: u64,
}

// ---------------------------------------------------------------------------
// StreamChannel — structure principale
// ---------------------------------------------------------------------------

/// Canal de streaming zero-copy basé sur ZeroCopyRing + pool de buffers.
#[repr(C, align(64))]
pub struct StreamChannel {
    pub id: ChannelId,
    /// Anneau zero-copy pour les transferts
    ring: ZeroCopyRing,
    /// Pool de buffers physiques
    pool: StreamPool,
    /// Statistiques locales
    pub stats: StreamStats,
    /// Canal fermé
    closed: AtomicU32,
    /// Granule configurée
    granule: AtomicU32,
    _pad: [u8; 24],
}

// SAFETY: ZeroCopyRing est Sync, StreamPool est Sync
unsafe impl Sync for StreamChannel {}
unsafe impl Send for StreamChannel {}

impl StreamChannel {
    pub const fn new_uninit() -> Self {
        Self {
            id: ChannelId::DANGLING,
            ring: ZeroCopyRing::new(),
            pool: StreamPool::new(),
            stats: StreamStats::new(),
            closed: AtomicU32::new(0),
            granule: AtomicU32::new(StreamGranule::Page4K as u32),
            _pad: [0u8; 24],
        }
    }

    /// Initialise le canal avec une base physique et une granularité.
    pub fn init(&mut self, base_phys: u64, granule: StreamGranule, buffer_count: usize) {
        self.id = alloc_channel_id();
        self.pool.init(base_phys, granule, buffer_count);
        self.granule.store(granule as u32, Ordering::Relaxed);
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire) != 0
    }

    pub fn close(&self) {
        self.closed.store(1, Ordering::Release);
    }

    // -----------------------------------------------------------------------
    // Côté producteur
    // -----------------------------------------------------------------------

    /// Alloue un buffer depuis le pool pour le producteur.
    /// Retourne l'index du buffer alloué.
    pub fn alloc_buffer(&self) -> Result<usize, IpcError> {
        if self.is_closed() {
            return Err(IpcError::Closed);
        }
        let idx = self.pool.alloc_buffer().ok_or(IpcError::OutOfResources)?;
        self.stats.pool_allocs.fetch_add(1, Ordering::Relaxed);
        Ok(idx)
    }

    /// Envoie le buffer `buf_idx` (rempli avec `data_len` octets utiles).
    pub fn push(&self, buf_idx: usize, data_len: usize) -> Result<MessageId, IpcError> {
        if self.is_closed() {
            return Err(IpcError::Closed);
        }

        let zc_ref = self.pool.make_ref(buf_idx, data_len)
            .ok_or(IpcError::InvalidHandle)?;

        match self.ring.push(zc_ref) {
            Ok(()) => {
                let mid = alloc_message_id();
                self.stats.pushes_ok.fetch_add(1, Ordering::Relaxed);
                self.stats.bytes_transferred.fetch_add(data_len as u64, Ordering::Relaxed);
                IPC_STATS.record(StatEvent::MessageSent);
                Ok(mid)
            }
            Err(_) => {
                // Backpressure : ring plein, libérer le buffer
                self.pool.free_buffer(buf_idx);
                self.stats.pushes_full.fetch_add(1, Ordering::Relaxed);
                self.stats.backpressure_events.fetch_add(1, Ordering::Relaxed);
                Err(IpcError::QueueFull)
            }
        }
    }

    // -----------------------------------------------------------------------
    // Côté consommateur
    // -----------------------------------------------------------------------

    /// Reçoit le prochain ZeroCopyRef depuis le ring.
    /// Le consommateur doit appeler `release_buffer()` après traitement.
    pub fn pop(&self) -> Result<ZeroCopyRef, IpcError> {
        if self.is_closed() && self.ring.is_empty() {
            return Err(IpcError::Closed);
        }

        match self.ring.pop() {
            Ok(zc_ref) => {
                self.stats.pops_ok.fetch_add(1, Ordering::Relaxed);
                IPC_STATS.record(StatEvent::MessageReceived);
                Ok(zc_ref)
            }
            Err(_) => {
                self.stats.pops_empty.fetch_add(1, Ordering::Relaxed);
                Err(IpcError::WouldBlock)
            }
        }
    }

    /// Libère un buffer identifié par son pool_idx après consommation.
    pub fn release_buffer(&self, pool_idx: usize) -> bool {
        let freed = self.pool.free_buffer(pool_idx);
        if freed {
            self.stats.pool_frees.fetch_add(1, Ordering::Relaxed);
        }
        freed
    }

    // -----------------------------------------------------------------------
    // Utilitaires
    // -----------------------------------------------------------------------

    pub fn pool_available(&self) -> usize {
        self.pool.available_count()
    }

    pub fn ring_is_empty(&self) -> bool {
        self.ring.is_empty()
    }

    pub fn snapshot_stats(&self) -> StreamStatsSnapshot {
        self.stats.snapshot()
    }
}

// ---------------------------------------------------------------------------
// Table statique globale de canaux streaming
// ---------------------------------------------------------------------------

pub const STREAM_CHANNEL_TABLE_SIZE: usize = 64;

struct StreamChannelTable {
    slots: [MaybeUninit<StreamChannel>; STREAM_CHANNEL_TABLE_SIZE],
    used: [bool; STREAM_CHANNEL_TABLE_SIZE],
    count: usize,
}

// SAFETY: accès protégé par SpinLock
unsafe impl Send for StreamChannelTable {}

impl StreamChannelTable {
    const fn new() -> Self {
        // SAFETY: mem::zeroed() évite la limite mémoire du const-eval pour grands tableaux.
        unsafe { core::mem::zeroed() }
    }

    fn alloc(&mut self, base_phys: u64, granule: StreamGranule, buffer_count: usize) -> Option<usize> {
        for i in 0..STREAM_CHANNEL_TABLE_SIZE {
            if !self.used[i] {
                let mut chan = StreamChannel::new_uninit();
                chan.init(base_phys, granule, buffer_count);
                self.slots[i].write(chan);
                self.used[i] = true;
                self.count += 1;
                return Some(i);
            }
        }
        None
    }

    fn free(&mut self, idx: usize) -> bool {
        if idx < STREAM_CHANNEL_TABLE_SIZE && self.used[idx] {
            // SAFETY: used[idx] garantit que slots[idx] est initialisé; used → false empêche double-drop.
            unsafe { self.slots[idx].assume_init_drop() };
            self.used[idx] = false;
            self.count -= 1;
            true
        } else {
            false
        }
    }

    unsafe fn get(&self, idx: usize) -> Option<&StreamChannel> {
        if idx < STREAM_CHANNEL_TABLE_SIZE && self.used[idx] {
            Some(self.slots[idx].assume_init_ref())
        } else {
            None
        }
    }
}

static STREAM_CHANNEL_TABLE: SpinLock<StreamChannelTable> =
    SpinLock::new(StreamChannelTable::new());

// ---------------------------------------------------------------------------
// API publique de haut niveau
// ---------------------------------------------------------------------------

/// Crée un canal streaming avec `buffer_count` buffers de taille `granule`.
/// `base_phys` = adresse physique de base de la région mémoire pré-allouée.
pub fn stream_channel_create(
    base_phys: u64,
    granule: StreamGranule,
    buffer_count: usize,
) -> Result<usize, IpcError> {
    let mut tbl = STREAM_CHANNEL_TABLE.lock();
    tbl.alloc(base_phys, granule, buffer_count).ok_or(IpcError::OutOfResources)
}

/// Alloue un buffer producteur sur le canal `idx`.
pub fn stream_alloc_buffer(idx: usize) -> Result<usize, IpcError> {
    let tbl = STREAM_CHANNEL_TABLE.lock();
    let chan = unsafe { tbl.get(idx) }.ok_or(IpcError::InvalidHandle)?;
    let chan_ref: &'static StreamChannel = unsafe { &*(chan as *const StreamChannel) };
    drop(tbl);
    chan_ref.alloc_buffer()
}

/// Envoie le buffer `buf_idx` (avec `data_len` octets utiles) sur le canal `idx`.
pub fn stream_push(idx: usize, buf_idx: usize, data_len: usize) -> Result<MessageId, IpcError> {
    let tbl = STREAM_CHANNEL_TABLE.lock();
    let chan = unsafe { tbl.get(idx) }.ok_or(IpcError::InvalidHandle)?;
    let chan_ref: &'static StreamChannel = unsafe { &*(chan as *const StreamChannel) };
    drop(tbl);
    chan_ref.push(buf_idx, data_len)
}

/// Reçoit le prochain ZeroCopyRef du canal `idx`.
pub fn stream_pop(idx: usize) -> Result<ZeroCopyRef, IpcError> {
    let tbl = STREAM_CHANNEL_TABLE.lock();
    let chan = unsafe { tbl.get(idx) }.ok_or(IpcError::InvalidHandle)?;
    let chan_ref: &'static StreamChannel = unsafe { &*(chan as *const StreamChannel) };
    drop(tbl);
    chan_ref.pop()
}

/// Libère le buffer `pool_idx` après consommation sur le canal `idx`.
pub fn stream_release_buffer(idx: usize, pool_idx: usize) -> Result<(), IpcError> {
    let tbl = STREAM_CHANNEL_TABLE.lock();
    let chan = unsafe { tbl.get(idx) }.ok_or(IpcError::InvalidHandle)?;
    let chan_ref: &'static StreamChannel = unsafe { &*(chan as *const StreamChannel) };
    drop(tbl);
    if chan_ref.release_buffer(pool_idx) {
        Ok(())
    } else {
        Err(IpcError::InvalidHandle)
    }
}

/// Ferme et détruit le canal streaming `idx`.
pub fn stream_channel_destroy(idx: usize) -> Result<(), IpcError> {
    let tbl = STREAM_CHANNEL_TABLE.lock();
    if let Some(chan) = unsafe { tbl.get(idx) } {
        chan.close();
    }
    drop(tbl);
    let mut tbl = STREAM_CHANNEL_TABLE.lock();
    if !tbl.free(idx) {
        return Err(IpcError::InvalidHandle);
    }
    Ok(())
}
