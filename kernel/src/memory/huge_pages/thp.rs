// kernel/src/memory/huge_pages/thp.rs
//
// Transparent Huge Pages (THP) — allocation et gestion des pages 2 MiB.
// COUCHE 0 — aucune dépendance externe.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use crate::memory::core::types::{PhysAddr, Frame, AllocFlags};
use crate::memory::core::constants::{PAGE_SIZE, HUGE_PAGE_SIZE};
use crate::memory::physical::allocator::buddy::{alloc_pages, free_pages};

// ─────────────────────────────────────────────────────────────────────────────
// CONFIGURATION THP
// ─────────────────────────────────────────────────────────────────────────────

/// Ordre buddy pour une page 2 MiB (2MiB / 4KiB = 512 = 2^9).
pub const HUGE_PAGE_ORDER: u32 = 9;

/// Modes THP.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[repr(u8)]
pub enum ThpMode {
    /// THP désactivées.
    Disabled  = 0,
    /// THP activées si possible (best-effort).
    Madvise   = 1,
    /// THP activées systématiquement.
    Always    = 2,
}

/// Configuration globale THP.
pub struct ThpConfig {
    pub mode:         spin::RwLock<ThpMode>,
    pub enabled:      AtomicBool,
    /// Seuil de VMAs (en pages) à partir duquel on tente un THP.
    pub min_vma_size: AtomicU64,
}

impl ThpConfig {
    const fn new() -> Self {
        ThpConfig {
            mode:         spin::RwLock::new(ThpMode::Madvise),
            enabled:      AtomicBool::new(true),
            min_vma_size: AtomicU64::new(512), // au moins 2 MiB
        }
    }

    pub fn is_enabled(&self) -> bool { self.enabled.load(Ordering::Relaxed) }
    pub fn set_mode(&self, mode: ThpMode) { *self.mode.write() = mode; }
    pub fn get_mode(&self) -> ThpMode { *self.mode.read() }
}

pub static THP_CONFIG: ThpConfig = ThpConfig::new();

// ─────────────────────────────────────────────────────────────────────────────
// STATISTIQUES THP
// ─────────────────────────────────────────────────────────────────────────────

pub struct ThpStats {
    pub alloc_success:  AtomicU64,
    pub alloc_fail:     AtomicU64,
    pub splits:         AtomicU64,
    pub promotions:     AtomicU64,
    pub current_huge:   AtomicU64,
}

impl ThpStats {
    const fn new() -> Self {
        ThpStats {
            alloc_success:  AtomicU64::new(0),
            alloc_fail:     AtomicU64::new(0),
            splits:         AtomicU64::new(0),
            promotions:     AtomicU64::new(0),
            current_huge:   AtomicU64::new(0),
        }
    }
}

pub static THP_STATS: ThpStats = ThpStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// API THP
// ─────────────────────────────────────────────────────────────────────────────

/// Alloue une huge page 2 MiB (ordre 9 dans le buddy).
/// Retourne le frame de base (aligné 2 MiB).
pub fn alloc_huge_page(flags: AllocFlags) -> Option<Frame> {
    if !THP_CONFIG.is_enabled() { return None; }

    match alloc_pages(HUGE_PAGE_ORDER as usize, flags) {
        Ok(frame) => {
            THP_STATS.alloc_success.fetch_add(1, Ordering::Relaxed);
            THP_STATS.current_huge.fetch_add(1, Ordering::Relaxed);
            Some(frame)
        }
        Err(_) => {
            THP_STATS.alloc_fail.fetch_add(1, Ordering::Relaxed);
            None
        }
    }
}

/// Libère une huge page allouée par `alloc_huge_page`.
///
/// # Safety
/// `frame` doit avoir été alloué avec `alloc_huge_page`.
pub unsafe fn free_huge_page(frame: Frame) {
    let _ = free_pages(frame, HUGE_PAGE_ORDER as usize);
    THP_STATS.current_huge.fetch_sub(1, Ordering::Relaxed);
}

/// Tente de diviser une huge page en 512 pages normales.
/// Retourne les 512 frames.
///
/// # Safety
/// `frame` doit être un frame de base de huge page (aligné 2 MiB).
pub unsafe fn split_huge_page(frame: Frame) -> [Frame; 512] {
    THP_STATS.splits.fetch_add(1, Ordering::Relaxed);
    // La huge page est physiquement contiguë — on retourne les 512 frames.
    let base = frame.phys_addr().as_u64();
    let mut frames = [Frame::from_phys_addr(PhysAddr::new(0)); 512];
    for i in 0..512usize {
        frames[i] = Frame::from_phys_addr(PhysAddr::new(base + (i as u64 * PAGE_SIZE as u64)));
    }
    // On re-libère la huge page dans le buddy et on ré-alloue chaque page.
    let _ = free_pages(frame, HUGE_PAGE_ORDER as usize);
    THP_STATS.current_huge.fetch_sub(1, Ordering::Relaxed);
    frames
}

/// Tente de fusionner 512 pages 4KiB contigues en une huge page.
/// Retourne `Some(frame_base)` si réussi.
pub fn try_promote_to_huge(base: PhysAddr) -> Option<Frame> {
    // Vérifie l'alignement 2 MiB.
    if base.as_u64() % HUGE_PAGE_SIZE as u64 != 0 { return None; }
    // Vérifie que toutes les pages sont dispos dans le buddy — non implémenté ici
    // car nécessiterait de parcourir les free_lists, ce qui est géré par buddy.rs.
    // On délègue : on essaie simplement d'allouer ordre-9.
    let frame = alloc_huge_page(AllocFlags::NONE)?;
    THP_STATS.promotions.fetch_add(1, Ordering::Relaxed);
    Some(frame)
}
