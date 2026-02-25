// kernel/src/fs/io/direct_io.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// DIRECT I/O — O_DIRECT bypass du cache (Exo-OS · Couche 3)
// ═══════════════════════════════════════════════════════════════════════════════
//
// O_DIRECT effectue des transferts de données directement entre le buffer
// userland/kernel et le block device, en contournant le page cache.
//
// Utilisation typique :
//   - Bases de données (gèrent leur propre cache)
//   - Backup/restore (pas besoin de polluer le cache)
//   - Tests de performance I/O bruts
//
// Contraintes POSIX et Linux :
//   • Offset, len et adresse buf doivent être alignés sur 512 bytes minimum.
//   • En pratique, l'alignement logique dépend de la taille de secteur du device.
//   • On invalide les pages cache correspondantes après un DIO write.
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, Ordering};

use crate::fs::core::types::{FsError, FsResult, InodeNumber, FS_STATS};
use crate::fs::cache::page_cache::{PageIndex, PAGE_CACHE};
use crate::fs::block::bio::{Bio, BioOp, BioFlags};
use crate::fs::block::queue::submit_bio;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes d'alignement
// ─────────────────────────────────────────────────────────────────────────────

/// Alignement minimum pour O_DIRECT (taille de secteur).
pub const DIO_ALIGN: u64 = 512;
/// Alignement logique moderne (taille de bloc = 4 KiB).
pub const DIO_BLOCK_ALIGN: u64 = 4096;

// ─────────────────────────────────────────────────────────────────────────────
// Vérification d'alignement
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie que les paramètres d'un DIO sont alignés.
#[inline(always)]
pub fn check_dio_alignment(buf: u64, offset: u64, len: u64) -> FsResult<()> {
    if buf % DIO_BLOCK_ALIGN != 0 {
        return Err(FsError::InvalArg); // EINVAL — buf non aligné
    }
    if offset % DIO_ALIGN != 0 {
        return Err(FsError::InvalArg); // EINVAL — offset non aligné
    }
    if len == 0 || len % DIO_ALIGN != 0 {
        return Err(FsError::InvalArg); // EINVAL — len non alignée
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// direct_read — lecture directe sans cache
// ─────────────────────────────────────────────────────────────────────────────

/// Lecture directe depuis le block device dans `buf`, en bypassant le page cache.
///
/// `buf` = adresse virtuelle kernel d'un buffer DMA-safe aligné sur `DIO_BLOCK_ALIGN`.
pub fn direct_read(
    ino:    InodeNumber,
    dev:    u64,
    offset: u64,
    buf:    u64,
    len:    u64,
) -> FsResult<u64> {
    check_dio_alignment(buf, offset, len)?;

    // Invalide les pages cache pour la plage concernée (cohérence O_DIRECT).
    let pc = PAGE_CACHE.get();
    let start_page = PageIndex(offset / 4096);
    let end_page   = PageIndex((offset + len + 4095) / 4096);
    for idx in start_page.0..end_page.0 {
        pc.remove(ino, PageIndex(idx));
    }

    // Soumet une BIO lecture au block device.
    let bio = Bio::new(BioOp::Read, dev, offset / 512, buf, len as u32, BioFlags::DIRECT);
    submit_bio(bio)?;

    DIO_STATS.reads.fetch_add(1, Ordering::Relaxed);
    DIO_STATS.bytes_read.fetch_add(len, Ordering::Relaxed);
    FS_STATS.bytes_read.fetch_add(len, Ordering::Relaxed);
    Ok(len)
}

// ─────────────────────────────────────────────────────────────────────────────
// direct_write — écriture directe sans cache
// ─────────────────────────────────────────────────────────────────────────────

/// Écriture directe dans le block device depuis `buf`, en bypassant le page cache.
///
/// Invalide les pages cache correspondantes après l'écriture pour maintenir
/// la cohérence lors des lectures suivantes.
pub fn direct_write(
    ino:    InodeNumber,
    dev:    u64,
    offset: u64,
    buf:    u64,
    len:    u64,
) -> FsResult<u64> {
    check_dio_alignment(buf, offset, len)?;

    // Invalide les dirty pages en cache pour la plage.
    let pc = PAGE_CACHE.get();
    let start_page = PageIndex(offset / 4096);
    let end_page   = PageIndex((offset + len + 4095) / 4096);
    for idx in start_page.0..end_page.0 {
        pc.remove(ino, PageIndex(idx));
    }

    // Soumet une BIO écriture.
    let bio = Bio::new(BioOp::Write, dev, offset / 512, buf, len as u32, BioFlags::DIRECT);
    submit_bio(bio)?;

    DIO_STATS.writes.fetch_add(1, Ordering::Relaxed);
    DIO_STATS.bytes_written.fetch_add(len, Ordering::Relaxed);
    FS_STATS.bytes_written.fetch_add(len, Ordering::Relaxed);
    Ok(len)
}

// ─────────────────────────────────────────────────────────────────────────────
// DioBatch — batch de requêtes DIO
// ─────────────────────────────────────────────────────────────────────────────

/// Vecteur de requêtes DIO pour un scatter-gather (preadv/pwritev O_DIRECT).
#[derive(Clone, Debug)]
pub struct DioVec {
    pub buf:    u64,
    pub len:    u64,
    pub offset: u64,
}

/// Lecture vectorisée direct I/O (scatter read).
pub fn direct_readv(
    ino:  InodeNumber,
    dev:  u64,
    vecs: &[DioVec],
) -> FsResult<u64> {
    let mut total = 0u64;
    for v in vecs {
        total += direct_read(ino, dev, v.offset, v.buf, v.len)?;
    }
    Ok(total)
}

/// Écriture vectorisée direct I/O (gather write).
pub fn direct_writev(
    ino:  InodeNumber,
    dev:  u64,
    vecs: &[DioVec],
) -> FsResult<u64> {
    let mut total = 0u64;
    for v in vecs {
        total += direct_write(ino, dev, v.offset, v.buf, v.len)?;
    }
    Ok(total)
}

// ─────────────────────────────────────────────────────────────────────────────
// DioStats
// ─────────────────────────────────────────────────────────────────────────────

pub struct DioStats {
    pub reads:        AtomicU64,
    pub writes:       AtomicU64,
    pub bytes_read:   AtomicU64,
    pub bytes_written:AtomicU64,
    pub align_errors: AtomicU64,
}

impl DioStats {
    pub const fn new() -> Self {
        Self {
            reads:         AtomicU64::new(0),
            writes:        AtomicU64::new(0),
            bytes_read:    AtomicU64::new(0),
            bytes_written: AtomicU64::new(0),
            align_errors:  AtomicU64::new(0),
        }
    }
}

pub static DIO_STATS: DioStats = DioStats::new();
