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

use crate::memory::core::constants::PAGE_SIZE;
use crate::memory::core::types::PhysAddr;

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
// LZ4LITE — IMPLÉMENTATION LZ77 SIMPLIFIÉE
// ─────────────────────────────────────────────────────────────────────────────

/// Longueur minimale d'un match pour justifier une back-reference.
const MIN_MATCH_LEN: usize = 3;
/// Longueur maximale stockable dans un token de match (3..10).
const MAX_MATCH_LEN: usize = 10;
/// Offset maximal pour une back-reference (13 bits → 8191).
const MAX_OFFSET:    usize = 8191;

pub struct Lz4Lite;

impl CompressBackend for Lz4Lite {
    fn compress(src: &[u8], dst: &mut [u8]) -> Option<usize> {
        debug_assert_eq!(src.len(), PAGE_SIZE);
        let mut out  = 0usize;
        let mut pos  = 0usize;
        let src_len  = src.len();

        // Buffer de littéraux en attente avant flush.
        let mut lit_start = 0usize;
        let mut lit_count = 0usize;

        macro_rules! flush_literals {
            () => {
                while lit_count > 0 {
                    let chunk = lit_count.min(128);
                    if out >= dst.len() { return None; }
                    dst[out] = (chunk - 1) as u8;  // bit7=0, LLLLLLL
                    out += 1;
                    if out + chunk > dst.len() { return None; }
                    dst[out..out + chunk].copy_from_slice(&src[lit_start..lit_start + chunk]);
                    out  += chunk;
                    lit_start += chunk;
                    lit_count -= chunk;
                }
            };
        }

        while pos < src_len {
            // Chercher la plus longue back-reference dans la fenêtre glissante.
            let window_start = if pos > MAX_OFFSET { pos - MAX_OFFSET } else { 0 };
            let mut best_offset = 0usize;
            let mut best_len    = 0usize;

            // Limite de recherche pour rester O(N) amortis pour des données réelles.
            // Inspecter jusqu'à 32 positions candidate par position.
            let search_limit = 32usize;
            let wend = pos;
            let mut wpos = if wend > window_start + search_limit {
                wend - search_limit
            } else {
                window_start
            };
            while wpos < wend {
                let mut mlen = 0usize;
                while mlen < MAX_MATCH_LEN
                    && pos + mlen < src_len
                    && src[wpos + mlen] == src[pos + mlen]
                {
                    mlen += 1;
                }
                if mlen >= MIN_MATCH_LEN && mlen > best_len {
                    best_len    = mlen;
                    best_offset = pos - wpos;
                }
                wpos += 1;
            }

            if best_len >= MIN_MATCH_LEN {
                // Émettre les littéraux en attente.
                flush_literals!();

                // Émettre le token de match : 1_LLLOOO OO_OOOOOO OOOOOOOO
                let offset_bits = best_offset & 0x1FFF;  // 13 bits
                let len_bits    = (best_len - MIN_MATCH_LEN) & 0x7; // 3 bits
                let b0 = 0x80u8 | ((len_bits as u8) << 4) | ((offset_bits >> 8) as u8);
                let b1 = (offset_bits & 0xFF) as u8;
                if out + 2 > dst.len() { return None; }
                dst[out]     = b0;
                dst[out + 1] = b1;
                out += 2;
                pos += best_len;
                lit_start = pos;
            } else {
                // Pas de match — accumuler un littéral.
                lit_count += 1;
                pos       += 1;
            }
        }

        flush_literals!();
        Some(out)
    }

    fn decompress(src: &[u8], dst: &mut [u8]) -> bool {
        debug_assert_eq!(dst.len(), PAGE_SIZE);
        let mut inp  = 0usize;
        let mut outp = 0usize;

        while inp < src.len() {
            let token = src[inp];
            inp += 1;

            if token & 0x80 == 0 {
                // Séquence de littéraux.
                let count = (token as usize) + 1;
                if inp + count > src.len() || outp + count > dst.len() {
                    return false;
                }
                dst[outp..outp + count].copy_from_slice(&src[inp..inp + count]);
                inp  += count;
                outp += count;
            } else {
                // Back-reference.
                if inp >= src.len() { return false; }
                let b1      = src[inp] as usize;
                inp += 1;
                let len_bits   = ((token >> 4) & 0x07) as usize;
                let offset_hi  = (token & 0x1F) as usize;
                let offset     = (offset_hi << 8) | b1;
                let match_len  = len_bits + MIN_MATCH_LEN;

                if offset == 0 || offset > outp { return false; }
                if outp + match_len > dst.len() { return false; }

                let start = outp - offset;
                // Copie octet par octet pour gérer les chevauchements.
                for i in 0..match_len {
                    dst[outp + i] = dst[start + i];
                }
                outp += match_len;
            }
        }
        // La décompression doit remplir exactement PAGE_SIZE.
        outp == PAGE_SIZE
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
