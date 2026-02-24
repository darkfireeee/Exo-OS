// kernel/src/memory/physical/frame/reclaim.rs
//
// Récupération (reclaim) de frames — implémentation de l'algorithme CLOCK
// à deux mains (active list / inactive list).
//
// Architecture :
//   • LRU inactive list — candidats au swap.
//   • LRU active   list — frames chauds, pas encore candidats.
//   • `kswapd_reclaim(target)` — tente de libérer `target` frames.
//   • Indicateur par-CPU `PF_MEMALLOC` — signale qu'un thread est déjà
//     en train de reclaimer (évite la récursion).
//   • `lru_add_new` / `lru_promote` / `lru_demote` — gestion des listes.
//
// CLOCK-pro simplifié :
//   1. Scanner `inactive` depuis le pointeur d'horloge.
//   2. Frame ACCESSED → promouvoir en `active`, clear bit ACCESSED.
//   3. Frame non ACCESSED → candidat au swap ou à la libération directe.
//   4. Frame PINNED / DMA → sauter.
//   5. Quand `active` déborde (> HIGH_WATER_ACTIVE), dégrader en `inactive`.
//
// Constantes calibrées pour un OS kernel no_std.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

use crate::memory::core::types::{Frame, PhysAddr};
use crate::memory::physical::frame::descriptor::{FrameFlags, FRAME_DESCRIPTORS};
use crate::memory::physical::allocator::free_page;
use crate::memory::swap::backend::SWAP_BACKEND;

// ─────────────────────────────────────────────────────────────────────────────
// CONSTANTES
// ─────────────────────────────────────────────────────────────────────────────

/// Taille maximale de chaque liste LRU (en nombre de frames).
pub const LRU_LIST_SIZE: usize = 8192;

/// Seuil au-delà duquel on commence à dégrader des frames active → inactive.
pub const HIGH_WATER_ACTIVE: usize = LRU_LIST_SIZE * 3 / 4;

/// Nombre maximal de CPUs (doit correspondre à `crate::arch::percpu::MAX_CPUS`).
const MAX_CPUS: usize = 512;

/// Nombre max de frames à écrire en swap par cycle de reclaim.
pub const MAX_SWAP_PER_PASS: usize = 64;

/// Bit PF_MEMALLOC dans le flags per-CPU (bit 0).
const PF_MEMALLOC_BIT: u32 = 1 << 0;
/// Bit PF_KSWAPD dans le flags per-CPU (bit 1).
const PF_KSWAPD_BIT:   u32 = 1 << 1;

// ─────────────────────────────────────────────────────────────────────────────
// FLAGS PAR-CPU
// ─────────────────────────────────────────────────────────────────────────────

/// Table de flags per-CPU pour les threads de reclaim.
static RECLAIM_FLAGS: [AtomicU32; MAX_CPUS] = {
    // SAFETY: AtomicU32::new(0) est une const.
    const INIT: AtomicU32 = AtomicU32::new(0);
    [INIT; MAX_CPUS]
};

/// Marque le CPU courant comme étant en cours de reclaim (antirecursion).
#[inline]
pub fn enter_memalloc(cpu_id: usize) {
    if cpu_id < MAX_CPUS {
        RECLAIM_FLAGS[cpu_id].fetch_or(PF_MEMALLOC_BIT, Ordering::Relaxed);
    }
}

/// Enlève le marqueur PF_MEMALLOC du CPU courant.
#[inline]
pub fn leave_memalloc(cpu_id: usize) {
    if cpu_id < MAX_CPUS {
        RECLAIM_FLAGS[cpu_id].fetch_and(!PF_MEMALLOC_BIT, Ordering::Relaxed);
    }
}

/// Retourne `true` si le CPU courant est en train de reclaimer.
#[inline]
pub fn in_memalloc(cpu_id: usize) -> bool {
    if cpu_id >= MAX_CPUS { return false; }
    RECLAIM_FLAGS[cpu_id].load(Ordering::Relaxed) & PF_MEMALLOC_BIT != 0
}

// ─────────────────────────────────────────────────────────────────────────────
// LISTES LRU (RING-BUFFER STATIQUE)
// ─────────────────────────────────────────────────────────────────────────────

/// Entrée d'une liste LRU.
#[derive(Copy, Clone)]
struct LruEntry {
    pfn:   u64,
    valid: bool,
}

impl LruEntry {
    const EMPTY: LruEntry = LruEntry { pfn: 0, valid: false };
}

struct LruList {
    entries: [LruEntry; LRU_LIST_SIZE],
    head:    usize,  // prochain slot à insérer
    tail:    usize,  // prochain slot à retirer (FIFO)
    count:   usize,
    /// Pointeur d'horloge (scan CLOCK).
    clock:   usize,
}

impl LruList {
    const fn new() -> Self {
        LruList {
            entries: [LruEntry::EMPTY; LRU_LIST_SIZE],
            head:    0,
            tail:    0,
            count:   0,
            clock:   0,
        }
    }

    fn push(&mut self, pfn: u64) -> bool {
        if self.count >= LRU_LIST_SIZE { return false; }
        self.entries[self.head] = LruEntry { pfn, valid: true };
        self.head = (self.head + 1) & (LRU_LIST_SIZE - 1);
        self.count += 1;
        true
    }

    /// Retire la prochaine entrée valide depuis la queue (FIFO).
    fn pop(&mut self) -> Option<u64> {
        while self.count > 0 {
            let e = self.entries[self.tail];
            self.entries[self.tail] = LruEntry::EMPTY;
            self.tail = (self.tail + 1) & (LRU_LIST_SIZE - 1);
            self.count -= 1;
            if e.valid { return Some(e.pfn); }
        }
        None
    }

    /// Marque l'entrée `pfn` comme invalide (retirement explicite).
    fn remove(&mut self, pfn: u64) {
        for e in self.entries.iter_mut() {
            if e.valid && e.pfn == pfn {
                e.valid = false;
                if self.count > 0 { self.count -= 1; }
                break;
            }
        }
    }

    /// Avance le pointeur d'horloge et retourne la prochaine entrée valide.
    fn clock_next(&mut self) -> Option<u64> {
        let start = self.clock;
        loop {
            let idx = self.clock;
            self.clock = (self.clock + 1) & (LRU_LIST_SIZE - 1);
            if self.entries[idx].valid {
                return Some(self.entries[idx].pfn);
            }
            if self.clock == start { break; }
        }
        None
    }

    #[inline]
    fn len(&self) -> usize { self.count }
}

// ─────────────────────────────────────────────────────────────────────────────
// STATICS PROTÉGÉS PAR MUTEX
// ─────────────────────────────────────────────────────────────────────────────

static ACTIVE_LIST:   Mutex<LruList> = Mutex::new(LruList::new());
static INACTIVE_LIST: Mutex<LruList> = Mutex::new(LruList::new());

// ─────────────────────────────────────────────────────────────────────────────
// STATISTIQUES DE RECLAIM
// ─────────────────────────────────────────────────────────────────────────────

pub struct ReclaimStats {
    /// Frames libérés directement (frame_flags: FREE / non DIRTY).
    pub freed_direct: AtomicU64,
    /// Frames envoyés en swap puis libérés.
    pub swapped_out:  AtomicU64,
    /// Frames promus de inactive → active (bit ACCESSED).
    pub promoted:     AtomicU64,
    /// Frames dégradés de active → inactive.
    pub demoted:      AtomicU64,
    /// Frames sautés (PINNED / DMA).
    pub skipped:      AtomicU64,
    /// Nb de fois que le swap a échoué.
    pub swap_errors:  AtomicU64,
}

impl ReclaimStats {
    const fn new() -> Self {
        ReclaimStats {
            freed_direct: AtomicU64::new(0),
            swapped_out:  AtomicU64::new(0),
            promoted:     AtomicU64::new(0),
            demoted:      AtomicU64::new(0),
            skipped:      AtomicU64::new(0),
            swap_errors:  AtomicU64::new(0),
        }
    }
}

pub static RECLAIM_STATS: ReclaimStats = ReclaimStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// RÉSULTAT DE RECLAIM
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Debug)]
pub struct ReclaimResult {
    pub freed:    usize,
    pub swapped:  usize,
    pub promoted: usize,
    pub skipped:  usize,
}

// ─────────────────────────────────────────────────────────────────────────────
// API PUBLIQUE LRU
// ─────────────────────────────────────────────────────────────────────────────

/// Ajoute un nouveau frame à la liste inactive (nouveau → inactif par défaut).
pub fn lru_add_new(frame: Frame) {
    let mut inactive = INACTIVE_LIST.lock();
    if !inactive.push(frame.pfn()) {
        // Liste pleine — déclencher un mini-reclaim.
        drop(inactive);
        let _ = kswapd_reclaim(8, usize::MAX);
        let _ = INACTIVE_LIST.lock().push(frame.pfn());
    }
}

/// Retire un frame des deux listes LRU (lors de `free_page`).
pub fn lru_remove(frame: Frame) {
    ACTIVE_LIST.lock().remove(frame.pfn());
    INACTIVE_LIST.lock().remove(frame.pfn());
}

/// Promeut un frame de inactive → active (page fault d'accès).
pub fn promote_to_active(frame: Frame) {
    let pfn = frame.pfn();
    {
        let mut inactive = INACTIVE_LIST.lock();
        inactive.remove(pfn);
    }
    let pushed = ACTIVE_LIST.lock().push(pfn);
    if pushed {
        RECLAIM_STATS.promoted.fetch_add(1, Ordering::Relaxed);
    }
    // Si active déborde, dégrader le plus ancien en inactive.
    if ACTIVE_LIST.lock().len() > HIGH_WATER_ACTIVE {
        let victim = ACTIVE_LIST.lock().pop();
        if let Some(v) = victim {
            let _ = INACTIVE_LIST.lock().push(v);
            RECLAIM_STATS.demoted.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Dégrade un frame de active → inactive.
pub fn demote_to_inactive(frame: Frame) {
    let pfn = frame.pfn();
    ACTIVE_LIST.lock().remove(pfn);
    let _ = INACTIVE_LIST.lock().push(pfn);
    RECLAIM_STATS.demoted.fetch_add(1, Ordering::Relaxed);
}

// ─────────────────────────────────────────────────────────────────────────────
// HELPERS FRAMES
// ─────────────────────────────────────────────────────────────────────────────

#[inline]
fn frame_from_pfn(pfn: u64) -> Frame {
    Frame::containing(PhysAddr::new(pfn << 12))
}

#[inline]
fn frame_is_reclaimable(pfn: u64) -> bool {
    let frame = frame_from_pfn(pfn);
    let desc = FRAME_DESCRIPTORS.get(frame);
    let flags = desc.flags();
    // Ignorer les frames système (PINNED) ou DMA.
    !flags.contains(FrameFlags::PINNED) && !flags.contains(FrameFlags::DMA)
}

#[inline]
fn frame_is_accessed(pfn: u64) -> bool {
    let frame = frame_from_pfn(pfn);
    FRAME_DESCRIPTORS.get(frame).flags().contains(FrameFlags::ACCESSED)
}

#[inline]
fn frame_clear_accessed(pfn: u64) {
    let frame = frame_from_pfn(pfn);
    FRAME_DESCRIPTORS.get(frame).clear_flag(FrameFlags::ACCESSED);
}

// ─────────────────────────────────────────────────────────────────────────────
// KSWAPD RECLAIM
// ─────────────────────────────────────────────────────────────────────────────

/// Cœur de l'algorithme de reclaim.
///
/// Tente de libérer `target` frames en scannant la liste inactive.
/// `cpu_id` est l'identifiant du CPU appelant (pour PF_MEMALLOC).
///
/// ## Algorithme CLOCK à une main sur la liste inactive
/// 1. Prendre prochain candidat via `clock_next()`.
/// 2. Si le frame est ACCESSED → promouvoir en active, continuer.
/// 3. Si le frame est PINNED / DMA → sauter.
/// 4. Si le frame est DIRTY → swap out puis libérer.
/// 5. Sinon → libérer directement.
pub fn kswapd_reclaim(target: usize, cpu_id: usize) -> ReclaimResult {
    let mut result = ReclaimResult { freed: 0, swapped: 0, promoted: 0, skipped: 0 };

    if cpu_id < MAX_CPUS && in_memalloc(cpu_id) {
        // Récursion détectée — ne pas reclaimer.
        return result;
    }

    if cpu_id < MAX_CPUS {
        enter_memalloc(cpu_id);
    }

    let mut passes = 0usize;
    let max_scan = LRU_LIST_SIZE.min(target * 4);

    while result.freed < target && passes < max_scan {
        passes += 1;

        // Obtenir le PFN suivant dans la liste inactive via CLOCK.
        let pfn = {
            let mut inactive = INACTIVE_LIST.lock();
            inactive.clock_next()
        };

        let pfn = match pfn {
            Some(p) => p,
            None    => break, // liste vide
        };

        // Frame PINNED / DMA → sauter.
        if !frame_is_reclaimable(pfn) {
            RECLAIM_STATS.skipped.fetch_add(1, Ordering::Relaxed);
            result.skipped += 1;
            continue;
        }

        // Frame accédé récemment → promouvoir en active.
        if frame_is_accessed(pfn) {
            frame_clear_accessed(pfn);
            let frame = frame_from_pfn(pfn);
            promote_to_active(frame);
            result.promoted += 1;
            RECLAIM_STATS.promoted.fetch_add(1, Ordering::Relaxed);
            continue;
        }

        // Le frame est candidat à la libération ou au swap.
        let frame = frame_from_pfn(pfn);
        let frame_flags = FRAME_DESCRIPTORS.get(frame).flags();

        if frame_flags.contains(FrameFlags::DIRTY) {
            // Écrire en swap avant de libérer.
            let swap_written = unsafe { try_swap_out(frame) };
            if swap_written {
                // Retirer de la liste inactive et libérer le frame physique.
                INACTIVE_LIST.lock().remove(pfn);
                let _ = free_page(frame);
                result.freed   += 1;
                result.swapped += 1;
                RECLAIM_STATS.swapped_out.fetch_add(1, Ordering::Relaxed);
                RECLAIM_STATS.freed_direct.fetch_add(1, Ordering::Relaxed);
            } else {
                RECLAIM_STATS.swap_errors.fetch_add(1, Ordering::Relaxed);
                result.skipped += 1;
            }
        } else {
            // Frame propre — libération directe.
            INACTIVE_LIST.lock().remove(pfn);
            let _ = free_page(frame);
            result.freed += 1;
            RECLAIM_STATS.freed_direct.fetch_add(1, Ordering::Relaxed);
        }

        // Gérer le débordement de la liste active.
        if ACTIVE_LIST.lock().len() > HIGH_WATER_ACTIVE {
            let victim = ACTIVE_LIST.lock().pop();
            if let Some(v) = victim {
                let _ = INACTIVE_LIST.lock().push(v);
                RECLAIM_STATS.demoted.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    if cpu_id < MAX_CPUS {
        leave_memalloc(cpu_id);
    }

    result
}

/// Tente d'écrire le frame dans le swap.
/// Retourne `true` si l'écriture a réussi.
///
/// # Safety
/// Accède à la physmap — requiert que PHYS_MAP_BASE soit initialisé.
unsafe fn try_swap_out(frame: Frame) -> bool {
    let (dev_idx, slot) = match SWAP_BACKEND.alloc_slot() {
        Ok(pair) => pair,
        Err(_)   => return false,
    };
    // Écrire PAGE_SIZE octets depuis la physmap vers le swap device.
    match SWAP_BACKEND.write_page(dev_idx, slot, frame.phys_addr()) {
        Ok(_)  => true,
        Err(_) => {
            // Libérer le slot si l'écriture échoue.
            SWAP_BACKEND.free_slot(dev_idx, slot);
            false
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// QUERY STATS
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne le nombre de frames dans chaque liste.
pub fn lru_counts() -> (usize, usize) {
    let active   = ACTIVE_LIST.lock().len();
    let inactive = INACTIVE_LIST.lock().len();
    (active, inactive)
}
