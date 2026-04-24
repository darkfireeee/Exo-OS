// kernel/src/memory/cow/tracker.rs
//
// Traqueur CoW (Copy-on-Write) — maintient le comptage de référence
// des frames partagés entre processus (fork/mmap shared).
// COUCHE 0 — aucune dépendance externe.

use crate::memory::core::types::Frame;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

// ─────────────────────────────────────────────────────────────────────────────
// TABLE DE COMPTAGE CoW
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre de frames trackables simultanément (puissance de 2 pour hash).
pub const COW_TABLE_SIZE: usize = 4096;
/// Masque de hash.
pub const COW_TABLE_MASK: usize = COW_TABLE_SIZE - 1;

/// Sentinel : slot jamais utilisé.
const SLOT_EMPTY: u64 = 0;
/// Sentinel : slot supprimé (tombstone). Permet de ne pas briser la chaîne
/// de sondage linéaire lors d'une suppression.
const SLOT_DELETED: u64 = u64::MAX;

/// Une entrée CoW dans la table de hachage.
#[repr(C, align(16))]
struct CowEntry {
    /// Numéro de frame (0 = slot libre, u64::MAX = tombstone supprimé).
    frame_idx: AtomicU64,
    /// Nombre de mappages de ce frame (refcount CoW).
    ref_count: AtomicU32,
    _pad: u32,
}

impl CowEntry {
    #[allow(dead_code)]
    const fn new() -> Self {
        CowEntry {
            frame_idx: AtomicU64::new(0),
            ref_count: AtomicU32::new(0),
            _pad: 0,
        }
    }
}

/// Table de hachage CoW avec sondage linéaire.
/// Stocke les frames partagés (refcount ≥ 2).
pub struct CowTracker {
    table: [CowEntry; COW_TABLE_SIZE],
    /// Verrou global protégeant inc/dec contre les races TOCTOU.
    lock: Mutex<()>,
    pub tracked_count: AtomicU64,
    pub inc_count: AtomicU64,
    pub dec_count: AtomicU64,
    pub collision_max: AtomicU32,
}

// SAFETY: CowTracker utilise un Mutex interne pour toutes les mutations.
unsafe impl Sync for CowTracker {}
unsafe impl Send for CowTracker {}

impl CowTracker {
    const fn new() -> Self {
        CowTracker {
            // SAFETY: CowEntry est zéro-initialisable.
            table: unsafe { core::mem::MaybeUninit::zeroed().assume_init() },
            lock: Mutex::new(()),
            tracked_count: AtomicU64::new(0),
            inc_count: AtomicU64::new(0),
            dec_count: AtomicU64::new(0),
            collision_max: AtomicU32::new(0),
        }
    }

    /// Hash d'un numéro de frame FNV-1a (rapide, pas de division).
    #[inline]
    fn hash(frame_idx: u64) -> usize {
        let mut h = 0xcbf29ce484222325u64;
        h ^= frame_idx;
        h = h.wrapping_mul(0x00000100000001B3);
        (h as usize) & COW_TABLE_MASK
    }

    /// Incrémente le refcount CoW d'un frame.
    /// Si le frame n'est pas dans la table, l'y insère avec refcount=2.
    pub fn inc(&self, frame: Frame) -> u32 {
        let idx = frame.phys_addr().as_u64() / 4096;
        let start = Self::hash(idx);
        let _guard = self.lock.lock();
        let mut first_tombstone: Option<usize> = None;
        let mut collisions = 0u32;

        for probe in 0..COW_TABLE_SIZE {
            let slot = (start + probe) & COW_TABLE_MASK;
            let entry = &self.table[slot];
            let existing = entry.frame_idx.load(Ordering::Relaxed);

            if existing == idx {
                // Slot trouvé — incrémente sous verrou.
                let rc = entry.ref_count.fetch_add(1, Ordering::Relaxed) + 1;
                self.inc_count.fetch_add(1, Ordering::Relaxed);
                return rc;
            }
            if existing == SLOT_EMPTY {
                // Fin de chaîne — insérer dans le premier tombstone disponible
                // ou dans ce slot vide.
                let insert_slot = first_tombstone.unwrap_or(slot);
                let entry_ins = &self.table[insert_slot];
                entry_ins.frame_idx.store(idx, Ordering::Relaxed);
                entry_ins.ref_count.store(2, Ordering::Relaxed);
                self.tracked_count.fetch_add(1, Ordering::Relaxed);
                self.inc_count.fetch_add(1, Ordering::Relaxed);
                let old_max = self.collision_max.load(Ordering::Relaxed);
                if collisions > old_max {
                    self.collision_max.store(collisions, Ordering::Relaxed);
                }
                return 2;
            }
            if existing == SLOT_DELETED && first_tombstone.is_none() {
                first_tombstone = Some(slot);
            }
            collisions += 1;
        }
        // Table pleine — cas dégénéré.
        u32::MAX
    }

    /// Décrémente le refcount CoW d'un frame.
    /// Retourne le nouveau refcount (0 = frame peut être libéré/récupéré en écriture).
    pub fn dec(&self, frame: Frame) -> u32 {
        let idx = frame.phys_addr().as_u64() / 4096;
        let start = Self::hash(idx);
        let _guard = self.lock.lock();

        for probe in 0..COW_TABLE_SIZE {
            let slot = (start + probe) & COW_TABLE_MASK;
            let entry = &self.table[slot];
            let existing = entry.frame_idx.load(Ordering::Relaxed);

            if existing == SLOT_EMPTY {
                break; // Fin de chaîne réelle — frame non suivi
            }
            if existing == SLOT_DELETED {
                continue; // Tombstone — continuer le sondage
            }
            if existing == idx {
                let old = entry.ref_count.load(Ordering::Relaxed);
                if old <= 1 {
                    // Dernière référence — marquer comme tombstone (ne brise pas la chaîne).
                    entry.frame_idx.store(SLOT_DELETED, Ordering::Relaxed);
                    entry.ref_count.store(0, Ordering::Relaxed);
                    self.tracked_count.fetch_sub(1, Ordering::Relaxed);
                    self.dec_count.fetch_add(1, Ordering::Relaxed);
                    return 0;
                }
                let new_rc = entry.ref_count.fetch_sub(1, Ordering::Relaxed) - 1;
                if new_rc == 0 {
                    // Passage à zéro : tombstone (cas concurrent sous verrou).
                    entry.frame_idx.store(SLOT_DELETED, Ordering::Relaxed);
                    self.tracked_count.fetch_sub(1, Ordering::Relaxed);
                }
                self.dec_count.fetch_add(1, Ordering::Relaxed);
                return new_rc;
            }
        }
        0 // Non trouvé → considéré comme déjà libéré
    }

    /// Retourne le refcount actuel sans modifier.
    pub fn ref_count(&self, frame: Frame) -> u32 {
        let idx = frame.phys_addr().as_u64() / 4096;
        let start = Self::hash(idx);
        // Lecture seule : pas besoin du verrou (les tombstones ne mentent pas).
        for probe in 0..COW_TABLE_SIZE {
            let slot = (start + probe) & COW_TABLE_MASK;
            let entry = &self.table[slot];
            let existing = entry.frame_idx.load(Ordering::Acquire);
            if existing == SLOT_EMPTY {
                return 1;
            }
            if existing == SLOT_DELETED {
                continue;
            }
            if existing == idx {
                return entry.ref_count.load(Ordering::Acquire);
            }
        }
        1
    }

    /// Indique si un frame est partagé (refcount ≥ 2).
    #[inline]
    pub fn is_shared(&self, frame: Frame) -> bool {
        self.ref_count(frame) >= 2
    }
}

pub static COW_TRACKER: CowTracker = CowTracker::new();
