// kernel/src/memory/physical/frame/ref_count.rs
//
// Refcount atomique CoW pour les frames physiques.
// Gère les scénarios de partage mémoire (fork(), mmap(), SHM).
// Couche 0 — aucune dépendance externe.

use core::sync::atomic::{AtomicU32, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// ATOMIC REFCOUNT
// ─────────────────────────────────────────────────────────────────────────────

/// Compteur de références atomique pour frame physique.
///
/// Protocole :
///   - 0 = frame libre (aucun propriétaire)
///   - 1 = frame alloué exclusivement (accessible en lecture/écriture)
///   - 2+ = frame partagé CoW (lecture seule, copie à l'écriture)
///
/// Thread-safety : toutes les opérations sont atomiques et sans verrou.
#[repr(transparent)]
pub struct AtomicRefCount(AtomicU32);

impl AtomicRefCount {
    /// Crée un refcount à zéro (frame libre).
    #[inline(always)]
    pub const fn new_zero() -> Self {
        AtomicRefCount(AtomicU32::new(0))
    }

    /// Crée un refcount à 1 (frame alloué exclusivement).
    #[inline(always)]
    pub const fn new_one() -> Self {
        AtomicRefCount(AtomicU32::new(1))
    }

    /// Retourne la valeur courante (snapshot ; peut changer ensuite).
    #[inline(always)]
    pub fn get(&self) -> u32 {
        self.0.load(Ordering::Relaxed)
    }

    /// Vérifie si le frame est libre (refcount == 0).
    #[inline(always)]
    pub fn is_zero(&self) -> bool {
        self.0.load(Ordering::Acquire) == 0
    }

    /// Vérifie si le frame est exclusif (refcount == 1).
    #[inline(always)]
    pub fn is_exclusive(&self) -> bool {
        self.0.load(Ordering::Acquire) == 1
    }

    /// Vérifie si le frame est partagé (refcount >= 2).
    #[inline(always)]
    pub fn is_shared(&self) -> bool {
        self.0.load(Ordering::Acquire) >= 2
    }

    /// Incrémente le refcount, retourne la nouvelle valeur.
    /// Ne peut pas déborder (saturation à u32::MAX, panique en debug).
    #[inline(always)]
    pub fn inc(&self) -> u32 {
        let prev = self.0.fetch_add(1, Ordering::Relaxed);
        debug_assert_ne!(prev, u32::MAX, "AtomicRefCount overflow");
        prev + 1
    }

    /// Décrémente le refcount.
    /// Retourne `RefCountDecResult` indiquant ce qu'il faut faire.
    ///
    /// # Double-free guard (release + debug)
    /// Si le refcount est déjà 0, stoppe la décrémentation (restore 0) et
    /// retourne `StillShared` pour éviter une libération incorrecte.
    #[inline]
    pub fn dec(&self) -> RefCountDecResult {
        let prev = self.0.fetch_sub(1, Ordering::AcqRel);
        if prev == 0 {
            // Annuler le wrap vers u32::MAX
            self.0.store(0, Ordering::Release);
            debug_assert!(false, "AtomicRefCount underflow — double-free détecté");
            return RefCountDecResult::StillShared;
        }

        match prev {
            1 => RefCountDecResult::ShouldFree,
            2 => RefCountDecResult::BecameExclusive,
            _ => RefCountDecResult::StillShared,
        }
    }

    /// Tente d'incrémenter le refcount uniquement si non-nul (frame vivant).
    /// Pattern "get-or-create" : utile pour les lookups dans le frame table.
    /// Retourne `false` si le frame était libre (refcount == 0).
    #[inline]
    pub fn try_inc_if_nonzero(&self) -> bool {
        let mut current = self.0.load(Ordering::Relaxed);
        loop {
            if current == 0 {
                return false;
            }
            match self.0.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(new) => current = new,
            }
        }
    }

    /// Fixe le refcount à 0 (libération forcée — boot/reinit uniquement).
    /// SAFETY: Aucun autre thread ne doit utiliser ce frame au moment de l'appel.
    #[inline(always)]
    pub unsafe fn reset_to_zero(&self) {
        self.0.store(0, Ordering::Release);
    }

    /// Fixe le refcount à 1 (allocation initiale — boot/reinit uniquement).
    /// SAFETY: Comme reset_to_zero.
    #[inline(always)]
    pub unsafe fn reset_to_one(&self) {
        self.0.store(1, Ordering::Release);
    }

    /// Tente un CAS pour passer de `expected` à `new`.
    /// Retourne Ok(()) si réussi, Err(current) sinon.
    #[inline]
    pub fn compare_exchange(&self, expected: u32, new: u32) -> Result<(), u32> {
        self.0
            .compare_exchange(expected, new, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|cur| cur)
    }
}

/// Résultat d'un décrément de refcount.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum RefCountDecResult {
    /// Le refcount est tombé à 0 — le frame doit être libéré (retourné au buddy).
    ShouldFree,
    /// Le refcount est tombé à 1 — le dernier propriétaire peut écrire sans CoW.
    BecameExclusive,
    /// Le refcount est encore >= 2 — le frame reste partagé (CoW toujours actif).
    StillShared,
}

// ─────────────────────────────────────────────────────────────────────────────
// COW SLOT — entrée dans la CoW table
// ─────────────────────────────────────────────────────────────────────────────

/// Entrée du tracking CoW pour un frame.
/// Permet de savoir combien de processus partagent ce frame en lecture.
#[repr(C, align(16))]
pub struct CowSlot {
    /// PFN du frame partagé.
    pub pfn: u64,
    /// Refcount CoW (copies attendues).
    pub refcount: AtomicRefCount,
    /// Padding pour aligner à 16 bytes.
    _pad: u32,
}

const _: () = assert!(
    core::mem::size_of::<CowSlot>() == 16,
    "CowSlot doit faire 16 bytes"
);

impl CowSlot {
    #[inline]
    pub const fn new(pfn: u64) -> Self {
        CowSlot {
            pfn,
            refcount: AtomicRefCount::new_one(),
            _pad: 0,
        }
    }

    /// Vérifie si ce slot est libre (pfn == 0).
    #[inline(always)]
    pub fn is_free(&self) -> bool {
        self.pfn == 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// COW BREAK — logique de copie-on-write
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat d'un CoW break.
#[derive(Debug)]
pub enum CowBreakResult {
    /// La page était exclusive — pas de copie nécessaire, écriture directe OK.
    AlreadyExclusive,
    /// CoW break réussi — nouvelle adresse physique (copie du frame original).
    Copied { new_frame_pfn: u64 },
    /// Échec : plus de mémoire pour la copie.
    OutOfMemory,
}

/// Vérifie si un frame CoW peut être promu en exclusif sans copie.
/// Condition : refcount == 1 après décrément de la référence partagée.
///
/// Utilisation : lors d'un page fault CoW en Ring 3, le handler de faute
/// de page appelle cette fonction avant de décider si copier ou pas.
#[inline]
pub fn cow_can_promote(refcount: &AtomicRefCount) -> bool {
    // Si le refcount est déjà 1, ce thread est le seul détenteur
    refcount.is_exclusive()
}
