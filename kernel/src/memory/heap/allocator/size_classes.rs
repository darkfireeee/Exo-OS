// kernel/src/memory/heap/allocator/size_classes.rs
//
// Classes de taille pour l'allocateur heap hybride.
// Couvre 1 octet jusqu'à 2 KiB (au-delà → large allocator).
// Couche 0 — aucune dépendance externe sauf `spin`.

// ─────────────────────────────────────────────────────────────────────────────
// CLASSES DE TAILLE
// ─────────────────────────────────────────────────────────────────────────────

/// Une entrée dans la table de classes de taille.
#[derive(Debug, Clone, Copy)]
pub struct HeapSizeClass {
    /// Taille maximale en octets pour cette classe.
    pub max_size: usize,
    /// Alignement (toujours une puissance de 2).
    pub align:    usize,
    /// Index du cache slab/slub correspondant.
    pub slab_idx: usize,
}

/// Table de classes de taille du heap hybride.
/// Tailles : 8, 16, 24, 32, 48, 64, 96, 128, 192, 256, 384, 512, 768, 1024, 1536, 2048
pub const HEAP_SIZE_CLASSES: &[HeapSizeClass] = &[
    HeapSizeClass { max_size:    8, align:  8, slab_idx: 0 },
    HeapSizeClass { max_size:   16, align: 16, slab_idx: 1 },
    HeapSizeClass { max_size:   24, align:  8, slab_idx: 2 }, // → classe 32 slab
    HeapSizeClass { max_size:   32, align: 16, slab_idx: 2 },
    HeapSizeClass { max_size:   48, align: 16, slab_idx: 3 }, // → classe 64 slab
    HeapSizeClass { max_size:   64, align: 64, slab_idx: 3 },
    HeapSizeClass { max_size:   96, align: 64, slab_idx: 4 }, // → classe 128 slab
    HeapSizeClass { max_size:  128, align: 64, slab_idx: 4 },
    HeapSizeClass { max_size:  192, align: 64, slab_idx: 5 }, // → classe 256 slab
    HeapSizeClass { max_size:  256, align: 64, slab_idx: 5 },
    HeapSizeClass { max_size:  384, align: 64, slab_idx: 6 }, // → classe 512 slab
    HeapSizeClass { max_size:  512, align: 64, slab_idx: 6 },
    HeapSizeClass { max_size:  768, align: 64, slab_idx: 7 }, // → classe 1024 slab
    HeapSizeClass { max_size: 1024, align: 64, slab_idx: 7 },
    HeapSizeClass { max_size: 1536, align: 64, slab_idx: 8 }, // → classe 2048 slab
    HeapSizeClass { max_size: 2048, align: 64, slab_idx: 8 },
];

/// Seuil en octets au-dessus duquel on utilise le large allocator.
pub const HEAP_LARGE_THRESHOLD: usize = 2048;

/// Nombre de classes de taille disponibles.
pub const NUM_HEAP_SIZE_CLASSES: usize = 16;

/// Retourne l'index de la classe de taille pour `size` octets.
/// Retourne `None` si `size > HEAP_LARGE_THRESHOLD`.
#[inline]
pub fn heap_size_class_for(size: usize) -> Option<usize> {
    if size == 0 { return Some(0); } // Alloue 8 octets minimum
    for (i, sc) in HEAP_SIZE_CLASSES.iter().enumerate() {
        if size <= sc.max_size {
            return Some(i);
        }
    }
    None
}

/// Retourne la taille réelle allouée pour `size` (arrondie à la classe supérieure).
#[inline]
pub fn heap_alloc_size(size: usize) -> usize {
    if size == 0 { return HEAP_SIZE_CLASSES[0].max_size; }
    for sc in HEAP_SIZE_CLASSES {
        if size <= sc.max_size { return sc.max_size; }
    }
    // Large allocation : arrondir à PAGE_SIZE
    (size + crate::memory::core::PAGE_SIZE - 1) & !(crate::memory::core::PAGE_SIZE - 1)
}

/// Retourne l'alignement requis pour `size`.
#[inline]
pub fn heap_align_for(size: usize) -> usize {
    for sc in HEAP_SIZE_CLASSES {
        if size <= sc.max_size { return sc.align; }
    }
    crate::memory::core::PAGE_SIZE
}
