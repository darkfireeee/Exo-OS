// ipc/shared_memory/pool.rs — Pool de pages SHM pré-allouées pour Exo-OS
//
// Pool statique de SHM_POOL_PAGES (256) pages de 4 KiB chacune.
// Allocations en O(1) via bitmap d'occupation (AtomicU64 × MAX_POOL_WORDS).
// Invariant : toutes les pages ont le flag NO_COW positionné.
//
// Contrainte de performance : alloc < 100 ns (lock-free CAS sur bitmap)

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::ipc::core::constants::SHM_POOL_PAGES;
use crate::ipc::shared_memory::page::{PageFlags, PhysAddr, ShmPage, PAGE_SIZE};

// ---------------------------------------------------------------------------
// Bitmap d'allocation — 256 pages = 4 mots de 64 bits
// ---------------------------------------------------------------------------

/// Nombre de mots u64 dans le bitmap (SHM_POOL_PAGES / 64)
pub const POOL_BITMAP_WORDS: usize = SHM_POOL_PAGES / 64;

/// Adresse physique de base du pool (fixée au démarrage par init_shm_pool())
static POOL_BASE_PHYS: AtomicU64 = AtomicU64::new(0);

/// Bitmap des slots libres (bit=1 → libre, bit=0 → occupé)
static POOL_BITMAP: [AtomicU64; POOL_BITMAP_WORDS] = [
    AtomicU64::new(u64::MAX),
    AtomicU64::new(u64::MAX),
    AtomicU64::new(u64::MAX),
    AtomicU64::new(u64::MAX),
];

/// Descripteurs de pages
static POOL_PAGES: [ShmPage; SHM_POOL_PAGES] = {
    const INIT: ShmPage = ShmPage::new_uninit();
    [INIT; SHM_POOL_PAGES]
};

/// Compteur de pages libres (approximatif, pour diagnostic rapide)
static POOL_FREE_COUNT: AtomicUsize = AtomicUsize::new(SHM_POOL_PAGES);

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise le pool SHM avec une adresse physique de base.
/// À appeler une seule fois au démarrage du noyau (depuis ipc_init()).
///
/// # SAFETY
/// Doit être appelé AVANT toute allocation SHM. `base_phys` doit pointer
/// sur une région physique de `SHM_POOL_PAGES * PAGE_SIZE` octets réservée
/// par le memory manager.
pub unsafe fn init_shm_pool(base_phys: u64) {
    POOL_BASE_PHYS.store(base_phys, Ordering::Relaxed);

    for i in 0..SHM_POOL_PAGES {
        let phys = PhysAddr(base_phys + (i * PAGE_SIZE) as u64);
        POOL_PAGES[i].init(phys, i as u32, PageFlags::SHM_DEFAULT);
    }

    // Marquer toutes les pages comme libres
    for w in POOL_BITMAP.iter() {
        w.store(u64::MAX, Ordering::Release);
    }
    POOL_FREE_COUNT.store(SHM_POOL_PAGES, Ordering::Release);
}

// ---------------------------------------------------------------------------
// Allocation / libération de pages
// ---------------------------------------------------------------------------

/// Alloue une page SHM depuis le pool global.
///
/// Algorithme :
///   1. Parcourir les 4 mots du bitmap
///   2. Pour chaque mot non-zéro, isoler le bit le plus bas (trailing_zeros)
///   3. CAS atomique pour réserver le bit (1 → 0)
///   4. Retourner l'index de page = word * 64 + bit
///
/// Complexité : O(POOL_BITMAP_WORDS) = O(1)
/// Garantie lock-free : pas de mutex, seul un CAS par allocation.
pub fn shm_page_alloc() -> Option<usize> {
    for (word_idx, word) in POOL_BITMAP.iter().enumerate() {
        loop {
            let v = word.load(Ordering::Acquire);
            if v == 0 {
                break; // ce mot est plein, passer au suivant
            }
            let bit = v.trailing_zeros() as usize;
            let mask = 1u64 << bit;
            // Tentative CAS : marquer le bit comme occupé (1 → 0)
            match word.compare_exchange_weak(v, v & !mask, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => {
                    let page_idx = word_idx * 64 + bit;
                    if page_idx < SHM_POOL_PAGES {
                        POOL_PAGES[page_idx].ref_acquire();
                        POOL_FREE_COUNT.fetch_sub(1, Ordering::Relaxed);
                        return Some(page_idx);
                    }
                    // Index hors limites (ne devrait pas arriver) — libérer
                    word.fetch_or(mask, Ordering::Release);
                    return None;
                }
                Err(_) => {
                    // Contention : retour au début de la boucle
                    core::hint::spin_loop();
                    continue;
                }
            }
        }
    }
    None // pool exhausted
}

/// Libère la page à l'index `page_idx` dans le pool global.
/// Retourne `true` si la libération a réussi.
pub fn shm_page_free(page_idx: usize) -> bool {
    if page_idx >= SHM_POOL_PAGES {
        return false;
    }

    let freed = POOL_PAGES[page_idx].ref_release();
    if freed {
        // Remettre le bit à 1 dans le bitmap
        let word_idx = page_idx / 64;
        let bit = page_idx % 64;
        let mask = 1u64 << bit;
        POOL_BITMAP[word_idx].fetch_or(mask, Ordering::Release);
        POOL_FREE_COUNT.fetch_add(1, Ordering::Relaxed);
    }
    freed
}

/// Retourne la référence à la ShmPage pour `page_idx`.
pub fn shm_page_ref(page_idx: usize) -> Option<&'static ShmPage> {
    if page_idx < SHM_POOL_PAGES {
        Some(&POOL_PAGES[page_idx])
    } else {
        None
    }
}

/// Retourne l'adresse physique de la page `page_idx`.
pub fn shm_page_phys(page_idx: usize) -> Option<PhysAddr> {
    if page_idx < SHM_POOL_PAGES {
        Some(POOL_PAGES[page_idx].phys_addr)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Statistiques du pool
// ---------------------------------------------------------------------------

/// Retourne un snapshot des statistiques du pool.
pub fn shm_pool_stats() -> ShmPoolStats {
    let free = POOL_FREE_COUNT.load(Ordering::Relaxed);
    let mut total_reuses: u64 = 0;
    for p in POOL_PAGES.iter() {
        total_reuses += p.reuse_count.load(Ordering::Relaxed) as u64;
    }

    ShmPoolStats {
        total_pages: SHM_POOL_PAGES,
        free_pages: free,
        used_pages: SHM_POOL_PAGES.saturating_sub(free),
        base_phys: POOL_BASE_PHYS.load(Ordering::Relaxed),
        total_reuses,
    }
}

/// Snapshot des statistiques du pool SHM
#[derive(Debug, Clone, Copy)]
pub struct ShmPoolStats {
    pub total_pages: usize,
    pub free_pages: usize,
    pub used_pages: usize,
    pub base_phys: u64,
    pub total_reuses: u64,
}

// ---------------------------------------------------------------------------
// Allocation de plages contigues (pour les gros buffers)
// ---------------------------------------------------------------------------

/// Alloue `count` pages contiguës dans le pool.
/// Retourne l'index de la première page, ou `None` si impossible.
///
/// Note : linéaire O(SHM_POOL_PAGES) — réservé aux allocations rares/init.
pub fn shm_alloc_contiguous(count: usize) -> Option<usize> {
    if count == 0 || count > SHM_POOL_PAGES {
        return None;
    }

    // Recherche d'une plage de `count` bits consécutifs valorisés à 1 dans le bitmap
    'outer: for start in 0..=(SHM_POOL_PAGES - count) {
        // Vérifier que toutes les pages [start, start+count) sont libres
        for i in 0..count {
            let idx = start + i;
            let word_idx = idx / 64;
            let bit = idx % 64;
            let v = POOL_BITMAP[word_idx].load(Ordering::Acquire);
            if (v & (1u64 << bit)) == 0 {
                continue 'outer;
            }
        }

        // Toutes libres — essayer de les réserver atomiquement
        // (on utilise des CAS individuels — acceptable pour une opération rare)
        let mut reserved = 0usize;
        for i in 0..count {
            let idx = start + i;
            let word_idx = idx / 64;
            let bit = idx % 64;
            let mask = 1u64 << bit;
            let v = POOL_BITMAP[word_idx].load(Ordering::Acquire);

            match POOL_BITMAP[word_idx].compare_exchange(
                v,
                v & !mask,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    POOL_PAGES[idx].ref_acquire();
                    POOL_FREE_COUNT.fetch_sub(1, Ordering::Relaxed);
                    reserved += 1;
                }
                Err(_) => {
                    // Contention — annuler les réservations précédentes
                    for j in 0..reserved {
                        shm_page_free(start + j);
                    }
                    continue 'outer;
                }
            }
        }
        return Some(start);
    }
    None
}

/// Libère une plage de `count` pages contigues commençant à `start`.
pub fn shm_free_contiguous(start: usize, count: usize) -> bool {
    if start + count > SHM_POOL_PAGES {
        return false;
    }
    for i in 0..count {
        shm_page_free(start + i);
    }
    true
}
