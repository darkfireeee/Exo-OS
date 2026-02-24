// kernel/src/memory/physical/frame/descriptor.rs
//
// FrameDesc — descripteur d'un frame physique.
// Chaque frame physique dans le système a exactement un FrameDesc associé
// dans le tableau global `FRAME_TABLE`.
// Couche 0 — aucune dépendance externe.

use core::sync::atomic::{AtomicU32, AtomicU8, AtomicU16, Ordering};
use crate::memory::core::types::ZoneType;

// ─────────────────────────────────────────────────────────────────────────────
// FRAME FLAGS — état et propriétés d'un frame
// ─────────────────────────────────────────────────────────────────────────────

/// Drapeaux d'état d'un frame physique.
/// Représentés sur 16 bits pour tenir dans FrameDesc.
#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct FrameFlags(u16);

impl FrameFlags {
    /// Frame libre (non alloué).
    pub const FREE:         FrameFlags = FrameFlags(1 << 0);
    /// Frame alloué par le kernel.
    pub const KERNEL:       FrameFlags = FrameFlags(1 << 1);
    /// Frame alloué pour userspace.
    pub const USER:         FrameFlags = FrameFlags(1 << 2);
    /// Frame utilisé pour DMA (no-cache, no-swap, no-CoW).
    pub const DMA:          FrameFlags = FrameFlags(1 << 3);
    /// Frame verrouillé en RAM (ne pas swapper/migrer).
    pub const PINNED:       FrameFlags = FrameFlags(1 << 4);
    /// Frame Copy-on-Write (partagé entre processus, copie à l'écriture).
    pub const COW:          FrameFlags = FrameFlags(1 << 5);
    /// Frame sale (modifié depuis la dernière synchronisation disque).
    pub const DIRTY:        FrameFlags = FrameFlags(1 << 6);
    /// Frame accédé récemment (LRU clock bit).
    pub const ACCESSED:     FrameFlags = FrameFlags(1 << 7);
    /// Frame appartenant à une huge page (2 MiB).
    pub const HUGE_PAGE:    FrameFlags = FrameFlags(1 << 8);
    /// Frame appartenant à une page de swap.
    pub const SWAP:         FrameFlags = FrameFlags(1 << 9);
    /// Frame dans un per-CPU pool (hot cache).
    pub const IN_CPU_POOL:  FrameFlags = FrameFlags(1 << 10);
    /// Frame partagé entre plusieurs processus (SHM/mmap).
    pub const SHARED:       FrameFlags = FrameFlags(1 << 11);
    /// Frame en cours de migration inter-nœuds NUMA.
    pub const MIGRATING:    FrameFlags = FrameFlags(1 << 12);
    /// Frame réservé par le firmware/BIOS (jamais alloué).
    pub const RESERVED:     FrameFlags = FrameFlags(1 << 13);
    /// Frame de la liste LRU active (LRU policy).
    pub const LRU_ACTIVE:   FrameFlags = FrameFlags(1 << 14);
    /// Frame de la liste LRU inactive.
    pub const LRU_INACTIVE: FrameFlags = FrameFlags(1 << 15);

    pub const EMPTY: FrameFlags = FrameFlags(0);

    #[inline(always)]
    pub const fn contains(self, f: FrameFlags) -> bool {
        (self.0 & f.0) == f.0
    }

    #[inline(always)]
    pub const fn set(self, f: FrameFlags) -> Self {
        FrameFlags(self.0 | f.0)
    }

    #[inline(always)]
    pub const fn clear(self, f: FrameFlags) -> Self {
        FrameFlags(self.0 & !f.0)
    }

    #[inline(always)]
    pub const fn bits(self) -> u16 { self.0 }
}

// ─────────────────────────────────────────────────────────────────────────────
// FRAME DESCRIPTOR
// ─────────────────────────────────────────────────────────────────────────────

/// Descripteur d'un frame physique.
///
/// Doit tenir dans exactement 32 bytes pour que le tableau global
/// `FRAME_TABLE[nr_frames]` reste cache-friendly (2 descripteurs par
/// cache line de 64 bytes).
///
/// Layout (32 bytes) :
///   - refcount (4 bytes, AtomicU32) : compteur de références (CoW/SHM)
///   - flags    (2 bytes, AtomicU16) : état et propriétés du frame
///   - zone     (1 byte,  u8)        : ZoneType (Dma/Dma32/Normal/High/Movable)
///   - numa_node(1 byte,  u8)        : nœud NUMA d'appartenance (0..MAX_NUMA_NODES)
///   - order    (1 byte,  AtomicU8)  : ordre buddy (0=4KiB, 9=2MiB, ...)
///              si >=BUDDY_MAX_ORDER → dans pool DMA ou réservé
///   - lru_gen  (1 byte,  AtomicU8)  : génération LRU (pour clock algorithm)
///   - mapcount (2 bytes, AtomicU16) : nombre de page tables qui mappent ce frame
///   - _pad     (20 bytes)           : alignement futur / 32 bytes total
#[repr(C, align(32))]
pub struct FrameDesc {
    /// Compteur de références atomique — 0 = frame libre.
    pub refcount:   AtomicU32,
    /// Drapeaux d'état atomiques.
    pub flags:      AtomicU16,
    /// Zone mémoire du frame.
    pub zone:       u8,
    /// Nœud NUMA d'appartenance.
    pub numa_node:  u8,
    /// Ordre buddy courant (0..11). BUDDY_MAX_ORDER+1 = hors-buddy.
    pub order:      AtomicU8,
    /// Génération LRU (clock bit, incrémenté à chaque scan).
    pub lru_gen:    AtomicU8,
    /// Nombre de mappings actifs de ce frame (pour writable/COW accounting).
    pub mapcount:   AtomicU16,
    /// Réservé pour usage futur (padding à 32 bytes).
    _pad:           [u8; 20],
}

const _: () = assert!(
    core::mem::size_of::<FrameDesc>() == 32,
    "FrameDesc doit faire exactement 32 bytes"
);
const _: () = assert!(
    core::mem::align_of::<FrameDesc>() == 32,
    "FrameDesc doit être aligné sur 32 bytes"
);

impl FrameDesc {
    /// Crée un FrameDesc libre dans la zone et le nœud NUMA indiqués.
    #[inline]
    pub const fn new_free(zone: ZoneType, numa_node: u8) -> Self {
        FrameDesc {
            refcount:  AtomicU32::new(0),
            flags:     AtomicU16::new(FrameFlags::FREE.0),
            zone:      zone as u8,
            numa_node,
            order:     AtomicU8::new(0),
            lru_gen:   AtomicU8::new(0),
            mapcount:  AtomicU16::new(0),
            _pad:      [0u8; 20],
        }
    }

    /// Retourne la zone mémoire du frame.
    #[inline(always)]
    pub fn zone_type(&self) -> ZoneType {
        ZoneType::from_index(self.zone as usize).unwrap_or(ZoneType::Normal)
    }

    /// Retourne le nœud NUMA du frame.
    #[inline(always)]
    pub fn numa_node(&self) -> u8 {
        self.numa_node
    }

    // ── Refcount ────────────────────────────────────────────────────────────

    /// Retourne le refcount actuel.
    #[inline(always)]
    pub fn refcount(&self) -> u32 {
        self.refcount.load(Ordering::Relaxed)
    }

    /// Incrémente le refcount de manière atomique.
    /// Retourne le nouveau refcount.
    #[inline(always)]
    pub fn inc_ref(&self) -> u32 {
        self.refcount.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Décrémente le refcount de manière atomique.
    /// Retourne `true` si le frame doit être libéré (refcount atteint 0).
    #[inline(always)]
    pub fn dec_ref(&self) -> bool {
        let prev = self.refcount.fetch_sub(1, Ordering::AcqRel);
        debug_assert_ne!(prev, 0, "dec_ref avec refcount déjà à 0 — double-free détecté");
        prev == 1
    }

    /// Fixe le refcount à une valeur arbitraire (boot uniquement).
    /// SAFETY: Doit être appelé uniquement pendant l'initialisation du frame table,
    /// avant que d'autres CPUs aient accès au descripteur.
    #[inline(always)]
    pub unsafe fn set_refcount_init(&self, count: u32) {
        self.refcount.store(count, Ordering::Relaxed);
    }

    // ── Flags ────────────────────────────────────────────────────────────────

    /// Retourne les flags courants.
    #[inline(always)]
    pub fn flags(&self) -> FrameFlags {
        FrameFlags(self.flags.load(Ordering::Relaxed))
    }

    /// Vérifie si un flag est présent.
    #[inline(always)]
    pub fn has_flag(&self, flag: FrameFlags) -> bool {
        self.flags().contains(flag)
    }

    /// Positionne un flag atomiquement.
    #[inline(always)]
    pub fn set_flag(&self, flag: FrameFlags) {
        self.flags.fetch_or(flag.bits(), Ordering::Relaxed);
    }

    /// Efface un flag atomiquement.
    #[inline(always)]
    pub fn clear_flag(&self, flag: FrameFlags) {
        self.flags.fetch_and(!flag.bits(), Ordering::Relaxed);
    }

    /// Échange atomiquement les flags (retourne les anciens).
    #[inline(always)]
    pub fn swap_flags(&self, new_flags: FrameFlags) -> FrameFlags {
        FrameFlags(self.flags.swap(new_flags.bits(), Ordering::AcqRel))
    }

    /// Vérifie si le frame est libre.
    #[inline(always)]
    pub fn is_free(&self) -> bool {
        self.has_flag(FrameFlags::FREE)
    }

    /// Vérifie si le frame est pinné (non-swappable).
    #[inline(always)]
    pub fn is_pinned(&self) -> bool {
        self.has_flag(FrameFlags::PINNED)
    }

    /// Vérifie si le frame est CoW.
    #[inline(always)]
    pub fn is_cow(&self) -> bool {
        self.has_flag(FrameFlags::COW)
    }

    /// Vérifie si le frame est réservé (firmware).
    #[inline(always)]
    pub fn is_reserved(&self) -> bool {
        self.has_flag(FrameFlags::RESERVED)
    }

    // ── Order (buddy) ────────────────────────────────────────────────────────

    /// Retourne l'ordre buddy courant.
    #[inline(always)]
    pub fn order(&self) -> u8 {
        self.order.load(Ordering::Relaxed)
    }

    /// Fixe l'ordre buddy.
    #[inline(always)]
    pub fn set_order(&self, order: u8) {
        self.order.store(order, Ordering::Relaxed);
    }

    // ── Mapcount ─────────────────────────────────────────────────────────────

    /// Retourne le nombre de mappings actifs.
    #[inline(always)]
    pub fn mapcount(&self) -> u16 {
        self.mapcount.load(Ordering::Relaxed)
    }

    /// Incrémente le mapcount.
    #[inline(always)]
    pub fn inc_mapcount(&self) -> u16 {
        self.mapcount.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Décrémente le mapcount. Retourne `true` si le mapcount atteint 0.
    #[inline(always)]
    pub fn dec_mapcount(&self) -> bool {
        let prev = self.mapcount.fetch_sub(1, Ordering::AcqRel);
        debug_assert_ne!(prev, 0, "mapcount underflow");
        prev == 1
    }

    // ── LRU ─────────────────────────────────────────────────────────────────

    /// Retourne la génération LRU courante.
    #[inline(always)]
    pub fn lru_gen(&self) -> u8 {
        self.lru_gen.load(Ordering::Relaxed)
    }

    /// Incrémente la génération LRU (saturation à u8::MAX).
    #[inline(always)]
    pub fn touch_lru(&self) {
        let cur = self.lru_gen.load(Ordering::Relaxed);
        if cur < u8::MAX {
            self.lru_gen.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Réinitialise la génération LRU à 0 (frame vieux → candidat éviction).
    #[inline(always)]
    pub fn reset_lru(&self) {
        self.lru_gen.store(0, Ordering::Relaxed);
    }

    // ── Transition libre ↔ alloué ────────────────────────────────────────────

    /// Marque le frame comme alloué (retire FREE, incrémente refcount à 1).
    /// Retourne false si le frame n'était pas libre (en debug: panique).
    #[inline]
    pub fn mark_allocated(&self, kernel: bool) -> bool {
        let prev_flags = FrameFlags(
            self.flags.fetch_and(!FrameFlags::FREE.bits(), Ordering::AcqRel)
        );
        if !prev_flags.contains(FrameFlags::FREE) {
            debug_assert!(false, "mark_allocated: frame n'était pas FREE");
            return false;
        }
        let access_flag = if kernel { FrameFlags::KERNEL } else { FrameFlags::USER };
        self.set_flag(access_flag);
        self.refcount.store(1, Ordering::Release);
        true
    }

    /// Marque le frame comme libre (remet les flags à FREE, refcount à 0).
    /// SAFETY: L'appelant doit garantir que plus aucune référence n'existe.
    #[inline]
    pub unsafe fn mark_free(&self) {
        debug_assert_eq!(self.refcount.load(Ordering::Acquire), 0,
            "mark_free avec refcount != 0");
        self.mapcount.store(0, Ordering::Relaxed);
        self.order.store(0, Ordering::Relaxed);
        self.lru_gen.store(0, Ordering::Relaxed);
        self.flags.store(FrameFlags::FREE.bits(), Ordering::Release);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TABLE GLOBALE DES DESCRIPTEURS DE FRAMES (FRAME_DESCRIPTORS)
// ─────────────────────────────────────────────────────────────────────────────
//
// Chaque frame physique a exactement un FrameDescEntry dans cette table.
// Indexée par le PFN (Physical Frame Number).
//
// Taille : MAX_PHYS_FRAMES × 2 octets = 4 MiB (pour 8 GiB max de RAM).
// Stockée en BSS — zéro-initialisée au boot.
//
// Accès typiques :
//   FRAME_DESCRIPTORS.get(frame).flags()           → lire les flags
//   FRAME_DESCRIPTORS.get(frame).set_flag(f)       → poser un flag
//   FRAME_DESCRIPTORS.get(frame).clear_flag(f)     → effacer un flag
//   FRAME_DESCRIPTORS.get(frame).test_and_clear(f) → CAS atomique
//
// RÈGLE : Toujours vérifier que pfn < MAX_PHYS_FRAMES avant l'accès.
//         Les frames au-delà de 8 GiB physiques ne sont pas couverts.
//         Ces frames ne doivent pas être alloués (zone guard).

use crate::memory::core::types::Frame;

/// Nombre maximal de frames couverts par la table globale.
/// 2^21 frames × 4 KiB/frame = 8 GiB RAM max.
pub const MAX_PHYS_FRAMES: usize = 1 << 21;

/// Entrée compacte de la table globale — 2 octets par frame.
/// Contient uniquement les flags LRU/CoW/DIRTY/PINNED/DMA.
#[repr(transparent)]
pub struct FrameDescEntry {
    flags: AtomicU16,
}

impl FrameDescEntry {
    const fn new() -> Self {
        FrameDescEntry { flags: AtomicU16::new(0) }
    }

    /// Retourne les flags courants.
    #[inline(always)]
    pub fn flags(&self) -> FrameFlags {
        FrameFlags(self.flags.load(Ordering::Relaxed))
    }

    /// Pose un flag.
    #[inline(always)]
    pub fn set_flag(&self, f: FrameFlags) {
        self.flags.fetch_or(f.bits(), Ordering::Relaxed);
    }

    /// Efface un flag.
    #[inline(always)]
    pub fn clear_flag(&self, f: FrameFlags) {
        self.flags.fetch_and(!f.bits(), Ordering::Relaxed);
    }

    /// Test-and-clear atomique.
    /// Retourne `true` si le flag était positionné avant l'effacement.
    #[inline]
    pub fn test_and_clear(&self, f: FrameFlags) -> bool {
        let prev = self.flags.fetch_and(!f.bits(), Ordering::AcqRel);
        FrameFlags(prev).contains(f)
    }

    /// Remplace les flags entièrement (retourne les anciens).
    #[inline(always)]
    pub fn swap_flags(&self, new: FrameFlags) -> FrameFlags {
        FrameFlags(self.flags.swap(new.bits(), Ordering::AcqRel))
    }
}

/// Table globale de tous les descripteurs légers de frames physiques.
pub struct FrameDescriptorTable {
    entries: [FrameDescEntry; MAX_PHYS_FRAMES],
}

// SAFETY: FrameDescEntry contient uniquement un AtomicU16,
// qui est Send + Sync par nature. La table est donc Send + Sync.
unsafe impl Sync for FrameDescriptorTable {}
unsafe impl Send for FrameDescriptorTable {}

impl FrameDescriptorTable {
    /// Retourne une référence vers l'entrée pour le frame donné.
    ///
    /// # Panics (debug)
    /// Si `frame.pfn() >= MAX_PHYS_FRAMES`, panique en mode debug,
    /// sature à `MAX_PHYS_FRAMES - 1` en mode release.
    #[inline]
    pub fn get(&self, frame: Frame) -> &FrameDescEntry {
        let pfn = frame.pfn() as usize;
        debug_assert!(
            pfn < MAX_PHYS_FRAMES,
            "FrameDescriptorTable::get: PFN {} hors limites (max {})",
            pfn, MAX_PHYS_FRAMES
        );
        // SAFETY: tableau[pfn] est valide pour pfn < MAX_PHYS_FRAMES.
        &self.entries[pfn.min(MAX_PHYS_FRAMES - 1)]
    }

    /// Retourne une référence directe (unsafe, sans vérification de bornes).
    ///
    /// # Safety
    /// L'appelant doit garantir que `pfn < MAX_PHYS_FRAMES`.
    #[inline(always)]
    pub unsafe fn get_unchecked(&self, pfn: u64) -> &FrameDescEntry {
        &*self.entries.as_ptr().add(pfn as usize)
    }
}

/// Table globale des descripteurs de frames.
/// En BSS — initialisée à zéro (tous les flags à 0 = EMPTY).
/// À l'allocation d'un frame, `mark_allocated()` positionne les flags appropriés.
pub static FRAME_DESCRIPTORS: FrameDescriptorTable = FrameDescriptorTable {
    entries: {
        const E: FrameDescEntry = FrameDescEntry::new();
        [E; MAX_PHYS_FRAMES]
    },
};
