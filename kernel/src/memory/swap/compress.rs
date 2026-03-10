// kernel/src/memory/swap/compress.rs
//
// Compression/décompression de pages swap — implémentation zswap light.
//
// Fournit un cache en RAM de pages compressées (zswap pool) pour retarder
// les I/O vers le périphérique de swap ou les éviter entièrement.
//
// Architecture :
//   • `CompressBackend` trait — interface de compression/décompression.
//   • `Lz4Lite` — algorithme LZ77 simplifié sans dépendance externe.
//   • `ZswapSlot` — entrée du pool (données compressées + métadonnées).
//   • `ZswapPool` — tableau statique de slots, protégé par Mutex.
//   • `ZSWAP_POOL` — instance globale unique.
//
// Le format de compression LZ77-lite :
//   Flux d'tokens :
//     [0b0_LLLLLLL] literal  : L+1 octets littéraux suivent (1..128).
//     [0b1_LLLOOO | OO_OOOOOO | OOOOOOOO] match : offset 13 bits, len 3+L octets (3..10).
//   Si la sortie dépasse ZSWAP_SLOT_SIZE → échec (page non compressible).
//
// Taille max d'un slot compressé : ZSWAP_SLOT_SIZE = 3072 octets (75 % de 4 KiB).
// Au-delà, la page est rejetée vers le swap device classique.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

use lz4_flex::block::{compress_into, decompress_into};

use crate::memory::core::constants::PAGE_SIZE;

// ─────────────────────────────────────────────────────────────────────────────
// CONSTANTES
// ─────────────────────────────────────────────────────────────────────────────

/// Taille maximale du buffer compressé accepté (75 % de PAGE_SIZE).
pub const ZSWAP_SLOT_SIZE:  usize = 3072;

/// Nombre de slots dans le pool zswap.
pub const MAX_ZSWAP_SLOTS:  usize = 4096;

/// Taille du hash de validation (CRC32-like XOR fold).
const CHECKSUM_SIZE: usize = 4;

// ─────────────────────────────────────────────────────────────────────────────
// TRAIT COMPRESSBACKEND
// ─────────────────────────────────────────────────────────────────────────────

/// Interface de compression/décompression.
pub trait CompressBackend {
    /// Compresse `src` (PAGE_SIZE octets) dans `dst`.
    /// Retourne `Some(len)` si réussi, `None` si la page n'est pas compressible.
    fn compress(src: &[u8], dst: &mut [u8]) -> Option<usize>;

    /// Décompresse `src` (données compressées) dans `dst` (PAGE_SIZE octets).
    /// Retourne `true` si réussi.
    fn decompress(src: &[u8], dst: &mut [u8]) -> bool;
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4LITE — via crate lz4_flex (block mode, no_std)
// ─────────────────────────────────────────────────────────────────────────────
//
// RÈGLE CRYPTO-CRATES : JAMAIS d'implémentation from scratch.
// Crate : lz4_flex v0.11.x, default-features = false
//   - LZ4 block format pure Rust, no_std + alloc
//   - Conforme au format LZ4 block officiel

pub struct Lz4Lite;

impl CompressBackend for Lz4Lite {
    /// Compresse `src` (PAGE_SIZE octets) dans `dst` via lz4_flex.
    /// Retourne `Some(len)` si réussi, `None` si la page n'est pas compressible.
    fn compress(src: &[u8], dst: &mut [u8]) -> Option<usize> {
        debug_assert_eq!(src.len(), PAGE_SIZE);
        compress_into(src, dst).ok()
    }

    /// Décompresse `src` vers `dst` (PAGE_SIZE octets) via lz4_flex.
    /// Retourne `true` si la décompression a rempli exactement PAGE_SIZE.
    fn decompress(src: &[u8], dst: &mut [u8]) -> bool {
        debug_assert_eq!(dst.len(), PAGE_SIZE);
        match decompress_into(src, dst) {
            Ok(n) => n == PAGE_SIZE,
            Err(_) => false,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SLOTS ZSWAP
// ─────────────────────────────────────────────────────────────────────────────

/// Un slot du pool zswap. Taille fixe = ZSWAP_SLOT_SIZE + métadonnées.
pub struct ZswapSlot {
    /// Données compressées.
    data:           [u8; ZSWAP_SLOT_SIZE],
    /// Longueur réelle des données compressées (0 = libre).
    compressed_len: u16,
    /// PFN d'origine de la page (identifiant).
    pub orig_pfn:   u64,
    /// Ce slot est-il occupé ?
    pub valid:      AtomicBool,
    /// XOR-fold CRC16 des données décompressées pour validation.
    checksum:       u16,
}

impl ZswapSlot {
    const fn new() -> Self {
        ZswapSlot {
            data:           [0u8; ZSWAP_SLOT_SIZE],
            compressed_len: 0,
            orig_pfn:       0,
            valid:          AtomicBool::new(false),
            checksum:       0,
        }
    }
}

/// Calcule un CRC16 XOR-fold simple sur `data`.
fn xor_checksum(data: &[u8]) -> u16 {
    let mut acc = 0xFFFFu16;
    for &b in data {
        acc = acc ^ (b as u16);
        acc = acc.rotate_left(5);
    }
    acc
}

// ─────────────────────────────────────────────────────────────────────────────
// ZSWAP POOL
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat d'une tentative de compression/stockage.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ZswapStoreResult {
    /// Stocké avec succès, index du slot retourné.
    Stored(usize),
    /// Page non compressible (ratio > 75 %).
    NotCompressible,
    /// Pool plein.
    PoolFull,
}

/// Résultat d'une récupération depuis le pool zswap.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ZswapLoadResult {
    /// Décompressé avec succès dans le buffer fourni.
    Ok,
    /// Slot introuvable pour ce PFN.
    NotFound,
    /// Données corrompues (checksum mismatch).
    Corrupt,
    /// Erreur de décompression.
    DecompressError,
}

struct ZswapPoolInner {
    slots: &'static mut [ZswapSlot],
    count: usize,
}

/// Pool de pages compressées en RAM.
pub struct ZswapPool {
    inner: Mutex<()>,
    /// Statistiques.
    pub stored:       AtomicU64,
    pub evicted:      AtomicU64,
    pub decomp_ok:    AtomicU64,
    pub decomp_fail:  AtomicU64,
    pub compress_fail: AtomicU64,
}

// Tableau statique global des slots.
static mut ZSWAP_SLOTS: [ZswapSlot; MAX_ZSWAP_SLOTS] = {
    const S: ZswapSlot = ZswapSlot::new();
    [S; MAX_ZSWAP_SLOTS]
};

impl ZswapPool {
    const fn new() -> Self {
        ZswapPool {
            inner:         Mutex::new(()),
            stored:        AtomicU64::new(0),
            evicted:       AtomicU64::new(0),
            decomp_ok:     AtomicU64::new(0),
            decomp_fail:   AtomicU64::new(0),
            compress_fail: AtomicU64::new(0),
        }
    }

    /// Compresse et stocke la page identifiée par `pfn`.
    /// `page_data` doit faire exactement PAGE_SIZE octets.
    pub fn store(&self, pfn: u64, page_data: &[u8]) -> ZswapStoreResult {
        debug_assert_eq!(page_data.len(), PAGE_SIZE);

        // Compresser dans un buffer temporaire sur la pile.
        let mut tmp = [0u8; ZSWAP_SLOT_SIZE];
        let compressed_len = match Lz4Lite::compress(page_data, &mut tmp) {
            Some(n) => n,
            None    => {
                self.compress_fail.fetch_add(1, Ordering::Relaxed);
                return ZswapStoreResult::NotCompressible;
            }
        };

        if compressed_len > ZSWAP_SLOT_SIZE {
            self.compress_fail.fetch_add(1, Ordering::Relaxed);
            return ZswapStoreResult::NotCompressible;
        }

        let csum = xor_checksum(page_data);
        let _guard = self.inner.lock();

        // Trouver un slot libre (scan linéaire).
        let slots = unsafe { &mut ZSWAP_SLOTS };
        for (idx, slot) in slots.iter_mut().enumerate() {
            if !slot.valid.load(Ordering::Acquire) {
                slot.data[..compressed_len].copy_from_slice(&tmp[..compressed_len]);
                slot.compressed_len = compressed_len as u16;
                slot.orig_pfn       = pfn;
                slot.checksum       = csum;
                slot.valid.store(true, Ordering::Release);
                self.stored.fetch_add(1, Ordering::Relaxed);
                return ZswapStoreResult::Stored(idx);
            }
        }

        ZswapStoreResult::PoolFull
    }

    /// Décompresse la page `pfn` depuis le pool dans `out` (PAGE_SIZE octets).
    /// Libère le slot après récupération.
    pub fn load(&self, pfn: u64, out: &mut [u8]) -> ZswapLoadResult {
        debug_assert_eq!(out.len(), PAGE_SIZE);
        let _guard = self.inner.lock();
        let slots  = unsafe { &mut ZSWAP_SLOTS };

        for slot in slots.iter_mut() {
            if slot.valid.load(Ordering::Acquire) && slot.orig_pfn == pfn {
                let clen = slot.compressed_len as usize;
                let ok   = Lz4Lite::decompress(&slot.data[..clen], out);
                if !ok {
                    self.decomp_fail.fetch_add(1, Ordering::Relaxed);
                    return ZswapLoadResult::DecompressError;
                }
                let csum = xor_checksum(out);
                if csum != slot.checksum {
                    self.decomp_fail.fetch_add(1, Ordering::Relaxed);
                    return ZswapLoadResult::Corrupt;
                }
                // Libérer le slot.
                slot.valid.store(false, Ordering::Release);
                slot.compressed_len = 0;
                self.decomp_ok.fetch_add(1, Ordering::Relaxed);
                self.evicted.fetch_add(1, Ordering::Relaxed);
                return ZswapLoadResult::Ok;
            }
        }
        ZswapLoadResult::NotFound
    }

    /// Invalide (expulse) le slot associé à `pfn` sans décompresser.
    pub fn evict(&self, pfn: u64) -> bool {
        let _guard = self.inner.lock();
        let slots  = unsafe { &mut ZSWAP_SLOTS };
        for slot in slots.iter_mut() {
            if slot.valid.load(Ordering::Acquire) && slot.orig_pfn == pfn {
                slot.valid.store(false, Ordering::Release);
                slot.compressed_len = 0;
                self.evicted.fetch_add(1, Ordering::Relaxed);
                return true;
            }
        }
        false
    }

    /// Nombre de slots occupés.
    pub fn occupancy(&self) -> usize {
        let _guard = self.inner.lock();
        let slots  = unsafe { &ZSWAP_SLOTS };
        slots.iter().filter(|s| s.valid.load(Ordering::Relaxed)).count()
    }
}

/// Instance globale du pool zswap.
pub static ZSWAP_POOL: ZswapPool = ZswapPool::new();
