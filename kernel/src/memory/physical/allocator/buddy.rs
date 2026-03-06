// kernel/src/memory/physical/allocator/buddy.rs
//
// Buddy Allocator — allocateur physique principal O(log n).
// Implémentation classique du système de buddy avec free-lists par ordre.
// Couche 0 — aucune dépendance externe.
//
// Principe :
//   La RAM est divisée en blocs de taille 2^order × PAGE_SIZE.
//   Chaque ordre dispose d'une free-list doublement chaînée.
//   Allocation  : parcours des ordres de bas en haut jusqu'au premier
//                 bloc disponible, puis division des blocs supérieurs.
//   Libération  : recherche du buddy et fusion récursive (coalescing).
//
// Invariants :
//   - Un bloc d'ordre k a une adresse alignée sur 2^k pages.
//   - Le buddy d'un bloc d'adresse P est P ⊕ (1 << k).
//   - Jamais de fragmentation externe au-delà de 1 bloc de chaque ordre.

use core::sync::atomic::{AtomicUsize, AtomicBool, Ordering};
use core::ptr::{NonNull, null_mut};

use spin::Mutex;

use crate::memory::core::{
    Frame, PhysAddr, AllocFlags, AllocError, ZoneType,
    PAGE_SIZE, BUDDY_MAX_ORDER, BUDDY_ORDER_COUNT,
    pages_to_bytes, is_aligned,
};
use crate::memory::physical::frame::descriptor::{FRAME_DESCRIPTORS, FrameFlags};

// ─────────────────────────────────────────────────────────────────────────────
// FREE LIST NODE — nœud de liste chaînée dans un buddy bloc libre
// ─────────────────────────────────────────────────────────────────────────────

/// Nœud de liste chaînée embedding dans un bloc libre.
/// Stocké directement dans les premiers octets du bloc (page physique libre).
///
/// SAFETY: Ce nœud est lisible uniquement quand le bloc est LIBRE.
/// Dès qu'un bloc est alloué, son contenu est indéfini pour l'allocateur.
#[repr(C)]
struct FreeNode {
    next:  *mut FreeNode,
    prev:  *mut FreeNode,
    order: u8,
    _pad:  [u8; 7],
}

impl FreeNode {
    #[inline]
    unsafe fn init(ptr: *mut FreeNode, order: u8) {
        (*ptr).next  = ptr;
        (*ptr).prev  = ptr;
        (*ptr).order = order;
    }

    #[inline]
    unsafe fn insert_after(list_head: *mut FreeNode, node: *mut FreeNode) {
        let next = (*list_head).next;
        (*node).prev      = list_head;
        (*node).next      = next;
        (*next).prev      = node;
        (*list_head).next = node;
    }

    #[inline]
    unsafe fn remove(node: *mut FreeNode) {
        let prev = (*node).prev;
        let next = (*node).next;
        (*prev).next = next;
        (*next).prev = prev;
        (*node).next = node;
        (*node).prev = node;
    }

    #[inline]
    unsafe fn is_empty(list_head: *const FreeNode) -> bool {
        (*list_head).next == list_head as *mut FreeNode
    }

    #[inline]
    unsafe fn pop(list_head: *mut FreeNode) -> Option<*mut FreeNode> {
        if Self::is_empty(list_head) {
            return None;
        }
        let node = (*list_head).next;
        Self::remove(node);
        Some(node)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BUDDY ORDER LEVEL — une free-list pour un ordre donné
// ─────────────────────────────────────────────────────────────────────────────

/// Tête de free-list pour un ordre donné.
#[repr(C, align(64))]
struct BuddyOrderLevel {
    /// Tête sentinelle (nœud vide — next/prev pointent vers elle-même si vide).
    head:  FreeNode,
    /// Nombre de blocs libres à cet ordre.
    count: AtomicUsize,
}

impl BuddyOrderLevel {
    const fn new() -> Self {
        BuddyOrderLevel {
            head:  FreeNode { next: null_mut(), prev: null_mut(), order: 0, _pad: [0; 7] },
            count: AtomicUsize::new(0),
        }
    }

    /// Initialise la sentinelle (ne peut pas être fait en const, besoin de pointeur).
    unsafe fn init(&mut self) {
        let h = &mut self.head as *mut FreeNode;
        (*h).next = h;
        (*h).prev = h;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BUDDY ZONE — allocateur buddy pour une zone mémoire
// ─────────────────────────────────────────────────────────────────────────────

/// Allocateur buddy pour une zone mémoire donnée.
///
/// Chaque zone (DMA, DMA32, Normal, Movable) dispose de sa propre instance.
/// L'allocation est protégée par un spinlock par zone, ce qui permet
/// la concurrence inter-zones sans contention croisée.
pub struct BuddyZone {
    /// Zone mémoire gérée par cet allocateur.
    zone_type:    ZoneType,
    /// Nœud NUMA.
    numa_node:    u8,
    /// Adresse physique de début.
    phys_start:   PhysAddr,
    /// Adresse physique de fin (exclusive).
    phys_end:     PhysAddr,
    /// Nombre total de frames dans cette zone.
    total_frames: usize,
    /// Free-lists protégées par spinlock.
    /// L'usage de spin::Mutex est autorisé dans memory/ (Couche 0).
    inner: Mutex<BuddyZoneInner>,
    /// Nombre de frames libres (snapshot approximatif, cache-friendly).
    free_frames:  AtomicUsize,
    /// Compteur d'allocations.
    alloc_count:  AtomicUsize,
    /// Compteur de libérations.
    free_count:   AtomicUsize,
    /// Compteur d'échecs (fragmentation ou mémoire épuisée).
    fail_count:   AtomicUsize,
    /// Compteur de scissions (splits).
    split_count:  AtomicUsize,
    /// Compteur de fusions (merges/coalescing).
    merge_count:  AtomicUsize,
    /// Indicateur d'initialisation.
    initialized:  AtomicBool,
}

struct BuddyZoneInner {
    /// Free-lists par ordre (0..=BUDDY_MAX_ORDER).
    orders: [BuddyOrderLevel; BUDDY_ORDER_COUNT],
    /// Bitmap de disponibilité par frame (1 bit par frame).
    /// Alloué dans un buffer statique fourni au boot.
    /// Null si non configuré.
    ///
    /// SAFETY: Le bitmap est accessible uniquement sous le spinlock.
    bitmap_ptr:   *mut u64,
    bitmap_words: usize,
}

// SAFETY: BuddyZoneInner n'est accessible que sous le spinlock de BuddyZone.
unsafe impl Send for BuddyZoneInner {}

impl BuddyZoneInner {
    const fn new() -> Self {
        BuddyZoneInner {
            orders: [
                const { BuddyOrderLevel::new() };
                BUDDY_ORDER_COUNT
            ],
            bitmap_ptr:   null_mut(),
            bitmap_words: 0,
        }
    }

    /// Marque le frame `pfn` comme libre dans le bitmap.
    #[inline(always)]
    unsafe fn bitmap_clear(&self, pfn: usize) {
        if self.bitmap_ptr.is_null() { return; }
        let word = pfn / 64;
        let bit  = pfn % 64;
        if word < self.bitmap_words {
            (*self.bitmap_ptr.add(word)) &= !(1u64 << bit);
        }
    }

    /// Marque le frame `pfn` comme alloué dans le bitmap.
    #[inline(always)]
    unsafe fn bitmap_set(&self, pfn: usize) {
        if self.bitmap_ptr.is_null() { return; }
        let word = pfn / 64;
        let bit  = pfn % 64;
        if word < self.bitmap_words {
            (*self.bitmap_ptr.add(word)) |= 1u64 << bit;
        }
    }

    /// Retourne `true` si le frame `pfn` est libre dans le bitmap.
    #[inline(always)]
    unsafe fn bitmap_is_free(&self, pfn: usize) -> bool {
        if self.bitmap_ptr.is_null() { return false; }
        let word = pfn / 64;
        let bit  = pfn % 64;
        if word >= self.bitmap_words { return false; }
        ((*self.bitmap_ptr.add(word)) & (1u64 << bit)) == 0
    }
}

impl BuddyZone {
    /// Crée une instance non initialisée.
    pub const fn new_uninit() -> Self {
        BuddyZone {
            zone_type:    ZoneType::Normal,
            numa_node:    0,
            phys_start:   PhysAddr::new(0),
            phys_end:     PhysAddr::new(0),
            total_frames: 0,
            inner:        Mutex::new(BuddyZoneInner::new()),
            free_frames:  AtomicUsize::new(0),
            alloc_count:  AtomicUsize::new(0),
            free_count:   AtomicUsize::new(0),
            fail_count:   AtomicUsize::new(0),
            split_count:  AtomicUsize::new(0),
            merge_count:  AtomicUsize::new(0),
            initialized:  AtomicBool::new(false),
        }
    }

    /// Initialise l'allocateur buddy pour une zone.
    ///
    /// `bitmap_buf` : buffer alloué statiquement pour le bitmap de disponibilité.
    ///               Taille requise : ceil(total_pages / 64) × 8 bytes.
    ///
    /// SAFETY: Doit être appelé une seule fois depuis un contexte single-CPU
    ///         (avant l'init SMP).
    pub unsafe fn init(
        &self,
        zone_type:    ZoneType,
        numa_node:    u8,
        phys_start:   PhysAddr,
        phys_end:     PhysAddr,
        bitmap_buf:   *mut u64,
        bitmap_words: usize,
    ) {
        // Stocker les paramètres de zone
        let s = self as *const BuddyZone as *mut BuddyZone;
        (*s).zone_type    = zone_type;
        (*s).numa_node    = numa_node;
        (*s).phys_start   = phys_start;
        (*s).phys_end     = phys_end;
        (*s).total_frames =
            ((phys_end.as_u64() - phys_start.as_u64()) / PAGE_SIZE as u64) as usize;

        // Initialiser l'inner sous lock
        let mut inner = self.inner.lock();
        for order in &mut inner.orders {
            order.init();
        }
        inner.bitmap_ptr   = bitmap_buf;
        inner.bitmap_words = bitmap_words;

        // Initialiser le bitmap à "tout alloué" (bits = 1)
        if !bitmap_buf.is_null() {
            for i in 0..bitmap_words {
                *bitmap_buf.add(i) = u64::MAX;
            }
        }

        drop(inner);

        self.initialized.store(true, Ordering::Release);
    }

    /// Ajoute une plage de frames libres à l'allocateur (boot: après init).
    ///
    /// Appelé par le détecteur de mémoire E820 pour peupler l'allocateur
    /// avec les régions RAM disponibles.
    ///
    /// SAFETY: Doit être appelé avant l'init SMP. `first_pfn..last_pfn`
    /// doit être une plage valide et non chevauchante avec des ranges existants.
    pub unsafe fn add_free_range(&self, first_pfn: usize, last_pfn: usize) {
        debug_assert!(self.initialized.load(Ordering::Acquire));
        debug_assert!(first_pfn < last_pfn);

        let mut pfn = first_pfn;
        while pfn < last_pfn {
            // Déterminer l'ordre maximal utilisable depuis ce pfn
            let remaining = last_pfn - pfn;
            let max_order_by_align = if pfn == 0 {
                0
            } else {
                pfn.trailing_zeros() as usize
            };
            let max_order_by_count = (usize::BITS - remaining.leading_zeros() - 1) as usize;
            let order = max_order_by_align
                .min(max_order_by_count)
                .min(BUDDY_MAX_ORDER);

            let phys = PhysAddr::new((self.phys_start.as_u64()) + (pfn as u64 * PAGE_SIZE as u64));
            self.add_free_block(phys, order);
            pfn += 1 << order;
        }
    }

    /// Ajoute un bloc libre à la free-list de l'ordre correspondant.
    ///
    /// SAFETY: `phys` doit être aligné sur 2^order pages et appartenir
    /// à cette zone. Appelé uniquement pendant l'init ou la libération.
    unsafe fn add_free_block(&self, phys: PhysAddr, order: usize) {
        debug_assert!(order <= BUDDY_MAX_ORDER);
        debug_assert!(phys >= self.phys_start && phys < self.phys_end);

        let node_ptr = phys_to_virt_buddy(phys) as *mut FreeNode;
        FreeNode::init(node_ptr, order as u8);

        let mut inner = self.inner.lock();
        FreeNode::insert_after(&mut inner.orders[order].head as *mut FreeNode, node_ptr);
        inner.orders[order].count.fetch_add(1, Ordering::Relaxed);

        // Marquer comme libre dans le bitmap
        let pfn = ((phys.as_u64() - self.phys_start.as_u64()) / PAGE_SIZE as u64) as usize;
        for i in 0..(1 << order) {
            inner.bitmap_clear(pfn + i);
        }

        self.free_frames.fetch_add(1 << order, Ordering::Relaxed);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // ALLOCATION
    // ─────────────────────────────────────────────────────────────────────────

    /// Alloue `2^order` pages contiguës depuis cette zone.
    ///
    /// Algorithme :
    ///   1. Vérifier les hints NUMA IA (lookup statique .rodata).
    ///   2. Parcourir les ordres de `order` vers `BUDDY_MAX_ORDER`.
    ///   3. Si un bloc est trouvé à un ordre supérieur `found_order` :
    ///      - Diviser (split) jusqu'à l'ordre demandé.
    ///      - Ajouter les demi-blocs supérieurs à leurs free-lists.
    ///   4. Marquer les pages comme allouées dans le bitmap.
    ///   5. Mettre à jour les statistiques.
    pub fn alloc_pages(&self, order: usize, flags: AllocFlags) -> Result<Frame, AllocError> {
        debug_assert!(order <= BUDDY_MAX_ORDER, "ordre buddy hors limites");

        if !self.initialized.load(Ordering::Acquire) {
            return Err(AllocError::NotInitialized);
        }

        // RÈGLE NO-ALLOC : pas d'allocation, tout se passe sous le lock.
        let result = {
            let mut inner = self.inner.lock();
            self.alloc_inner(&mut inner, order, flags)
        };

        match result {
            Ok(phys) => {
                self.free_frames.fetch_sub(1 << order, Ordering::Relaxed);
                self.alloc_count.fetch_add(1, Ordering::Relaxed);
                if flags.contains(AllocFlags::ZEROED) {
                    // SAFETY: Le frame vient d'être alloué, nous en sommes l'unique propriétaire.
                    unsafe { zero_pages(phys, order); }
                }
                let frame = Frame::containing(phys);
                // V-05 / MEM-05 : poser DMA_PINNED sur tous les frames DMA jusqu'à
                // wait_dma_complete() — protège contre reclaim et swap.
                if flags.contains(AllocFlags::DMA) || flags.contains(AllocFlags::DMA32) {
                    FRAME_DESCRIPTORS.get(frame).set_flag(FrameFlags::DMA_PINNED);
                }
                Ok(frame)
            }
            Err(e) => {
                self.fail_count.fetch_add(1, Ordering::Relaxed);
                Err(e)
            }
        }
    }

    /// Logique d'allocation interne (sous spinlock).
    fn alloc_inner(
        &self,
        inner: &mut BuddyZoneInner,
        order: usize,
        _flags: AllocFlags,
    ) -> Result<PhysAddr, AllocError> {
        // Chercher le premier ordre >= `order` avec un bloc disponible
        for current_order in order..=BUDDY_MAX_ORDER {
            // SAFETY: Sous le spinlock, accès exclusif aux free-lists.
            unsafe {
                let head = &mut inner.orders[current_order].head as *mut FreeNode;
                match FreeNode::pop(head) {
                    None => continue, // Pas de bloc à cet ordre
                    Some(node) => {
                        inner.orders[current_order].count.fetch_sub(1, Ordering::Relaxed);

                        let phys = virt_to_phys_buddy(node as usize);

                        // Diviser (split) si nous sommes à un ordre supérieur
                        let block_phys = phys;
                        for split_order in (order..current_order).rev() {
                            // Le buddy de `block_phys` à l'ordre `split_order + 1`
                            // est `block_phys + 2^split_order × PAGE_SIZE`
                            let buddy_phys = PhysAddr::new(
                                block_phys.as_u64() + (PAGE_SIZE << split_order) as u64
                            );

                            // Remettre le buddy dans la free-list
                            let buddy_node = phys_to_virt_buddy(buddy_phys) as *mut FreeNode;
                            FreeNode::init(buddy_node, split_order as u8);
                            let buddy_head = &mut inner.orders[split_order].head as *mut FreeNode;
                            FreeNode::insert_after(buddy_head, buddy_node);
                            inner.orders[split_order].count.fetch_add(1, Ordering::Relaxed);

                            // Vider le bitmap du buddy (il est libre maintenant)
                            let buddy_pfn = self.phys_to_pfn(buddy_phys);
                            for i in 0..(1usize << split_order) {
                                inner.bitmap_clear(buddy_pfn + i);
                            }

                            self.split_count.fetch_add(1, Ordering::Relaxed);
                        }

                        // Marquer les pages allouées dans le bitmap
                        let pfn = self.phys_to_pfn(block_phys);
                        for i in 0..(1usize << order) {
                            inner.bitmap_set(pfn + i);
                        }

                        return Ok(block_phys);
                    }
                }
            }
        }

        Err(AllocError::OutOfMemory)
    }

    // ─────────────────────────────────────────────────────────────────────────
    // LIBÉRATION + COALESCING
    // ─────────────────────────────────────────────────────────────────────────

    /// Libère un bloc de `2^order` pages commençant à `frame`.
    ///
    /// Algorithme de coalescing :
    ///   1. Calculer l'adresse du buddy.
    ///   2. Si le buddy est libre (dans le bitmap) et au même ordre :
    ///      - Retirer le buddy de sa free-list.
    ///      - Fusionner les deux blocs (prendre le plus petit pfn).
    ///      - Incrémenter l'ordre et recommencer.
    ///   3. Sinon, insérer dans la free-list à l'ordre courant.
    pub fn free_pages(&self, frame: Frame, order: usize) -> Result<(), AllocError> {
        debug_assert!(order <= BUDDY_MAX_ORDER);

        if !self.initialized.load(Ordering::Acquire) {
            return Err(AllocError::NotInitialized);
        }

        let phys = frame.start_address();
        debug_assert!(phys >= self.phys_start && phys < self.phys_end,
            "free_pages: frame hors zone");
        debug_assert!(
            is_aligned(phys.as_usize(), PAGE_SIZE << order),
            "free_pages: frame mal aligné pour l'ordre {}", order
        );

        {
            let mut inner = self.inner.lock();
            // SAFETY: Sous spinlock, accès exclusif.
            unsafe { self.free_inner(&mut inner, phys, order); }
        }

        self.free_frames.fetch_add(1 << order, Ordering::Relaxed);
        self.free_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    unsafe fn free_inner(&self, inner: &mut BuddyZoneInner, phys: PhysAddr, order: usize) {
        let mut current_phys  = phys;
        let mut current_order = order;

        // Marquer les pages comme libres dans le bitmap
        let pfn = self.phys_to_pfn(current_phys);
        for i in 0..(1usize << order) {
            inner.bitmap_clear(pfn + i);
        }

        // Tentatives de fusion (coalescing)
        while current_order < BUDDY_MAX_ORDER {
            let buddy_phys = self.buddy_of(current_phys, current_order);

            // Le buddy doit être dans la même zone
            if buddy_phys < self.phys_start || buddy_phys >= self.phys_end {
                break;
            }

            // Vérifier si le buddy est libre dans le bitmap
            let buddy_pfn = self.phys_to_pfn(buddy_phys);
            let buddy_free = (0..(1usize << current_order))
                .all(|i| inner.bitmap_is_free(buddy_pfn + i));

            if !buddy_free {
                break; // Le buddy est alloué → pas de fusion possible
            }

            // Retirer le buddy de sa free-list
            let buddy_node = phys_to_virt_buddy(buddy_phys) as *mut FreeNode;
            // Vérifier que le buddy est bien dans une free-list (ordre cohérent)
            if (*buddy_node).order != current_order as u8 {
                break;
            }
            FreeNode::remove(buddy_node);
            inner.orders[current_order].count.fetch_sub(1, Ordering::Relaxed);

            // Fusionner : prendre l'adresse la plus basse des deux blocs
            current_phys = if current_phys < buddy_phys { current_phys } else { buddy_phys };
            current_order += 1;
            self.merge_count.fetch_add(1, Ordering::Relaxed);
        }

        // Insérer le bloc (éventuellement fusionné) dans la free-list
        let node_ptr = phys_to_virt_buddy(current_phys) as *mut FreeNode;
        FreeNode::init(node_ptr, current_order as u8);
        let head = &mut inner.orders[current_order].head as *mut FreeNode;
        FreeNode::insert_after(head, node_ptr);
        inner.orders[current_order].count.fetch_add(1, Ordering::Relaxed);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // UTILITAIRES
    // ─────────────────────────────────────────────────────────────────────────

    /// Calcule l'adresse physique du buddy d'un bloc.
    /// Le buddy d'un bloc à l'ordre k, adresse P, est P ⊕ (2^k × PAGE_SIZE).
    #[inline(always)]
    fn buddy_of(&self, phys: PhysAddr, order: usize) -> PhysAddr {
        let relative = phys.as_u64() - self.phys_start.as_u64();
        let buddy_relative = relative ^ ((PAGE_SIZE << order) as u64);
        PhysAddr::new(self.phys_start.as_u64() + buddy_relative)
    }

    /// Retourne le PFN relatif au début de cette zone.
    #[inline(always)]
    fn phys_to_pfn(&self, phys: PhysAddr) -> usize {
        ((phys.as_u64() - self.phys_start.as_u64()) / PAGE_SIZE as u64) as usize
    }

    /// Retourne le nombre de frames libres (snapshot approximatif).
    #[inline(always)]
    pub fn free_frames(&self) -> usize {
        self.free_frames.load(Ordering::Relaxed)
    }

    /// Retourne le nombre total de frames gérées par cette zone.
    #[inline(always)]
    pub fn total_frames_count(&self) -> usize {
        self.total_frames
    }

    /// Retourne les statistiques de l'allocateur.
    pub fn stats(&self) -> BuddyZoneStats {
        let inner = self.inner.lock();
        let mut free_by_order = [0usize; BUDDY_ORDER_COUNT];
        for (i, order) in inner.orders.iter().enumerate() {
            free_by_order[i] = order.count.load(Ordering::Relaxed);
        }
        BuddyZoneStats {
            zone_type:    self.zone_type,
            numa_node:    self.numa_node,
            total_frames: self.total_frames,
            free_frames:  self.free_frames.load(Ordering::Relaxed),
            alloc_count:  self.alloc_count.load(Ordering::Relaxed),
            free_count:   self.free_count.load(Ordering::Relaxed),
            fail_count:   self.fail_count.load(Ordering::Relaxed),
            split_count:  self.split_count.load(Ordering::Relaxed),
            merge_count:  self.merge_count.load(Ordering::Relaxed),
            free_by_order,
        }
    }

    /// Vérifie si l'allocateur gère la zone donnée.
    #[inline(always)]
    pub fn manages_zone(&self, zone: ZoneType) -> bool {
        self.zone_type == zone
    }

    /// Vérifie si l'allocateur est initialisé.
    #[inline(always)]
    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Acquire)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// STATISTIQUES
// ─────────────────────────────────────────────────────────────────────────────

/// Statistiques d'une zone buddy.
#[derive(Copy, Clone, Debug)]
pub struct BuddyZoneStats {
    pub zone_type:    ZoneType,
    pub numa_node:    u8,
    pub total_frames: usize,
    pub free_frames:  usize,
    pub alloc_count:  usize,
    pub free_count:   usize,
    pub fail_count:   usize,
    pub split_count:  usize,
    pub merge_count:  usize,
    /// Nombre de blocs libres par ordre (0..BUDDY_ORDER_COUNT).
    pub free_by_order: [usize; BUDDY_ORDER_COUNT],
}

impl BuddyZoneStats {
    /// Pourcentage de mémoire libre (0.0-100.0).
    pub fn free_percent(&self) -> f32 {
        if self.total_frames == 0 { return 0.0; }
        self.free_frames as f32 * 100.0 / self.total_frames as f32
    }

    /// Taux de fragmentation (0.0 = aucune, 1.0 = maximale).
    /// Mesure l'écart entre la mémoire libre totale et les blocs de grand ordre.
    pub fn fragmentation_ratio(&self) -> f32 {
        if self.free_frames == 0 { return 0.0; }
        let largest_order = (0..BUDDY_ORDER_COUNT).rev()
            .find(|&o| self.free_by_order[o] > 0)
            .unwrap_or(0);
        let max_contiguous = self.free_by_order[largest_order] * (1 << largest_order);
        1.0 - (max_contiguous as f32 / self.free_frames as f32)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ALLOCATEUR BUDDY GLOBAL — couvre toutes les zones
// ─────────────────────────────────────────────────────────────────────────────

/// Allocateur buddy global couvrant toutes les zones et tous les nœuds NUMA.
///
/// Routage des allocations selon les flags :
///   - AllocFlags::DMA    → BuddyZone DMA
///   - AllocFlags::DMA32  → BuddyZone DMA32
///   - Default           → BuddyZone Normal (NUMA-preferred)
pub struct GlobalBuddyAllocator {
    // Une zone par ZoneType par nœud NUMA
    // Simplifié : tableau [ZoneType::COUNT] (NUMA node 0 par défaut pour l'instant)
    zones: [BuddyZone; 4], // DMA, DMA32, Normal, Movable
    initialized: AtomicBool,
}

// SAFETY: GlobalBuddyAllocator est thread-safe via ses spinlocks internes.
unsafe impl Sync for GlobalBuddyAllocator {}
unsafe impl Send for GlobalBuddyAllocator {}

impl GlobalBuddyAllocator {
    pub const fn new() -> Self {
        GlobalBuddyAllocator {
            zones:       [const { BuddyZone::new_uninit() }; 4],
            initialized: AtomicBool::new(false),
        }
    }

    /// Alloue `2^order` pages contiguës selon les flags.
    /// Sélectionne automatiquement la zone appropriée.
    #[inline]
    pub fn alloc_pages(&self, order: usize, flags: AllocFlags) -> Result<Frame, AllocError> {
        debug_assert!(self.initialized.load(Ordering::Acquire),
            "GlobalBuddyAllocator non initialisé");

        let zone_idx = self.zone_index_for(flags);
        let result = self.zones[zone_idx].alloc_pages(order, flags);

        // Fallback vers zone supérieure si la zone cible est épuisée
        if result.is_err() && !flags.contains(AllocFlags::DMA) {
            for fallback_idx in (zone_idx + 1)..4 {
                let r = self.zones[fallback_idx].alloc_pages(order, flags);
                if r.is_ok() { return r; }
            }
        }

        // Fallback vers zone inférieure (Normal → DMA32 sur systèmes avec < 4 GiB RAM).
        // Standard x86_64 : Normal est vide quand toute la RAM est < 4 GiB.
        // Ne descend pas en-dessous de DMA32 (idx=1) pour les allocations non-DMA.
        if result.is_err() && zone_idx > 1 {
            for fallback_idx in (1..zone_idx).rev() {
                let r = self.zones[fallback_idx].alloc_pages(order, flags);
                if r.is_ok() { return r; }
            }
        }

        result
    }

    /// Libère un bloc de `2^order` pages.
    #[inline]
    pub fn free_pages(&self, frame: Frame, order: usize) -> Result<(), AllocError> {
        let phys = frame.start_address();
        // Déterminer la zone d'appartenance
        for zone in &self.zones {
            if zone.is_initialized() && zone.phys_start <= phys && phys < zone.phys_end {
                return zone.free_pages(frame, order);
            }
        }
        Err(AllocError::ZoneUnavailable)
    }

    /// Alloue depuis le nœud NUMA demandé.
    ///
    /// Itère les zones initialisées dont `numa_node` correspond à `numa_node`.
    /// Si aucune zone NUMA-locale ne peut satisfaire la demande, replie sur
    /// l'allocation globale (premier fallback).
    pub fn alloc_on_node(
        &self,
        order: usize,
        flags: AllocFlags,
        numa_node: u8,
    ) -> Result<Frame, AllocError> {
        debug_assert!(self.initialized.load(Ordering::Acquire),
            "GlobalBuddyAllocator non initialisé");

        // Phase 1 : essai dans les zones NUMA-locales.
        for zone in &self.zones {
            if !zone.is_initialized() { continue; }
            if zone.numa_node != numa_node { continue; }
            if let Ok(frame) = zone.alloc_pages(order, flags) {
                return Ok(frame);
            }
        }

        // Phase 2 : fallback global (allocation cross-NUMA).
        self.alloc_pages(order, flags)
    }

    #[inline]
    fn zone_index_for(&self, flags: AllocFlags) -> usize {
        match flags.required_zone() {
            ZoneType::Dma     => 0,
            ZoneType::Dma32   => 1,
            ZoneType::Movable => 3,
            _                 => 2, // Normal par défaut
        }
    }

    /// Retourne le nombre total de frames libres dans toutes les zones.
    pub fn total_free_frames(&self) -> usize {
        self.zones.iter()
            .filter(|z| z.is_initialized())
            .map(|z| z.free_frames())
            .sum()
    }

    /// Retourne le nombre total de frames physiques gérées (libres + utilisées).
    pub fn total_frames(&self) -> usize {
        self.zones.iter()
            .filter(|z| z.is_initialized())
            .map(|z| z.total_frames_count())
            .sum()
    }

    /// Marque l'allocateur comme initialisé.
    pub fn mark_initialized(&self) {
        self.initialized.store(true, Ordering::Release);
    }

    /// Initialise une zone de l'allocateur (appelé au boot, single-CPU).
    ///
    /// `zone_type` détermine l'index de la zone (0=DMA, 1=DMA32, 2=Normal, 3=Movable).
    /// `bitmap_buf` doit pointer vers un buffer statique de `bitmap_words` u64.
    ///
    /// # Safety
    /// - Doit être appelé UNE SEULE FOIS par zone, avant SMP.
    /// - `bitmap_buf` doit être valide pour `bitmap_words` × 8 bytes.
    pub unsafe fn init_zone(
        &self,
        zone_type:    ZoneType,
        phys_start:   PhysAddr,
        phys_end:     PhysAddr,
        bitmap_buf:   *mut u64,
        bitmap_words: usize,
    ) {
        let idx = (zone_type.index()).min(3);
        self.zones[idx].init(zone_type, 0 /* NUMA node 0 */,
            phys_start, phys_end, bitmap_buf, bitmap_words);
    }

    /// Ajoute une plage de frames libres aux zones initialisées qui couvrent cette plage.
    ///
    /// Appelé pour chaque région E820/UEFI utilisable, après `init_zone()`.
    ///
    /// # Safety
    /// - Zones concernées doivent être initialisées.
    /// - La plage ne doit pas contenir de mémoire kernel active.
    pub unsafe fn add_free_zone_region(&self, start: PhysAddr, end: PhysAddr) {
        for zone in &self.zones {
            if !zone.is_initialized() { continue; }
            let zs = zone.phys_start.as_u64();
            let ze = zone.phys_end.as_u64();
            let rs = start.as_u64();
            let re = end.as_u64();
            if rs >= ze || re <= zs { continue; }    // pas d'overlap
            let cs = rs.max(zs);
            let ce = re.min(ze);
            if cs >= ce { continue; }
            let first_pfn = ((cs - zs) / PAGE_SIZE as u64) as usize;
            let last_pfn  = ((ce - zs) / PAGE_SIZE as u64) as usize;
            zone.add_free_range(first_pfn, last_pfn);
        }
    }

    /// Retourne une référence à la zone pour un type donné.
    pub fn zone(&self, zone_type: ZoneType) -> Option<&BuddyZone> {
        let idx = match zone_type {
            ZoneType::Dma     => 0,
            ZoneType::Dma32   => 1,
            ZoneType::Normal  => 2,
            ZoneType::Movable => 3,
            ZoneType::High    => return None,
        };
        let z = &self.zones[idx];
        if z.is_initialized() { Some(z) } else { None }
    }

    /// Accès mutant à une zone (initialisation uniquement).
    ///
    /// SAFETY: Doit être appelé uniquement pendant l'init, en single-CPU.
    pub unsafe fn zone_mut(&self, zone_type: ZoneType) -> Option<&BuddyZone> {
        self.zone(zone_type)
    }
}

/// Allocateur buddy global.
pub static BUDDY: GlobalBuddyAllocator = GlobalBuddyAllocator::new();

/// Alloue `2^order` pages physiques contiguës.
#[inline(always)]
pub fn alloc_pages(order: usize, flags: AllocFlags) -> Result<Frame, AllocError> {
    BUDDY.alloc_pages(order, flags)
}

/// Libère un bloc de `2^order` pages.
#[inline(always)]
pub fn free_pages(frame: Frame, order: usize) -> Result<(), AllocError> {
    BUDDY.free_pages(frame, order)
}

/// Alloue exactement 1 page physique (ordre 0).
#[inline(always)]
pub fn alloc_page(flags: AllocFlags) -> Result<Frame, AllocError> {
    alloc_pages(0, flags)
}

/// Libère exactement 1 page physique.
#[inline(always)]
pub fn free_page(frame: Frame) -> Result<(), AllocError> {
    free_pages(frame, 0)
}

// ─────────────────────────────────────────────────────────────────────────────
// UTILITAIRES INTERNES (bridge physmap temporaire)
// ─────────────────────────────────────────────────────────────────────────────

/// Traduit une adresse physique en pointeur virtuel pour accéder aux FreeNodes.
/// Utilise la physmap directe (PHYS_MAP_BASE + phys).
///
/// SAFETY: La physmap doit être initialisée avant l'utilisation du buddy.
#[inline(always)]
fn phys_to_virt_buddy(phys: PhysAddr) -> usize {
    use crate::memory::core::layout::PHYS_MAP_BASE;
    PHYS_MAP_BASE.as_usize() + phys.as_usize()
}

/// Traduit un pointeur virtuel (dans la physmap) en adresse physique.
#[inline(always)]
fn virt_to_phys_buddy(virt: usize) -> PhysAddr {
    use crate::memory::core::layout::PHYS_MAP_BASE;
    PhysAddr::new((virt - PHYS_MAP_BASE.as_usize()) as u64)
}

/// Remplit une plage de pages de zéros.
///
/// SAFETY: `phys` doit être l'adresse de `2^order` pages allouées exclusivement
/// par l'appelant. La physmap doit être accessible.
#[inline]
unsafe fn zero_pages(phys: PhysAddr, order: usize) {
    let size = PAGE_SIZE << order;
    let virt = phys_to_virt_buddy(phys) as *mut u8;
    // SAFETY: Les pages sont allouées et mappées en physmap.
    core::ptr::write_bytes(virt, 0, size);
}
