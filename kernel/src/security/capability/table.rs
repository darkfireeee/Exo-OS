// kernel/src/security/capability/table.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// CAP TABLE — Table de capacités par processus (Exo-OS Security · Couche 2b)
// ═══════════════════════════════════════════════════════════════════════════════
//
// ⚠️  HORS périmètre de vérification formelle (proptest + INVARIANTS.md uniquement).
//     table.rs est l'implémentation concrète — non incluse dans la preuve.
//     Le périmètre formel couvre : model.rs, token.rs, rights.rs,
//     revocation.rs, delegation.rs (voir DOC7, RÈGLE CAP-02).
//
// STRUCTURE :
//   Chaque processus possède une CapTable correspondant à une map ObjectId→Entry.
//   L'implémentation utilise un tableau statique de CAP_TABLE_CAPACITY entrées
//   avec un spinlock global — pas d'allocation heap dans le hot path de vérification.
//
// PROPRIÉTÉS :
//   • Insertion : O(1) amortie (scan linéaire des slots libres)
//   • Vérification : O(1) — accès direct par index haché
//   • Révocation : O(1) — incrément atomique de génération
//   • Thread-safety : Spinlock sur mutations, lecture atomique de génération
//
// ESPACE MÉMOIRE RÉSERVÉ :
//   CAP_TABLE_CAPACITY × size_of::<CapEntry>() = 512 × 24 = 12 288 bytes ≈ 12 KiB
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

use super::rights::Rights;
use super::token::{CapObjectType, CapToken, ObjectId};
use super::verify::CapError;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Capacité maximale d'entrées par CapTable (par processus).
/// 512 capacités simultanées — suffisant pour un processus complexe.
pub const CAP_TABLE_CAPACITY: usize = 512;

/// Marqueur de slot vide.
const SLOT_FREE: u64 = u64::MAX;

// ─────────────────────────────────────────────────────────────────────────────
// CapEntry — entrée interne de la table
// ─────────────────────────────────────────────────────────────────────────────

/// Entrée interne dans la CapTable.
/// Taille : 8+4+4+4+4 = 24 bytes, alignée sur 8.
#[repr(C)]
struct CapEntry {
    /// Identifiant de l'objet — SLOT_FREE si libre.
    object_id: AtomicU64,
    /// Droits accordés pour cet objet.
    rights: AtomicU32,
    /// Génération courante — incrémentée lors d'une révocation.
    generation: AtomicU32,
    /// Type de l'objet — stocké pour validation rapide.
    type_tag: AtomicU32,
    /// Padding pour alignement cache-line-friendly.
    _pad: AtomicU32,
}

impl CapEntry {
    const fn new_free() -> Self {
        Self {
            object_id: AtomicU64::new(SLOT_FREE),
            rights: AtomicU32::new(0),
            generation: AtomicU32::new(0),
            type_tag: AtomicU32::new(0),
            _pad: AtomicU32::new(0),
        }
    }

    #[inline(always)]
    fn is_free(&self) -> bool {
        self.object_id.load(Ordering::Acquire) == SLOT_FREE
    }

    /// Retourne (rights, generation, type_tag) si l'ObjectId correspond.
    #[inline(always)]
    #[allow(dead_code)]
    fn load_for_verify(&self, expected_oid: ObjectId) -> Option<(Rights, u32, CapObjectType)> {
        let oid = self.object_id.load(Ordering::Acquire);
        if oid != expected_oid.0 {
            return None;
        }
        let r = Rights::from_bits_truncate(self.rights.load(Ordering::Acquire));
        let gen = self.generation.load(Ordering::Acquire);
        let tt = CapObjectType::from_u16(self.type_tag.load(Ordering::Acquire) as u16);
        Some((r, gen, tt))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CapTable — structure principale
// ─────────────────────────────────────────────────────────────────────────────

/// Table de capacités d'un processus.
///
/// # Sécurité processus
/// Chaque processus possède sa propre CapTable — pas de partage direct.
/// L'accès en écriture est protégé par un Mutex; la vérification (lecture atomique)
/// ne prend pas le lock en condition normale.
pub struct CapTable {
    /// Tableau fixe d'entrées — pas d'allocation heap.
    entries: [CapEntry; CAP_TABLE_CAPACITY],
    /// Mutex pour les mutations (grant, revoke, inherit).
    /// Pas pris sur le chemin verify() sauf fallback.
    write_lock: Mutex<()>,
    /// Nombre d'entrées actuellement occupées.
    count: AtomicU32,
    /// Statistiques.
    stats: CapTableStats,
}

/// Statistiques par table.
struct CapTableStats {
    grants: AtomicU64,
    revocations: AtomicU64,
    lookups: AtomicU64,
    misses: AtomicU64,
}

impl CapTableStats {
    const fn new() -> Self {
        Self {
            grants: AtomicU64::new(0),
            revocations: AtomicU64::new(0),
            lookups: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }
}

/// Snapshot de statistiques CapTable.
#[derive(Debug, Clone, Copy)]
pub struct CapTableSnapshot {
    pub grants: u64,
    pub revocations: u64,
    pub lookups: u64,
    pub misses: u64,
    pub count: u32,
}

// ─────────────────────────────────────────────────────────────────────────────
// Implémentation CapTable
// ─────────────────────────────────────────────────────────────────────────────

// SAFETY: CapTable contient uniquement des types atomiques + Mutex.
// Les accès concurrents sont correctement synchronisés.
unsafe impl Send for CapTable {}
unsafe impl Sync for CapTable {}

// Macro pour créer un tableau [CapEntry; N] avec des entrées libres.
// Nécessaire car CapEntry n'est pas Copy.
macro_rules! init_cap_entries {
    () => {{
        let mut arr: [core::mem::MaybeUninit<CapEntry>; CAP_TABLE_CAPACITY] =
            // SAFETY: MaybeUninit array uninit valide; chaque slot est écrasé par new_free() ensuite.
            unsafe { core::mem::MaybeUninit::uninit().assume_init() };
        let mut i = 0usize;
        while i < CAP_TABLE_CAPACITY {
            arr[i] = core::mem::MaybeUninit::new(CapEntry::new_free());
            i += 1;
        }
        // SAFETY: Tous les éléments initialisés.
        unsafe { core::mem::transmute::<_, [CapEntry; CAP_TABLE_CAPACITY]>(arr) }
    }};
}

impl CapTable {
    /// Crée une nouvelle CapTable vide.
    pub fn new() -> Self {
        Self {
            entries: init_cap_entries!(),
            write_lock: Mutex::new(()),
            count: AtomicU32::new(0),
            stats: CapTableStats::new(),
        }
    }

    // ── Hachage ObjectId → index ─────────────────────────────────────────────

    /// Hache un ObjectId en index de départ dans le tableau.
    /// Utilise FNV-1a 64-bit condensé sur 9 bits.
    #[inline(always)]
    fn hash_index(oid: ObjectId) -> usize {
        let h = oid.0.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        let h = h ^ (h >> 30);
        (h as usize) & (CAP_TABLE_CAPACITY - 1)
    }

    // ── Recherche d'un slot ──────────────────────────────────────────────────

    /// Trouve l'index de l'entrée pour un ObjectId donné (probe linéaire).
    /// Retourne None si non trouvé.
    #[inline]
    fn find_slot(&self, oid: ObjectId) -> Option<usize> {
        let start = Self::hash_index(oid);
        for probe in 0..CAP_TABLE_CAPACITY {
            let idx = (start + probe) & (CAP_TABLE_CAPACITY - 1);
            let entry = &self.entries[idx];
            let stored = entry.object_id.load(Ordering::Acquire);
            if stored == oid.0 {
                return Some(idx);
            }
            if stored == SLOT_FREE {
                // Pas de wrapped-around possible sans plus de remplissage
                return None;
            }
        }
        None
    }

    /// Trouve un slot libre — utilisé lors d'un grant().
    #[inline]
    fn find_free_slot(&self, oid: ObjectId) -> Option<usize> {
        let start = Self::hash_index(oid);
        for probe in 0..CAP_TABLE_CAPACITY {
            let idx = (start + probe) & (CAP_TABLE_CAPACITY - 1);
            if self.entries[idx].is_free() {
                return Some(idx);
            }
            // Si même ObjectId déjà présent, retourner cet index (mise à jour)
            if self.entries[idx].object_id.load(Ordering::Acquire) == oid.0 {
                return Some(idx);
            }
        }
        None
    }

    // ── API publique ─────────────────────────────────────────────────────────

    /// Ajoute une entrée dans la table et retourne le CapToken correspondant.
    ///
    /// # Erreurs
    /// - `CapError::TableFull` si la table est saturée.
    ///
    /// # Thread-safety
    /// Acquiert le mutex d'écriture.
    pub fn grant(
        &self,
        object_id: ObjectId,
        rights: Rights,
        type_tag: CapObjectType,
    ) -> Result<CapToken, CapError> {
        let _guard = self.write_lock.lock();

        let idx = self.find_free_slot(object_id).ok_or(CapError::TableFull)?;
        let entry = &self.entries[idx];

        let gen = if entry.is_free() {
            // Nouveau slot — génération 0
            entry.rights.store(rights.bits(), Ordering::Release);
            entry.type_tag.store(type_tag as u32, Ordering::Release);
            entry.generation.store(0, Ordering::Release);
            // Publication atomique de l'ObjectId — doit être LAST
            entry.object_id.store(object_id.0, Ordering::Release);
            self.count.fetch_add(1, Ordering::Relaxed);
            0
        } else {
            // Mise à jour d'un slot existant (re-grant après révocation)
            let current_gen = entry.generation.load(Ordering::Acquire);
            entry.rights.store(rights.bits(), Ordering::Release);
            entry.type_tag.store(type_tag as u32, Ordering::Release);
            current_gen
        };

        self.stats.grants.fetch_add(1, Ordering::Relaxed);
        Ok(CapToken::new(object_id, rights, gen, type_tag))
    }

    /// Révoque tous les tokens pour un ObjectId donné.
    ///
    /// La révocation est O(1) : on incrémente uniquement le compteur de génération.
    /// Les tokens existants avec l'ancienne génération retourneront Err(Revoked).
    ///
    /// # PROPRIÉTÉ VÉRIFIÉE (proptest + INVARIANTS.md — LAC-02)
    /// ∀ token t, revoke(obj) → verify(t) = Err(Denied)
    pub fn revoke(&self, object_id: ObjectId) -> Result<(), CapError> {
        if let Some(idx) = self.find_slot(object_id) {
            // Incrément atomique de génération — invalide TOUS les tokens existants
            self.entries[idx].generation.fetch_add(1, Ordering::Release);
            self.stats.revocations.fetch_add(1, Ordering::Relaxed);
            Ok(())
        } else {
            Err(CapError::ObjectNotFound)
        }
    }

    /// Supprime complètement une entrée (libère le slot).
    /// Utilisé lors de la destruction d'un objet.
    pub fn remove(&self, object_id: ObjectId) -> Result<(), CapError> {
        let _guard = self.write_lock.lock();
        if let Some(idx) = self.find_slot(object_id) {
            // On révoque d'abord (incrément génération)
            self.entries[idx].generation.fetch_add(1, Ordering::Release);
            // Puis on libère le slot en marquant comme libre
            self.entries[idx]
                .object_id
                .store(SLOT_FREE, Ordering::Release);
            self.entries[idx].rights.store(0, Ordering::Release);
            self.count.fetch_sub(1, Ordering::Relaxed);
            Ok(())
        } else {
            Err(CapError::ObjectNotFound)
        }
    }

    /// Incrémente directement le compteur de génération d'un objet.
    /// Utilisé par `revocation::revoke()` interne.
    #[inline]
    pub(super) fn increment_generation(&self, object_id: ObjectId, ord: Ordering) {
        if let Some(idx) = self.find_slot(object_id) {
            self.entries[idx].generation.fetch_add(1, ord);
        }
    }

    /// Charge l'entrée pour un ObjectId donné — utilisée par `revocation::verify()`.
    #[inline]
    pub(super) fn get(&self, object_id: ObjectId) -> Option<CapEntryView> {
        self.stats.lookups.fetch_add(1, Ordering::Relaxed);
        if let Some(idx) = self.find_slot(object_id) {
            let entry = &self.entries[idx];
            let rights = Rights::from_bits_truncate(entry.rights.load(Ordering::Acquire));
            let gen = entry.generation.load(Ordering::Acquire);
            let tt = CapObjectType::from_u16(entry.type_tag.load(Ordering::Acquire) as u16);
            Some(CapEntryView {
                rights,
                generation: gen,
                type_tag: tt,
            })
        } else {
            self.stats.misses.fetch_add(1, Ordering::Relaxed);
            None
        }
    }

    /// Vérifie si l'objet existe dans la table.
    #[inline]
    pub fn contains(&self, object_id: ObjectId) -> bool {
        self.find_slot(object_id).is_some()
    }

    /// Retourne un snapshot de stats.
    pub fn stats(&self) -> CapTableSnapshot {
        CapTableSnapshot {
            grants: self.stats.grants.load(Ordering::Relaxed),
            revocations: self.stats.revocations.load(Ordering::Relaxed),
            lookups: self.stats.lookups.load(Ordering::Relaxed),
            misses: self.stats.misses.load(Ordering::Relaxed),
            count: self.count.load(Ordering::Relaxed),
        }
    }

    /// Nombre d'entrées actives.
    #[inline]
    pub fn len(&self) -> usize {
        self.count.load(Ordering::Relaxed) as usize
    }

    /// Vrai si la table est vide.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for CapTable {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CapEntryView — vue lecture
// ─────────────────────────────────────────────────────────────────────────────

/// Vue en lecture d'une entrée — retournée par `get()`.
#[derive(Debug, Clone, Copy)]
pub struct CapEntryView {
    pub rights: Rights,
    pub generation: u32,
    pub type_tag: CapObjectType,
}
