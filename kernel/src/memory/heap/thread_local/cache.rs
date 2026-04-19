// kernel/src/memory/heap/thread_local/cache.rs
//
// Cache per-CPU pour les petites allocations heap.
// Chaque CPU maintient une paire de magazines par classe de taille.
// Hot path alloc/free entièrement sans lock.
//
// COUCHE 0 — aucune dépendance externe.

use core::ptr::NonNull;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::memory::heap::thread_local::magazine::{CpuMagazinePair, MAGAZINE_SIZE};
use crate::memory::heap::allocator::size_classes::{
    HEAP_SIZE_CLASSES, heap_size_class_for,
};
use crate::memory::physical::allocator::slub::SLUB_CACHES;
use crate::memory::core::types::AllocFlags;

/// Nombre maximum de CPUs supportés.
pub use crate::memory::core::constants::MAX_CPUS;

/// Assertion compile-time.
const _: () = assert!(
    MAX_CPUS == crate::memory::core::constants::MAX_CPUS,
    "heap cache MAX_CPUS doit correspondre à memory::core::constants::MAX_CPUS"
);

// ─────────────────────────────────────────────────────────────────────────────
// STATISTIQUES PAR CPU
// ─────────────────────────────────────────────────────────────────────────────

/// Statistiques détaillées par CPU et par classe de taille.
pub struct PerCpuCacheStats {
    pub hits:       AtomicU64,   // allocs servies par le cache local
    pub misses:     AtomicU64,   // allocs qui ont dû aller au SLUB
    pub free_hits:  AtomicU64,   // frees absorbées par le cache local
    pub free_miss:  AtomicU64,   // frees renvoyées au SLUB
    pub drains:     AtomicU64,   // nombre de drain complets (contexte switch, etc.)
    pub refills:    AtomicU64,   // nombre de refills depuis le SLUB
}

impl PerCpuCacheStats {
    const fn new() -> Self {
        PerCpuCacheStats {
            hits:       AtomicU64::new(0),
            misses:     AtomicU64::new(0),
            free_hits:  AtomicU64::new(0),
            free_miss:  AtomicU64::new(0),
            drains:     AtomicU64::new(0),
            refills:    AtomicU64::new(0),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CACHE PER-CPU (STRUCTURE PRINCIPALE)
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre de classes de taille couvertes par le cache per-CPU.
/// On couvre uniquement les classes ≤ 2048 octets (SLUB path).
pub const CACHED_SIZE_CLASSES: usize = 16;

/// Cache per-CPU = tableau de paires de magazines, une par classe de taille.
///
/// Aligné sur 64 octets (évite les faux-partages entre CPUs dans un tableau).
#[repr(C, align(64))]
pub struct PerCpuCache {
    /// Paires de magazines pour chaque classe de taille.
    pub magazines: [CpuMagazinePair; CACHED_SIZE_CLASSES],
    /// CPU propriétaire de ce cache.
    cpu_id:    u32,
    /// Cache actif ? (faux tant que non initialisé).
    pub active:    bool,
    /// Statistiques.
    pub stats: PerCpuCacheStats,
    /// Padding pour aligner à 64 * N.
    _pad: [u8; 3],
}

impl PerCpuCache {
    /// Construit un cache vide (sans initialisation runtime).
    pub const fn new_uninit() -> Self {
        // On génère les 16 paires de magazines à la construction.
        // Rust const fn ne permet pas encore les boucles sur [T; N] en stable,
        // on initialise explicitement.
        PerCpuCache {
            magazines: [
                CpuMagazinePair::new(0),
                CpuMagazinePair::new(1),
                CpuMagazinePair::new(2),
                CpuMagazinePair::new(3),
                CpuMagazinePair::new(4),
                CpuMagazinePair::new(5),
                CpuMagazinePair::new(6),
                CpuMagazinePair::new(7),
                CpuMagazinePair::new(8),
                CpuMagazinePair::new(9),
                CpuMagazinePair::new(10),
                CpuMagazinePair::new(11),
                CpuMagazinePair::new(12),
                CpuMagazinePair::new(13),
                CpuMagazinePair::new(14),
                CpuMagazinePair::new(15),
            ],
            cpu_id: 0,
            active: false,
            stats:  PerCpuCacheStats::new(),
            _pad:   [0u8; 3],
        }
    }

    /// Initialise le cache pour un CPU donné.
    pub fn init(&mut self, cpu_id: u32) {
        self.cpu_id = cpu_id;
        self.active = true;
        for (i, mag) in self.magazines.iter_mut().enumerate() {
            mag.loaded.size_class = i;
            mag.prev.size_class   = i;
        }
    }

    /// Alloue un objet de `size` octets depuis le cache per-CPU.
    ///
    /// Retourne `None` si le cache est épuisé pour cette classe
    /// (l'appelant doit alors appeler `alloc_slow`).
    #[inline]
    pub fn alloc_fast(&mut self, size: usize) -> Option<NonNull<u8>> {
        if !self.active { return None; }
        let class_idx = heap_size_class_for(size)?;
        if class_idx >= CACHED_SIZE_CLASSES { return None; }

        if let Some(ptr) = self.magazines[class_idx].alloc() {
            self.stats.hits.fetch_add(1, Ordering::Relaxed);
            Some(ptr)
        } else {
            self.stats.misses.fetch_add(1, Ordering::Relaxed);
            None
        }
    }

    /// Libère `ptr` (de taille `size`) dans le cache per-CPU.
    ///
    /// Retourne `false` si le cache est saturé (l'appelant doit drainer).
    #[inline]
    pub fn free_fast(&mut self, ptr: NonNull<u8>, size: usize) -> bool {
        if !self.active { return false; }
        let class_idx = match heap_size_class_for(size) {
            Some(idx) => idx,
            None      => return false,
        };
        if class_idx >= CACHED_SIZE_CLASSES { return false; }

        if self.magazines[class_idx].free(ptr) {
            self.stats.free_hits.fetch_add(1, Ordering::Relaxed);
            true
        } else {
            self.stats.free_miss.fetch_add(1, Ordering::Relaxed);
            false
        }
    }

    /// Chemin lent : recharge le magazine depuis le SLUB puis sert l'allocation.
    pub fn alloc_slow(&mut self, _size: usize, class_idx: usize) -> Option<NonNull<u8>> {
        // Remplit le magazine loaded depuis le SLUB (batch de MAGAZINE_SIZE/2).
        let batch = MAGAZINE_SIZE / 2;
        let slub_idx = HEAP_SIZE_CLASSES[class_idx].slab_idx as usize;
        let mut loaded = 0usize;

        for _ in 0..batch {
            match SLUB_CACHES[slub_idx].alloc(AllocFlags::NONE) {
                Ok(ptr) => {
                    // Pousse directement dans le magazine prev (loaded est plein côté alloc).
                    if !self.magazines[class_idx].prev.push(ptr) { break; }
                    loaded += 1;
                }
                Err(_) => break,
            }
        }

        if loaded == 0 { return None; }

        // Swap loaded ↔ prev pour remettre le prev rempli en position "loaded".
        core::mem::swap(
            &mut self.magazines[class_idx].loaded,
            &mut self.magazines[class_idx].prev,
        );

        self.stats.refills.fetch_add(1, Ordering::Relaxed);
        self.magazines[class_idx].loaded.pop()
    }

    /// Drain le magazine chargé vers le SLUB (appelé lors d'un context switch
    /// ou quand le magazine est plein sur le chemin lent).
    pub fn drain_class(&mut self, class_idx: usize) {
        if class_idx >= CACHED_SIZE_CLASSES { return; }
        let slub_idx = HEAP_SIZE_CLASSES[class_idx].slab_idx as usize;

        // Vide loaded.
        while let Some(ptr) = self.magazines[class_idx].loaded.pop() {
            // SAFETY: ptr a été alloué par le SLUB correspondant.
            unsafe { SLUB_CACHES[slub_idx].free(ptr); }
        }
        // Vide prev.
        while let Some(ptr) = self.magazines[class_idx].prev.pop() {
            // SAFETY: ptr alloqué par SLUB_CACHES[slub_idx] (magazine prev).
            unsafe { SLUB_CACHES[slub_idx].free(ptr); }
        }
        self.stats.drains.fetch_add(1, Ordering::Relaxed);
    }

    /// Drain toutes les classes (context switch complet).
    pub fn drain_all(&mut self) {
        for i in 0..CACHED_SIZE_CLASSES {
            self.drain_class(i);
        }
    }

    /// Nombre total d'objets en cache sur ce CPU.
    pub fn total_cached(&self) -> usize {
        self.magazines.iter().map(|m| m.cached_count()).sum()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TABLE GLOBALE DES CACHES PER-CPU
// ─────────────────────────────────────────────────────────────────────────────

/// Table globale : un cache par CPU.
/// L'accès est protégé par le fait qu'un seul CPU accède à son propre cache.
/// On utilise un `Mutex<()>` uniquement pour l'initialisation (une seule fois).
#[allow(dead_code)]
pub struct PerCpuCacheTable {
    caches: [PerCpuCache; MAX_CPUS],
    init_lock: spin::Mutex<bool>,
}

// SAFETY: L'accès à `caches[cpu_id]` est exclusif à cpu_id (pas de partage inter-CPU).
unsafe impl Sync for PerCpuCacheTable {}
unsafe impl Send for PerCpuCacheTable {}

impl PerCpuCacheTable {
    const fn new() -> Self {
        // Initialise les MAX_CPUS caches statiquement.
        // Rust ne permet pas encore `[expr; MAX_CPUS]` avec des types non-Copy complexes,
        // on utilise une macro répétitive.
        PerCpuCacheTable {
            caches: {
                // SAFETY: La structure est entièrement initialisée par const fn.
                // On va initialement mettre tous les champs à zéro via transmute d'un [u8] n'est
                // pas disponible en const. On utilise une approche de copie de new_uninit.
                //
                // En Rust stable, on ne peut pas [PerCpuCache::new_uninit(); 256] si PerCpuCache
                // n'est pas Copy. On l'initialise via MaybeUninit à l'init runtime.
                // Pour contourner ce problème en const, on encode les MAX_CPUS entrées explicitement
                // en déléguant à un helper qui retourne le tableau.
                //
                // Astuce: const { ... } + unsafe transmute d'un tableau de zéros.
                // SAFETY: Tous les AtomicXxx et primitifs s'initialisent proprement avec 0.
                // `active: false` = 0u8 ✓, `cpu_id: 0` ✓, tous les AtomicU64 = 0 ✓.
                // @rustc: `core::mem::MaybeUninit::zeroed().assume_init()` n'est pas const stable.
                // On utilise donc une initialisation runtime déportée dans `init_cpu()`.
                //
                // SAFETY: zeros valides pour AtomicXxx et primitifs; init_cpu() surcharge ensuite.
                unsafe { core::mem::MaybeUninit::zeroed().assume_init() }
            },
            init_lock: spin::Mutex::new(false),
        }
    }

    /// Initialise le cache pour un CPU donné. Doit être appelé une seule fois par CPU.
    ///
    /// # Safety
    /// Appelé depuis le code d'initialisation du CPU avant toute allocation.
    pub unsafe fn init_cpu(&self, cpu_id: usize) {
        assert!(cpu_id < MAX_CPUS, "vmalloc: cpu_id {} >= MAX_CPUS {}", cpu_id, MAX_CPUS);
        // SAFETY: chaque CPU accède uniquement à son propre slot.
        let cache_ptr = &self.caches[cpu_id] as *const PerCpuCache as *mut PerCpuCache;
        (*cache_ptr).init(cpu_id as u32);
    }

    /// Accès mutable au cache du CPU courant.
    ///
    /// # Safety
    /// `cpu_id` doit correspondre au CPU courant. Pas de préemption pendant l'accès.
    #[inline]
    pub unsafe fn get_mut(&self, cpu_id: usize) -> &mut PerCpuCache {
        let cache_ptr = core::ptr::addr_of!(self.caches[cpu_id]) as *mut PerCpuCache;
        &mut *cache_ptr
    }
}

/// Table globale des caches per-CPU.
pub static CPU_CACHES: PerCpuCacheTable = PerCpuCacheTable::new();

// ─────────────────────────────────────────────────────────────────────────────
// API PUBLIQUE SIMPLIFIÉE
// ─────────────────────────────────────────────────────────────────────────────

/// Chemin complet d'allocation avec cache per-CPU.
///
/// 1. Tente le cache (fast path — sans lock).
/// 2. Si raté, refill depuis SLUB (slow path).
/// 3. Si toujours rien, remonte l'erreur OOM.
///
/// # Safety
/// `cpu_id` doit être le CPU courant, sans préemption pendant l'appel.
pub unsafe fn cache_alloc(size: usize, cpu_id: usize) -> Option<NonNull<u8>> {
    let cache = CPU_CACHES.get_mut(cpu_id);
    if let Some(ptr) = cache.alloc_fast(size) { return Some(ptr); }

    // Slow path: refill.
    let class_idx = heap_size_class_for(size)?;
    cache.alloc_slow(size, class_idx)
}

/// Chemin complet de libération avec cache per-CPU.
///
/// 1. Tente de stocker dans le cache (fast path).
/// 2. Si plein, drains la moitié vers le SLUB, puis réessaie.
///
/// # Safety
/// `cpu_id` doit être le CPU courant, sans préemption pendant l'appel.
pub unsafe fn cache_free(ptr: NonNull<u8>, size: usize, cpu_id: usize) {
    let cache = CPU_CACHES.get_mut(cpu_id);
    if cache.free_fast(ptr, size) { return; }

    // Slow path: drain la classe concernée puis on libère directement via SLUB.
    if let Some(class_idx) = heap_size_class_for(size) {
        if class_idx < CACHED_SIZE_CLASSES {
            cache.drain_class(class_idx);
            // Après drain, réessaie.
            if cache.free_fast(ptr, size) { return; }
        }
    }
    // Dernier recours: libère directement via SLUB.
    if let Some(class_idx) = heap_size_class_for(size) {
        let slub_idx = HEAP_SIZE_CLASSES[class_idx].slab_idx as usize;
        SLUB_CACHES[slub_idx].free(ptr);
    }
}
