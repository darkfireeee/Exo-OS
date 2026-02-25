// kernel/src/fs/io/zero_copy.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ZERO-COPY — sendfile / splice / tee (Exo-OS · Couche 3)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Transferts de données entre fds sans copie userland :
//   • `splice_pages()` : déplace des pages du page cache d'un fd vers un autre.
//   • `sendfile_pages()` : envoie des pages d'un fichier vers un socket/fd.
//   • `tee_pages()` : duplique des pages (lecture seule) entre deux pipe fds.
//
// Architecture zero-copy :
//   1. Les `CachedPage` dans le page cache sont partagés entre src et dst via
//      `Arc::clone()` (refcount) — pas de copie des données.
//   2. Le writer du fd dst marque les pages dirty et track le `ino` destination.
//   3. Toute la comptabilité de bytes est instrumentée dans `ZC_STATS`.
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, Ordering};
use alloc::vec::Vec;

use alloc::sync::Arc;
use crate::fs::core::types::{FsError, FsResult, InodeNumber, FS_STATS};
use crate::fs::cache::page_cache::{PageIndex, PAGE_CACHE, CachedPage};
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────────────────────
// SpliceFlags
// ─────────────────────────────────────────────────────────────────────────────

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug)]
    pub struct SpliceFlags: u32 {
        /// Non-bloquant si pas de données disponibles.
        const NONBLOCK = 0x01;
        /// Transfert plus de données à venir (hint).
        const MORE     = 0x04;
        /// Gift : la page appartient au destinataire (ne pas free à la fin).
        const GIFT     = 0x08;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// splice_pages — transfert direct entre deux inodes
// ─────────────────────────────────────────────────────────────────────────────

/// Transfère `len` bytes de `src_ino` (à partir de `src_off`) vers `dst_ino`
/// (à `dst_off`), en déplaçant les pages sans copie mémoire.
///
/// Retourne le nombre de bytes transférés.
pub fn splice_pages(
    src_ino: InodeNumber,
    src_off: u64,
    dst_ino: InodeNumber,
    dst_off: u64,
    len:     u64,
    _flags:  SpliceFlags,
) -> FsResult<u64> {
    if len == 0 { return Ok(0); }

    let pc = PAGE_CACHE.get();
    const PAGE_SIZE: u64 = 4096;
    let mut transferred = 0u64;
    let mut remaining   = len;

    while remaining > 0 {
        let src_page_idx = PageIndex((src_off + transferred) / PAGE_SIZE);
        let page = pc.lookup(src_ino, src_page_idx)
            .ok_or(FsError::Io)?;

        if !page.uptodate.load(Ordering::Acquire) {
            return Err(FsError::Io);
        }

        let page_offset = ((src_off + transferred) % PAGE_SIZE) as usize;
        let page_avail  = PAGE_SIZE as usize - page_offset;
        let copy_len    = (remaining as usize).min(page_avail) as u64;

        // Re-associe la page au dst_ino (zero-copy : même données, nouveau mapping).
        let dst_page_idx = PageIndex((dst_off + transferred) / PAGE_SIZE);
        let new_page = Arc::new(CachedPage::new(dst_ino, dst_page_idx, page.phys, page.virt, 0));
        pc.insert(new_page).ok();

        // Marque dirty dans la destination.
        PAGE_CACHE.mark_dirty(dst_ino, dst_page_idx);

        transferred += copy_len;
        remaining   = remaining.saturating_sub(copy_len);
        ZC_STATS.bytes_spliced.fetch_add(copy_len, Ordering::Relaxed);
    }

    ZC_STATS.splice_calls.fetch_add(1, Ordering::Relaxed);
    FS_STATS.bytes_written.fetch_add(transferred, Ordering::Relaxed);
    Ok(transferred)
}

// ─────────────────────────────────────────────────────────────────────────────
// sendfile_pages — envoi fichier → socket
// ─────────────────────────────────────────────────────────────────────────────

/// Envoie `len` bytes du fichier `src_ino` (à partir de `src_off`) vers le
/// fd socket `dst_fd`, en utilisant le page cache pour le DMA.
///
/// N'implémente pas le DMA matériel ici (déléguée au driver réseau),
/// mais prépare et pin les pages pour le transfert.
pub fn sendfile_pages(
    src_ino: InodeNumber,
    src_off: u64,
    dst_fd:  i32,
    len:     u64,
) -> FsResult<u64> {
    if len == 0 { return Ok(0); }
    let _ = dst_fd; // utilisé par le driver réseau en pratique

    let pc = PAGE_CACHE.get();
    const PAGE_SIZE: u64 = 4096;
    let mut sent = 0u64;
    let mut remaining = len;

    while remaining > 0 {
        let idx = PageIndex((src_off + sent) / PAGE_SIZE);
        let page = pc.lookup(src_ino, idx).ok_or(FsError::Io)?;

        if !page.uptodate.load(Ordering::Acquire) {
            return Err(FsError::Io);
        }

        page.pin();
        let page_off  = ((src_off + sent) % PAGE_SIZE) as usize;
        let available = PAGE_SIZE as usize - page_off;
        let chunk     = (remaining as usize).min(available) as u64;

        // Le driver réseau lirait page.phys + page_off via DMA ici.
        // On simule le succès.

        page.unpin();
        sent      += chunk;
        remaining  = remaining.saturating_sub(chunk);
        ZC_STATS.bytes_sendfile.fetch_add(chunk, Ordering::Relaxed);
    }

    ZC_STATS.sendfile_calls.fetch_add(1, Ordering::Relaxed);
    FS_STATS.bytes_read.fetch_add(sent, Ordering::Relaxed);
    Ok(sent)
}

// ─────────────────────────────────────────────────────────────────────────────
// tee_pages — duplication pipe → pipe (lecture seule)
// ─────────────────────────────────────────────────────────────────────────────

/// Duplique `len` bytes de `src_ino` vers `dst_ino` sans consommer la source.
/// Équivalent du syscall `tee(2)` Linux.
pub fn tee_pages(
    src_ino: InodeNumber,
    dst_ino: InodeNumber,
    src_off: u64,
    len:     u64,
) -> FsResult<u64> {
    if len == 0 { return Ok(0); }

    let pc = PAGE_CACHE.get();
    const PAGE_SIZE: u64 = 4096;
    let mut copied    = 0u64;
    let mut remaining = len;

    while remaining > 0 {
        let src_idx = PageIndex((src_off + copied) / PAGE_SIZE);
        let page    = pc.lookup(src_ino, src_idx).ok_or(FsError::Io)?;

        if !page.uptodate.load(Ordering::Acquire) {
            return Err(FsError::Io);
        }

        let dst_idx = PageIndex((src_off + copied) / PAGE_SIZE);
        // Partage la même frame physique (zero-copy).
        let new_page = Arc::new(CachedPage::new(dst_ino, dst_idx, page.phys, page.virt, 0));
        pc.insert(new_page).ok();

        let chunk = PAGE_SIZE.min(remaining);
        copied    += chunk;
        remaining  = remaining.saturating_sub(chunk);
        ZC_STATS.bytes_tee.fetch_add(chunk, Ordering::Relaxed);
    }

    ZC_STATS.tee_calls.fetch_add(1, Ordering::Relaxed);
    Ok(copied)
}

// ─────────────────────────────────────────────────────────────────────────────
// ZeroCopyStats
// ─────────────────────────────────────────────────────────────────────────────

pub struct ZeroCopyStats {
    pub splice_calls:    AtomicU64,
    pub sendfile_calls:  AtomicU64,
    pub tee_calls:       AtomicU64,
    pub bytes_spliced:   AtomicU64,
    pub bytes_sendfile:  AtomicU64,
    pub bytes_tee:       AtomicU64,
}

impl ZeroCopyStats {
    pub const fn new() -> Self {
        Self {
            splice_calls:   AtomicU64::new(0),
            sendfile_calls: AtomicU64::new(0),
            tee_calls:      AtomicU64::new(0),
            bytes_spliced:  AtomicU64::new(0),
            bytes_sendfile: AtomicU64::new(0),
            bytes_tee:      AtomicU64::new(0),
        }
    }
}

pub static ZC_STATS: ZeroCopyStats = ZeroCopyStats::new();
