// kernel/src/memory/heap/thread_local/magazine.rs
//
// Magazine cache per-CPU — tampon de frames pré-alloués pour éviter les locks.
// Inspiré du magazine protocol de Solaris/Bonwick.
// Couche 0 — aucune dépendance externe sauf `spin`.

use core::ptr::NonNull;

/// Taille d'un magazine (objets en cache par CPU).
pub const MAGAZINE_SIZE: usize = 64;

/// Un magazine = tableau de pointeurs vers des objets libres pré-alloués.
///
/// Deux magazines par CPU : un "full" (prêt à distribuer) et un "empty"
/// (prêt à être rempli). Le swap entre les deux est atomique O(1).
#[repr(C, align(64))]
pub struct Magazine {
    /// Tableau d'objets libres (index 0..count-1 sont valides).
    objects: [*mut u8; MAGAZINE_SIZE],
    /// Nombre d'objets actuellement dans le magazine.
    count:   usize,
    /// Classe de taille de ce magazine.
    pub size_class: usize,
}

// SAFETY: Magazine est utilisé par un seul CPU à la fois (per-CPU).
unsafe impl Send for Magazine {}
unsafe impl Sync for Magazine {}

impl Magazine {
    pub const fn new(size_class: usize) -> Self {
        Magazine {
            objects:    [core::ptr::null_mut(); MAGAZINE_SIZE],
            count:      0,
            size_class,
        }
    }

    /// Retire un objet du magazine (O(1)).
    /// Retourne `None` si le magazine est vide.
    #[inline]
    pub fn pop(&mut self) -> Option<NonNull<u8>> {
        if self.count == 0 { return None; }
        self.count -= 1;
        NonNull::new(self.objects[self.count])
    }

    /// Pousse un objet dans le magazine (O(1)).
    /// Retourne `false` si le magazine est plein.
    #[inline]
    pub fn push(&mut self, ptr: NonNull<u8>) -> bool {
        if self.count >= MAGAZINE_SIZE { return false; }
        self.objects[self.count] = ptr.as_ptr();
        self.count += 1;
        true
    }

    #[inline] pub fn is_empty(&self) -> bool { self.count == 0 }
    #[inline] pub fn is_full(&self)  -> bool { self.count >= MAGAZINE_SIZE }
    #[inline] pub fn len(&self)      -> usize { self.count }
    #[inline] pub fn capacity(&self) -> usize { MAGAZINE_SIZE }
}

// ─────────────────────────────────────────────────────────────────────────────
// PAIRE DE MAGAZINES PAR CPU
// ─────────────────────────────────────────────────────────────────────────────

/// Paire de magazines per-CPU pour une classe de taille.
///
/// Protocol :
/// 1. Alloc : pop() sur `loaded`. Si vide, swap loaded↔prev et pop sur nouveau loaded.
///    Si toujours vide, aller au dépôt central.
/// 2. Free  : push() sur `loaded`. Si plein, swap loaded↔prev et push sur nouveau loaded.
///    Si toujours plein, rendre prev au dépôt et allouer un nouveau.
pub struct CpuMagazinePair {
    pub loaded: Magazine,
    pub prev:   Magazine,
}

impl CpuMagazinePair {
    pub const fn new(size_class: usize) -> Self {
        CpuMagazinePair {
            loaded: Magazine::new(size_class),
            prev:   Magazine::new(size_class),
        }
    }

    /// Alloue un objet via le magazine.
    /// Retourne `None` si les deux magazines sont vides (refill nécessaire).
    #[inline]
    pub fn alloc(&mut self) -> Option<NonNull<u8>> {
        if let Some(p) = self.loaded.pop() { return Some(p); }
        // Swap et réessayer
        core::mem::swap(&mut self.loaded, &mut self.prev);
        self.loaded.pop()
    }

    /// Libère un objet via le magazine.
    /// Retourne `false` si les deux magazines sont pleins (drain nécessaire).
    #[inline]
    pub fn free(&mut self, ptr: NonNull<u8>) -> bool {
        if self.loaded.push(ptr) { return true; }
        // Swap et réessayer
        core::mem::swap(&mut self.loaded, &mut self.prev);
        self.loaded.push(ptr)
    }

    /// Retourne le nombre total d'objets en cache sur ce CPU.
    #[inline]
    pub fn cached_count(&self) -> usize {
        self.loaded.len() + self.prev.len()
    }
}
