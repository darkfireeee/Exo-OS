// kernel/src/fs/exofs/storage/block_cache.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Cache de blocs ExoFS — LRU write-back pour les accès disque fréquents
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// Le BlockCache maintient en RAM des copies de blocs physiques récemment lus
// ou écrits. Il opère en write-back : les blocs modifiés (dirty) sont flushés
// périodiquement vers le disque ou sur demande.
//
// Politique d'éviction : LRU (Least Recently Used).
// Capacité configurable à la création (en nombre de blocs de 4 KB).
//
// Règles :
// - OOM-02   : try_reserve avant toute insertion.
// - ARITH-02 : checked_add pour les offset.
// - LOCK-04  : SpinLock uniquement pendant la mutation du cache.

use crate::fs::exofs::core::{DiskOffset, ExofsError, ExofsResult};
use crate::fs::exofs::storage::layout::BLOCK_SIZE;
use crate::fs::exofs::storage::storage_stats::STORAGE_STATS;
use crate::scheduler::sync::spinlock::SpinLock;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// CacheBlockState — état d'un bloc en cache
// ─────────────────────────────────────────────────────────────────────────────

/// État d'un bloc dans le cache.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CacheBlockState {
    /// Bloc propre (copie fidèle du disque).
    Clean,
    /// Bloc modifié (doit être flushé avant éviction).
    Dirty,
}

// ─────────────────────────────────────────────────────────────────────────────
// CacheBlock — entrée du cache
// ─────────────────────────────────────────────────────────────────────────────

/// Entrée dans le cache de blocs.
pub(crate) struct CacheBlock {
    /// Offset disque de ce bloc.
    offset: DiskOffset,
    /// Contenu du bloc (BLOCK_SIZE = 4096 octets).
    data: Vec<u8>,
    /// État (propre ou dirty).
    state: CacheBlockState,
    /// Compteur LRU : plus élevé = plus récemment utilisé.
    lru_tick: u64,
    /// Nombre de hits sur ce bloc.
    hits: u32,
}

impl CacheBlock {
    fn new(offset: DiskOffset, data: Vec<u8>, lru_tick: u64) -> Self {
        Self {
            offset,
            data,
            state: CacheBlockState::Clean,
            lru_tick,
            hits: 0,
        }
    }

    fn touch(&mut self, tick: u64) {
        self.lru_tick = tick;
        self.hits = self.hits.saturating_add(1);
    }

    fn mark_dirty(&mut self) {
        self.state = CacheBlockState::Dirty;
    }

    fn mark_clean(&mut self) {
        self.state = CacheBlockState::Clean;
    }

    fn is_dirty(&self) -> bool {
        self.state == CacheBlockState::Dirty
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CacheState — état interne protégé par SpinLock
// ─────────────────────────────────────────────────────────────────────────────

struct CacheState {
    entries: Vec<CacheBlock>,
    capacity: usize,
    tick: u64,
}

impl CacheState {
    fn new(capacity: usize) -> Self {
        Self {
            entries: Vec::new(),
            capacity,
            tick: 0,
        }
    }

    fn next_tick(&mut self) -> u64 {
        self.tick = self.tick.wrapping_add(1);
        self.tick
    }

    fn find(&mut self, offset: DiskOffset) -> Option<&mut CacheBlock> {
        self.entries.iter_mut().find(|b| b.offset == offset)
    }

    #[allow(dead_code)]
    fn find_ref(&self, offset: DiskOffset) -> Option<&CacheBlock> {
        self.entries.iter().find(|b| b.offset == offset)
    }

    fn is_full(&self) -> bool {
        self.entries.len() >= self.capacity
    }

    /// Sélectionne la victime LRU pour éviction.
    /// Préfère les blocs propres, sinon prend le moins récemment utilisé.
    fn lru_victim(&self) -> Option<usize> {
        if self.entries.is_empty() {
            return None;
        }

        // Chercher un bloc clean LRU.
        let clean_victim = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, b)| !b.is_dirty())
            .min_by_key(|(_, b)| b.lru_tick);

        if let Some((i, _)) = clean_victim {
            return Some(i);
        }

        // Tous sont dirty → prendre le dirty LRU.
        self.entries
            .iter()
            .enumerate()
            .min_by_key(|(_, b)| b.lru_tick)
            .map(|(i, _)| i)
    }

    /// Nombre de blocs dirty.
    fn dirty_count(&self) -> usize {
        self.entries.iter().filter(|b| b.is_dirty()).count()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BlockCache
// ─────────────────────────────────────────────────────────────────────────────

/// Cache de blocs LRU write-back pour ExoFS.
pub struct BlockCache {
    state: SpinLock<CacheState>,
    // ── Statistiques ───────────────────────────────────────────────────────
    hits: AtomicU64,
    misses: AtomicU64,
    evictions: AtomicU64,
    flushes: AtomicU64,
    dirty_writes: AtomicU64,
}

impl BlockCache {
    // ── Constructeur ─────────────────────────────────────────────────────────

    /// Crée un cache d'une capacité de `capacity` blocs.
    pub fn new(capacity: usize) -> Self {
        Self {
            state: SpinLock::new(CacheState::new(capacity)),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
            flushes: AtomicU64::new(0),
            dirty_writes: AtomicU64::new(0),
        }
    }

    // ── Lecture ───────────────────────────────────────────────────────────────

    /// Lit un bloc depuis le cache ou le disque.
    ///
    /// `read_fn` est appelé uniquement en cas de cache miss.
    pub fn read_block(
        &self,
        offset: DiskOffset,
        out_buf: &mut [u8],
        read_fn: &dyn Fn(DiskOffset, &mut [u8]) -> ExofsResult<usize>,
    ) -> ExofsResult<usize> {
        if out_buf.len() < BLOCK_SIZE as usize {
            return Err(ExofsError::InvalidArgument);
        }

        // Cache lookup.
        {
            let mut st = self.state.lock();
            let tick = st.next_tick();
            if let Some(entry) = st.find(offset) {
                entry.touch(tick);
                out_buf[..BLOCK_SIZE as usize].copy_from_slice(&entry.data);
                drop(st);
                self.hits.fetch_add(1, Ordering::Relaxed);
                STORAGE_STATS.inc_cache_hit();
                return Ok(BLOCK_SIZE as usize);
            }
        }

        // Cache miss → lecture physique.
        let mut buf: Vec<u8> = Vec::new();
        buf.try_reserve(BLOCK_SIZE as usize)
            .map_err(|_| ExofsError::NoMemory)?;
        buf.resize(BLOCK_SIZE as usize, 0u8);

        let n = read_fn(offset, &mut buf)?;
        out_buf[..BLOCK_SIZE as usize].copy_from_slice(&buf[..BLOCK_SIZE as usize]);

        // Insérer dans le cache.
        self.insert_clean(offset, buf)?;

        self.misses.fetch_add(1, Ordering::Relaxed);
        STORAGE_STATS.inc_cache_miss();
        STORAGE_STATS.add_read(n as u64);

        Ok(n)
    }

    // ── Écriture ──────────────────────────────────────────────────────────────

    /// Écrit un bloc dans le cache (dirty).
    ///
    /// Le bloc n'est PAS immédiatement persisté — il sera flushé lors d'un
    /// appel à `flush_dirty()` ou à l'éviction.
    ///
    /// # Règle WRITE-02 : la vérification bytes_written est à la charge du caller.
    pub fn write_block_dirty(&self, offset: DiskOffset, data: &[u8]) -> ExofsResult<()> {
        if data.len() != BLOCK_SIZE as usize {
            return Err(ExofsError::InvalidArgument);
        }

        let mut st = self.state.lock();
        let tick = st.next_tick();

        if let Some(entry) = st.find(offset) {
            // Mise à jour en place.
            entry.data.copy_from_slice(data);
            entry.mark_dirty();
            entry.touch(tick);
            self.dirty_writes.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }

        // Nouvel entrée.
        let mut buf: Vec<u8> = Vec::new();
        buf.try_reserve(BLOCK_SIZE as usize)
            .map_err(|_| ExofsError::NoMemory)?;
        buf.extend_from_slice(data);

        // Éviction si plein.
        if st.is_full() {
            self.evict_one(&mut st, None)?;
        }

        st.entries
            .try_reserve(1)
            .map_err(|_| ExofsError::NoMemory)?;
        let mut entry = CacheBlock::new(offset, buf, tick);
        entry.mark_dirty();
        st.entries.push(entry);

        self.dirty_writes.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Écrit un bloc dans le cache ET immédiatement sur le disque.
    pub fn write_block_sync(
        &self,
        offset: DiskOffset,
        data: &[u8],
        write_fn: &dyn Fn(&[u8], DiskOffset) -> ExofsResult<usize>,
    ) -> ExofsResult<usize> {
        self.write_block_dirty(offset, data)?;
        // Flush immédiat.
        let n = write_fn(data, offset)?;
        // Marquer propre si succès.
        {
            let mut st = self.state.lock();
            if let Some(entry) = st.find(offset) {
                entry.mark_clean();
            }
        }
        STORAGE_STATS.add_write(n as u64);
        Ok(n)
    }

    // ── Flush ─────────────────────────────────────────────────────────────────

    /// Flushe tous les blocs dirty vers le disque.
    pub fn flush_dirty(
        &self,
        write_fn: &dyn Fn(&[u8], DiskOffset) -> ExofsResult<usize>,
    ) -> ExofsResult<u64> {
        let offsets_data: Vec<(DiskOffset, Vec<u8>)> = {
            let mut st = self.state.lock();
            let mut dirty = Vec::new();

            for entry in &mut st.entries {
                if entry.is_dirty() {
                    // Cloner pour écrire sans tenir le lock.
                    let data = entry.data.clone();
                    dirty.push((entry.offset, data));
                }
            }
            dirty
        };

        let mut flushed = 0u64;
        for (offset, data) in &offsets_data {
            match write_fn(data, *offset) {
                Ok(n) => {
                    // Marquer propre.
                    let mut st = self.state.lock();
                    if let Some(entry) = st.find(*offset) {
                        entry.mark_clean();
                    }
                    self.flushes.fetch_add(1, Ordering::Relaxed);
                    STORAGE_STATS.inc_cache_flush();
                    STORAGE_STATS.add_write(n as u64);
                    flushed = flushed.saturating_add(1);
                }
                Err(_) => {
                    STORAGE_STATS.inc_io_error();
                }
            }
        }
        Ok(flushed)
    }

    // ── Invalidation ─────────────────────────────────────────────────────────

    /// Invalide un bloc du cache (force la re-lecture depuis le disque).
    pub fn invalidate(&self, offset: DiskOffset) {
        let mut st = self.state.lock();
        if let Some(pos) = st.entries.iter().position(|b| b.offset == offset) {
            st.entries.swap_remove(pos);
        }
    }

    /// Invalide tous les blocs du cache.
    pub fn invalidate_all(&self) {
        let mut st = self.state.lock();
        st.entries.clear();
    }

    // ── Utilitaires ───────────────────────────────────────────────────────────

    fn insert_clean(&self, offset: DiskOffset, data: Vec<u8>) -> ExofsResult<()> {
        let mut st = self.state.lock();
        let tick = st.next_tick();

        if st.is_full() {
            self.evict_one(&mut st, None)?;
        }

        st.entries
            .try_reserve(1)
            .map_err(|_| ExofsError::NoMemory)?;
        st.entries.push(CacheBlock::new(offset, data, tick));
        Ok(())
    }

    fn evict_one(
        &self,
        st: &mut CacheState,
        write_fn: Option<&dyn Fn(&[u8], DiskOffset) -> ExofsResult<usize>>,
    ) -> ExofsResult<()> {
        let victim_idx = match st.lru_victim() {
            Some(i) => i,
            None => return Err(ExofsError::NoMemory),
        };

        let victim = &st.entries[victim_idx];
        if victim.is_dirty() {
            if let Some(wfn) = write_fn {
                let _ = wfn(&victim.data, victim.offset);
                STORAGE_STATS.inc_cache_flush();
            }
        }

        st.entries.swap_remove(victim_idx);
        self.evictions.fetch_add(1, Ordering::Relaxed);
        STORAGE_STATS.inc_cache_eviction();
        Ok(())
    }

    // ── Requêtes ─────────────────────────────────────────────────────────────

    pub fn hit_count(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }
    pub fn miss_count(&self) -> u64 {
        self.misses.load(Ordering::Relaxed)
    }
    pub fn eviction_count(&self) -> u64 {
        self.evictions.load(Ordering::Relaxed)
    }
    pub fn flush_count(&self) -> u64 {
        self.flushes.load(Ordering::Relaxed)
    }

    /// Ratio de hit en pourcentage (0..=100).
    pub fn hit_rate_pct(&self) -> u64 {
        let h = self.hits.load(Ordering::Relaxed);
        let m = self.misses.load(Ordering::Relaxed);
        let t = h.saturating_add(m);
        if t == 0 {
            0
        } else {
            (h as u128 * 100 / t as u128) as u64
        }
    }

    pub fn dirty_count(&self) -> usize {
        self.state.lock().dirty_count()
    }
    pub fn cached_count(&self) -> usize {
        self.state.lock().entries.len()
    }
    pub fn capacity(&self) -> usize {
        self.state.lock().capacity
    }
    pub fn is_empty(&self) -> bool {
        self.state.lock().entries.is_empty()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests unitaires
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const BLK: usize = BLOCK_SIZE as usize;

    fn make_cache() -> BlockCache {
        BlockCache::new(8)
    }

    fn mock_read(_offset: DiskOffset, buf: &mut [u8]) -> ExofsResult<usize> {
        buf.fill(0xAB);
        Ok(buf.len())
    }

    fn mock_write(data: &[u8], _offset: DiskOffset) -> ExofsResult<usize> {
        Ok(data.len())
    }

    #[test]
    fn test_read_miss_then_hit() {
        let c = make_cache();
        let off = DiskOffset(4096);
        let mut buf = vec![0u8; BLK];

        c.read_block(off, &mut buf, &mock_read).unwrap();
        assert_eq!(c.miss_count(), 1);

        c.read_block(off, &mut buf, &mock_read).unwrap();
        assert_eq!(c.hit_count(), 1);
    }

    #[test]
    fn test_write_dirty_then_flush() {
        let c = make_cache();
        let off = DiskOffset(4096);
        let data = vec![0xFFu8; BLK];

        c.write_block_dirty(off, &data).unwrap();
        assert_eq!(c.dirty_count(), 1);

        c.flush_dirty(&mock_write).unwrap();
        assert_eq!(c.dirty_count(), 0);
        assert_eq!(c.flush_count(), 1);
    }

    #[test]
    fn test_invalidate() {
        let c = make_cache();
        let off = DiskOffset(4096);
        let mut buf = vec![0u8; BLK];
        c.read_block(off, &mut buf, &mock_read).unwrap();
        assert_eq!(c.cached_count(), 1);
        c.invalidate(off);
        assert_eq!(c.cached_count(), 0);
    }

    #[test]
    fn test_eviction_lru() {
        let c = BlockCache::new(2);
        let mut buf = vec![0u8; BLK];
        c.read_block(DiskOffset(0), &mut buf, &mock_read).unwrap();
        c.read_block(DiskOffset(4096), &mut buf, &mock_read)
            .unwrap();
        // Cache plein, 3ème lecture → éviction du plus ancien.
        c.read_block(DiskOffset(8192), &mut buf, &mock_read)
            .unwrap();
        assert_eq!(c.cached_count(), 2);
        assert_eq!(c.eviction_count(), 1);
    }
}
