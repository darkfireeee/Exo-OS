// kernel/src/memory/heap/thread_local/drain.rs
//
// Drain des caches per-CPU — déclenché lors d'un context switch ou d'une
// pression mémoire. Renvoie tous les objets en cache vers les pools SLUB.
//
// COUCHE 0 — aucune dépendance externe.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::memory::heap::allocator::size_classes::HEAP_SIZE_CLASSES;
use crate::memory::heap::thread_local::cache::{CACHED_SIZE_CLASSES, CPU_CACHES, MAX_CPUS};
use crate::memory::physical::allocator::slub::SLUB_CACHES;

// ─────────────────────────────────────────────────────────────────────────────
// STATISTIQUES DE DRAIN
// ─────────────────────────────────────────────────────────────────────────────

pub struct DrainStats {
    /// Nombre total d'opérations de drain déclenchées.
    pub total_drains: AtomicU64,
    /// Nombre d'objets rendus au SLUB en tout.
    pub objects_drained: AtomicU64,
    /// Drains déclenchés par pression mémoire (OOM shrink).
    pub pressure_drains: AtomicU64,
    /// Drains déclenchés par context switch.
    pub context_switch_drains: AtomicU64,
    /// Drains déclenchés explicitement (shutdown / quiesce).
    pub explicit_drains: AtomicU64,
}

impl DrainStats {
    const fn new() -> Self {
        DrainStats {
            total_drains: AtomicU64::new(0),
            objects_drained: AtomicU64::new(0),
            pressure_drains: AtomicU64::new(0),
            context_switch_drains: AtomicU64::new(0),
            explicit_drains: AtomicU64::new(0),
        }
    }
}

pub static DRAIN_STATS: DrainStats = DrainStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// POLITIQUE DE DRAIN
// ─────────────────────────────────────────────────────────────────────────────

/// Politique déterminant combien d'objets drainer par classe.
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum DrainPolicy {
    /// Drain intégral : vide les deux magazines.
    Full,
    /// Drain partiel : vide seulement le magazine `prev` (le loaded reste intact).
    Partial,
    /// Drain d'urgence : vide tous les CPUs connues (requiert l'accès global).
    AllCpus,
}

// ─────────────────────────────────────────────────────────────────────────────
// FONCTIONS DE DRAIN
// ─────────────────────────────────────────────────────────────────────────────

/// Compte le nombre d'objets drainés depuis un cache pour une classe donnée.
#[inline]
fn drain_class_inner(cpu_id: usize, class_idx: usize, policy: DrainPolicy) -> usize {
    // SAFETY: cpu_id correspond au CPU courant; aucune préemption pendant le drain.
    let cache = unsafe { CPU_CACHES.get_mut(cpu_id) };
    let slub_idx = HEAP_SIZE_CLASSES[class_idx].slab_idx as usize;
    let mut count = 0usize;

    match policy {
        DrainPolicy::Full | DrainPolicy::AllCpus => {
            while let Some(ptr) = cache.magazines[class_idx].loaded.pop() {
                // SAFETY: ptr a été alloué par SLUB_CACHES[slub_idx].
                unsafe {
                    SLUB_CACHES[slub_idx].free(ptr);
                }
                count += 1;
            }
            while let Some(_ptr) = cache.magazines[class_idx].prev.pop() {
                // SAFETY: ptr alloqué par SLUB_CACHES[slub_idx].                unsafe { SLUB_CACHES[slub_idx].free(ptr); }
                count += 1;
            }
        }
        DrainPolicy::Partial => {
            while let Some(ptr) = cache.magazines[class_idx].prev.pop() {
                // SAFETY: ptr alloqu\u00e9 par SLUB_CACHES[slub_idx] (drain partial).
                unsafe {
                    SLUB_CACHES[slub_idx].free(ptr);
                }
                count += 1;
            }
        }
    }
    count
}

/// Drain le cache per-CPU `cpu_id` selon la politique spécifiée.
///
/// # Safety
/// Doit être appelé depuis le CPU `cpu_id` (ou depuis un contexte où aucun autre
/// thread ne peut accéder au cache de ce CPU).
pub unsafe fn drain_cpu(cpu_id: usize, policy: DrainPolicy) {
    if cpu_id >= MAX_CPUS {
        return;
    }

    let mut total_objects = 0u64;

    for class_idx in 0..CACHED_SIZE_CLASSES {
        total_objects += drain_class_inner(cpu_id, class_idx, policy) as u64;
    }

    DRAIN_STATS.total_drains.fetch_add(1, Ordering::Relaxed);
    DRAIN_STATS
        .objects_drained
        .fetch_add(total_objects, Ordering::Relaxed);
}

/// Drain déclenché lors d'un context switch.
///
/// # Safety
/// Même conditions que `drain_cpu`.
#[inline]
pub unsafe fn drain_on_context_switch(cpu_id: usize) {
    // Drain partiel sur context switch pour limiter la latence.
    drain_cpu(cpu_id, DrainPolicy::Partial);
    DRAIN_STATS
        .context_switch_drains
        .fetch_add(1, Ordering::Relaxed);
}

/// Drain déclenché par pression mémoire (shrinker de l'OOM killer).
///
/// # Safety
/// Même conditions que `drain_cpu`.
#[inline]
pub unsafe fn drain_on_memory_pressure(cpu_id: usize) {
    drain_cpu(cpu_id, DrainPolicy::Full);
    DRAIN_STATS.pressure_drains.fetch_add(1, Ordering::Relaxed);
}

/// Drain explicite de tous les CPUs actifs.
/// Utilisé au shutdown ou lors d'un quiesce global.
///
/// # Safety
/// Doit être appelé quand tous les CPUs sont quiescents (pas d'allocations en cours).
pub unsafe fn drain_all_cpus() {
    for cpu_id in 0..MAX_CPUS {
        let cache = CPU_CACHES.get_mut(cpu_id);
        if !cache.active {
            continue;
        }

        let mut total = 0u64;
        for class_idx in 0..CACHED_SIZE_CLASSES {
            total += drain_class_inner(cpu_id, class_idx, DrainPolicy::AllCpus) as u64;
        }
        DRAIN_STATS
            .objects_drained
            .fetch_add(total, Ordering::Relaxed);
    }
    DRAIN_STATS.total_drains.fetch_add(1, Ordering::Relaxed);
    DRAIN_STATS.explicit_drains.fetch_add(1, Ordering::Relaxed);
}

/// Drain une seule classe de taille sur un CPU donné.
/// Utile pour un drain ciblé (ex: libérer la pression sur une seule taille).
///
/// # Safety
/// Même conditions que `drain_cpu`.
pub unsafe fn drain_cpu_class(cpu_id: usize, class_idx: usize, policy: DrainPolicy) {
    if cpu_id >= MAX_CPUS || class_idx >= CACHED_SIZE_CLASSES {
        return;
    }
    let count = drain_class_inner(cpu_id, class_idx, policy);
    DRAIN_STATS
        .objects_drained
        .fetch_add(count as u64, Ordering::Relaxed);
    DRAIN_STATS.total_drains.fetch_add(1, Ordering::Relaxed);
}

/// Retourne le nombre total d'objets en cache sur tous les CPUs actifs.
/// Ne nécessite pas de lock (lecture seule approximative).
pub fn total_cached_objects() -> usize {
    let mut total = 0usize;
    for cpu_id in 0..MAX_CPUS {
        // SAFETY: lecture approximative; on doit ignorer les races (stat non critique).
        let cache = unsafe { CPU_CACHES.get_mut(cpu_id) };
        if cache.active {
            total += cache.total_cached();
        }
    }
    total
}
