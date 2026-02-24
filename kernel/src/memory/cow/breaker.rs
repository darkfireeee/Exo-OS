// kernel/src/memory/cow/breaker.rs
//
// Rupture CoW (Copy-on-Write break) — copie physique d'un frame partagé.
//
// Quand un processus tente d'écrire sur une page marquée `COW` et
// partagée (refcount ≥ 2), le page fault handler appelle `break_cow`.
// Cette fonction :
//   1. Alloue un nouveau frame physique exclusif.
//   2. Copie le contenu de l'ancien frame vers le nouveau (physmap).
//   3. Décrémente le refcount de l'ancien frame via `COW_TRACKER`.
//   4. Retourne le nouveau frame (le caller doit mettre à jour le PTE).
//
// Si le frame est déjà exclusif (refcount == 1), la copie est évitée
// et le même frame est retourné après vérification.
//
// COUCHE 0 — aucune dépendance externe.

use core::sync::atomic::Ordering;

use crate::memory::core::types::{Frame, PhysAddr, AllocFlags, AllocError};
use crate::memory::core::constants::PAGE_SIZE;
use crate::memory::core::layout::PHYS_MAP_BASE;
use crate::memory::physical::allocator::{alloc_page, free_page};
use crate::memory::cow::tracker::COW_TRACKER;

// ─────────────────────────────────────────────────────────────────────────────
// RÉSULTAT D'UNE RUPTURE CoW
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat de `break_cow`.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum CowBreakOutcome {
    /// Nouvelle copie allouée. Le frame retourné remplace l'ancien dans le PTE.
    Copied(Frame),
    /// Le frame était déjà exclusif (refcount == 1) — aucune copie nécessaire.
    /// Le même frame peut être utilisé en écriture directement.
    AlreadyExclusive(Frame),
    /// Allocation de la nouvelle copie a échoué (OOM).
    Oom,
}

// ─────────────────────────────────────────────────────────────────────────────
// STATISTIQUES
// ─────────────────────────────────────────────────────────────────────────────

use core::sync::atomic::AtomicU64;

pub struct CowBreakerStats {
    /// Ruptures CoW effectuées (copie physique réalisée).
    pub breaks:         AtomicU64,
    /// Frames qui étaient déjà exclusifs (zéro copy).
    pub already_excl:   AtomicU64,
    /// Échecs OOM lors de la rupture.
    pub oom_failures:   AtomicU64,
    /// Octets copiés au total via les ruptures CoW.
    pub bytes_copied:   AtomicU64,
}

impl CowBreakerStats {
    const fn new() -> Self {
        CowBreakerStats {
            breaks:       AtomicU64::new(0),
            already_excl: AtomicU64::new(0),
            oom_failures: AtomicU64::new(0),
            bytes_copied: AtomicU64::new(0),
        }
    }
}

pub static COW_BREAKER_STATS: CowBreakerStats = CowBreakerStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// RUPTURE CoW
// ─────────────────────────────────────────────────────────────────────────────

/// Brise le partage CoW sur `frame` et retourne le frame exclusif résultant.
///
/// ## Algorithme
/// 1. Consulter `COW_TRACKER` pour le refcount actuel.
/// 2. Si refcount == 1  → retourner `AlreadyExclusive(frame)`.
/// 3. Allouer un nouveau frame via `alloc_page(AllocFlags::NONE)`.
/// 4. Copier PAGE_SIZE octets de `frame` vers le nouveau via physmap.
/// 5. Décrémenter le refcount de `frame` via `COW_TRACKER.dec(frame)`.
/// 6. Retourner `Copied(new_frame)`.
///
/// ## Post-conditions pour le caller
/// - L'ancien `frame` a son refcount décrémenté d'une unité.
/// - Si `COW_TRACKER.dec()` retourne 0, libérer l'ancien frame.
/// - Mettre à jour le PTE pour pointer vers `new_frame`.
/// - Effacer le bit `COW` dans les flags du PTE de cette copie.
///
/// # Safety
/// - Les deux adresses physiques (src et dst) doivent être dans la physmap.
/// - La physmap (`PHYS_MAP_BASE`) doit être initialisée avant l'appel.
pub unsafe fn break_cow(frame: Frame) -> CowBreakOutcome {
    let refcount = COW_TRACKER.ref_count(frame);

    // Si le frame est déjà exclusif, pas de copie nécessaire.
    if refcount <= 1 {
        COW_BREAKER_STATS.already_excl.fetch_add(1, Ordering::Relaxed);
        return CowBreakOutcome::AlreadyExclusive(frame);
    }

    // Allouer un nouveau frame (non zéro — on va le remplir immédiatement).
    let new_frame = match alloc_page(AllocFlags::NONE) {
        Ok(f)  => f,
        Err(_) => {
            COW_BREAKER_STATS.oom_failures.fetch_add(1, Ordering::Relaxed);
            return CowBreakOutcome::Oom;
        }
    };

    // Copier le contenu de l'ancien frame vers le nouveau via la physmap directe.
    let phys_base = PHYS_MAP_BASE.as_u64();
    let src = (phys_base + frame.phys_addr().as_u64()) as *const u8;
    let dst = (phys_base + new_frame.phys_addr().as_u64()) as *mut u8;

    // SAFETY: La physmap couvre toute la RAM. PAGE_SIZE est fixe (4096).
    // Les frames ne se chevauchent jamais (chacun est à une adresse distincte).
    core::ptr::copy_nonoverlapping(src, dst, PAGE_SIZE);

    // Décrémenter le refcount de l'ancien frame.
    let new_rc = COW_TRACKER.dec(frame);
    if new_rc == 0 {
        // Dernier référent parti — libérer l'ancien frame.
        let _ = free_page(frame);
    }

    COW_BREAKER_STATS.breaks.fetch_add(1, Ordering::Relaxed);
    COW_BREAKER_STATS.bytes_copied.fetch_add(PAGE_SIZE as u64, Ordering::Relaxed);

    CowBreakOutcome::Copied(new_frame)
}

/// Variante "best-effort" : si OOM, retourner une erreur sans paniquer.
/// Identique à `break_cow` mais retourne un `Result`.
///
/// # Safety
/// Mêmes préconditions que `break_cow`.
#[inline]
pub unsafe fn try_break_cow(frame: Frame) -> Result<Frame, AllocError> {
    match break_cow(frame) {
        CowBreakOutcome::Copied(f)          => Ok(f),
        CowBreakOutcome::AlreadyExclusive(f) => Ok(f),
        CowBreakOutcome::Oom                 => Err(AllocError::OutOfMemory),
    }
}
