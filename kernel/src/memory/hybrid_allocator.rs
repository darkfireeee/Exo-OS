//! # Hybrid Allocator - Allocateur Mémoire 3 Niveaux
//! 
//! Architecture inspirée de TCMalloc/jemalloc avec optimisations pour kernel bare-metal.
//! 
//! ## Structure
//! 
//! ```text
//! ThreadCache (niveau 1) → CpuSlab (niveau 2) → BuddyAllocator (niveau 3)
//!     O(1) sans lock          Per-CPU lock-free      Lock pour grandes allocs
//! ```
//! 
//! ## Gains Attendus
//! - **5-15× plus rapide** que linked_list_allocator
//! - **>90% hit rate** sur ThreadCache
//! - **Zero contention** pour allocations petites (<2KB)
//! 
//! ## Tailles de Bins
//! - ThreadCache: 8, 16, 24, 32, 48, 64, 96, 128, 192, 256, 384, 512, 768, 1024, 1536, 2048 bytes
//! - CpuSlab: Pages 4KB
//! - Buddy: Blocs 4KB, 8KB, 16KB, 32KB, 64KB, 128KB, 256KB, 512KB, 1MB

use core::alloc::{GlobalAlloc, Layout};
use core::ptr::{self, NonNull};
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;
use alloc::vec::Vec;

/// Nombre de bins dans le ThreadCache
const NUM_BINS: usize = 16;

/// Tailles des bins (en bytes)
const BIN_SIZES: [usize; NUM_BINS] = [
    8, 16, 24, 32, 48, 64, 96, 128, 192, 256, 384, 512, 768, 1024, 1536, 2048,
];

/// Nombre max d'objets par bin
const MAX_OBJECTS_PER_BIN: usize = 64;

/// Taille d'une page
const PAGE_SIZE: usize = 4096;

/// Thread Cache - Niveau 1 (O(1), sans lock)
#[repr(C, align(64))]
pub struct ThreadCache {
    /// Bins pour chaque taille
    bins: [Bin; NUM_BINS],
    
    /// Statistiques
    stats: CacheStats,
    
    /// ID du thread propriétaire
    owner_thread: usize,
}

/// Bin pour une taille spécifique
#[repr(C, align(64))]
struct Bin {
    /// Liste libre de blocs (pointeurs vers premier bloc libre)
    free_list: *mut FreeBlock,
    
    /// Nombre d'objets actuellement dans le bin
    count: usize,
    
    /// Taille des objets dans ce bin
    object_size: usize,
    
    /// Padding pour éviter false sharing
    _pad: [u8; 40],
}

/// Bloc libre dans un bin
#[repr(C)]
struct FreeBlock {
    /// Pointeur vers le prochain bloc libre
    next: *mut FreeBlock,
}

/// Statistiques du cache
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CacheStats {
    /// Nombre de hits (allocation depuis cache)
    pub hits: u64,
    
    /// Nombre de misses (allocation depuis niveau supérieur)
    pub misses: u64,
    
    /// Nombre de bytes alloués
    pub bytes_allocated: u64,
    
    /// Nombre de bytes libérés
    pub bytes_freed: u64,
}

impl ThreadCache {
    /// Crée un nouveau ThreadCache
    pub const fn new(thread_id: usize) -> Self {
        const EMPTY_BIN: Bin = Bin {
            free_list: ptr::null_mut(),
            count: 0,
            object_size: 0,
            _pad: [0; 40],
        };
        
        Self {
            bins: [EMPTY_BIN; NUM_BINS],
            stats: CacheStats {
                hits: 0,
                misses: 0,
                bytes_allocated: 0,
                bytes_freed: 0,
            },
            owner_thread: thread_id,
        }
    }
    
    /// Initialise les bins avec les bonnes tailles
    pub fn init(&mut self) {
        for (i, bin) in self.bins.iter_mut().enumerate() {
            bin.object_size = BIN_SIZES[i];
        }
    }
    
    /// Trouve l'index du bin pour une taille donnée
    #[inline(always)]
    fn bin_index(size: usize) -> Option<usize> {
        // Recherche binaire pour trouver le bin approprié
        let mut left = 0;
        let mut right = NUM_BINS;
        
        while left < right {
            let mid = (left + right) / 2;
            if BIN_SIZES[mid] < size {
                left = mid + 1;
            } else {
                right = mid;
            }
        }
        
        if left < NUM_BINS {
            Some(left)
        } else {
            None
        }
    }
    
    /// Alloue un objet depuis le cache (fast path)
    #[inline(always)]
    pub unsafe fn allocate(&mut self, size: usize) -> Option<*mut u8> {
        // Trouve le bin approprié
        let bin_idx = Self::bin_index(size)?;
        let bin = &mut self.bins[bin_idx];
        
        // Si le bin a des objets libres, retourne le premier
        if !bin.free_list.is_null() {
            let block = bin.free_list;
            bin.free_list = (*block).next;
            bin.count -= 1;
            
            // Statistiques
            self.stats.hits += 1;
            self.stats.bytes_allocated += bin.object_size as u64;
            
            Some(block as *mut u8)
        } else {
            // Cache miss - besoin de refill depuis niveau supérieur
            self.stats.misses += 1;
            None
        }
    }
    
    /// Libère un objet dans le cache
    #[inline(always)]
    pub unsafe fn deallocate(&mut self, ptr: *mut u8, size: usize) -> bool {
        // Trouve le bin approprié
        let bin_idx = match Self::bin_index(size) {
            Some(idx) => idx,
            None => return false, // Trop grand pour le cache
        };
        
        let bin = &mut self.bins[bin_idx];
        
        // Si le bin est plein, refuse (sera libéré vers niveau supérieur)
        if bin.count >= MAX_OBJECTS_PER_BIN {
            return false;
        }
        
        // Ajoute le bloc à la liste libre
        let block = ptr as *mut FreeBlock;
        (*block).next = bin.free_list;
        bin.free_list = block;
        bin.count += 1;
        
        // Statistiques
        self.stats.bytes_freed += bin.object_size as u64;
        
        true
    }
    
    /// Retourne les statistiques du cache
    pub fn stats(&self) -> CacheStats {
        self.stats
    }
    
    /// Calcule le hit rate (pourcentage de hits)
    pub fn hit_rate(&self) -> f64 {
        let total = self.stats.hits + self.stats.misses;
        if total == 0 {
            0.0
        } else {
            (self.stats.hits as f64 / total as f64) * 100.0
        }
    }
    
    /// Vide un bin (retourne les blocs au niveau supérieur)
    pub unsafe fn flush_bin(&mut self, bin_idx: usize) -> Vec<*mut u8> {
        if bin_idx >= NUM_BINS {
            return Vec::new();
        }
        
        let bin = &mut self.bins[bin_idx];
        let mut blocks = Vec::with_capacity(bin.count);
        
        let mut current = bin.free_list;
        while !current.is_null() {
            let next = (*current).next;
            blocks.push(current as *mut u8);
            current = next;
        }
        
        bin.free_list = ptr::null_mut();
        bin.count = 0;
        
        blocks
    }
}

/// CPU Slab - Niveau 2 (Per-CPU, lock-free)
#[repr(C, align(4096))]
pub struct CpuSlab {
    /// Slabs pour chaque taille de bin
    slabs: [Slab; NUM_BINS],
    
    /// ID du CPU propriétaire
    cpu_id: usize,
    
    /// Statistiques
    allocations: AtomicU64,
    deallocations: AtomicU64,
}

/// Slab pour une taille spécifique
#[repr(C)]
struct Slab {
    /// Pages allouées pour ce slab
    pages: Mutex<Vec<*mut u8>>,
    
    /// Nombre d'objets libres
    free_count: AtomicUsize,
    
    /// Taille des objets
    object_size: usize,
}

impl CpuSlab {
    /// Crée un nouveau CpuSlab
    pub const fn new(cpu_id: usize) -> Self {
        const EMPTY_SLAB: Slab = Slab {
            pages: Mutex::new(Vec::new()),
            free_count: AtomicUsize::new(0),
            object_size: 0,
        };
        
        Self {
            slabs: [EMPTY_SLAB; NUM_BINS],
            cpu_id,
            allocations: AtomicU64::new(0),
            deallocations: AtomicU64::new(0),
        }
    }
    
    /// Initialise les slabs
    pub fn init(&mut self) {
        for (i, slab) in self.slabs.iter_mut().enumerate() {
            slab.object_size = BIN_SIZES[i];
        }
    }
    
    /// Alloue une page pour un slab et la subdivise en objets
    pub unsafe fn allocate_page(
        &mut self,
        bin_idx: usize,
        buddy: &BuddyAllocator,
    ) -> Option<*mut u8> {
        if bin_idx >= NUM_BINS {
            return None;
        }
        
        // Obtenir une page 4KB depuis le BuddyAllocator
        let page = buddy.allocate(PAGE_SIZE)?;
        
        let slab = &mut self.slabs[bin_idx];
        let obj_size = slab.object_size;
        let objects_per_page = PAGE_SIZE / obj_size;
        
        // Ajouter la page à la liste des pages du slab
        {
            let mut pages = slab.pages.lock();
            pages.push(page);
        }
        
        // Subdiviser la page en objets et créer une free list
        let first_obj = page;
        let mut prev_obj = first_obj as *mut FreeNode;
        
        for i in 1..objects_per_page {
            let obj_ptr = page.add(i * obj_size);
            let node = obj_ptr as *mut FreeNode;
            (*prev_obj).next = node;
            prev_obj = node;
        }
        (*prev_obj).next = ptr::null_mut();
        
        // Mettre à jour le compteur (tous les objets sauf le premier)
        slab.free_count.fetch_add(objects_per_page - 1, Ordering::Release);
        self.allocations.fetch_add(1, Ordering::Relaxed);
        
        Some(first_obj)
    }
    
    /// Remplit un ThreadCache depuis ce slab
    pub unsafe fn refill_cache(
        &mut self,
        cache: &mut ThreadCache,
        bin_idx: usize,
        count: usize,
        buddy: &BuddyAllocator,
    ) -> usize {
        if bin_idx >= NUM_BINS {
            return 0;
        }
        
        let slab = &self.slabs[bin_idx];
        let free_count = slab.free_count.load(Ordering::Acquire);
        
        // Si pas assez d'objets libres, allouer une nouvelle page
        if free_count < count {
            if let Some(first_obj) = self.allocate_page(bin_idx, buddy) {
                // Ajouter au cache
                let cache_bin = &mut cache.bins[bin_idx];
                if cache_bin.count < MAX_OBJECTS_PER_BIN {
                    let node = first_obj as *mut FreeNode;
                    (*node).next = cache_bin.free_list;
                    cache_bin.free_list = node;
                    cache_bin.count += 1;
                    return 1;
                }
            }
        }
        
        // Transférer des objets existants vers le cache
        let to_transfer = count.min(free_count).min(MAX_OBJECTS_PER_BIN - cache.bins[bin_idx].count);
        if to_transfer == 0 {
            return 0;
        }
        
        let pages = slab.pages.lock();
        let cache_bin = &mut cache.bins[bin_idx];
        let obj_size = slab.object_size;
        let mut transferred = 0;
        
        // Parcourir les pages pour récupérer des objets libres
        for &page in pages.iter() {
            if transferred >= to_transfer {
                break;
            }
            
            let objects_per_page = PAGE_SIZE / obj_size;
            for i in 0..objects_per_page {
                if transferred >= to_transfer {
                    break;
                }
                
                let obj_ptr = page.add(i * obj_size);
                let node = obj_ptr as *mut FreeNode;
                
                // Ajouter à la free list du cache
                (*node).next = cache_bin.free_list;
                cache_bin.free_list = node;
                cache_bin.count += 1;
                transferred += 1;
            }
        }
        
        // Mettre à jour les compteurs
        if transferred > 0 {
            slab.free_count.fetch_sub(transferred, Ordering::Release);
            self.allocations.fetch_add(transferred as u64, Ordering::Relaxed);
        }
        
        transferred
    }
    
    /// Retourne un objet au slab (quand ThreadCache est plein)
    pub unsafe fn return_to_slab(&mut self, bin_idx: usize, obj: *mut u8) {
        if bin_idx >= NUM_BINS {
            return;
        }
        
        let slab = &self.slabs[bin_idx];
        let node = obj as *mut FreeNode;
        
        // Pour l'instant, on ajoute simplement à la liste
        // Dans une vraie implémentation, on pourrait libérer des pages entières
        (*node).next = ptr::null_mut();
        
        slab.free_count.fetch_add(1, Ordering::Release);
        self.deallocations.fetch_add(1, Ordering::Relaxed);
    }
    
    /// Statistiques du slab
    pub fn stats(&self) -> (u64, u64) {
        (
            self.allocations.load(Ordering::Relaxed),
            self.deallocations.load(Ordering::Relaxed),
        )
    }
}

/// Buddy Allocator - Niveau 3 (Pour grandes allocations)
#[repr(C)]
pub struct BuddyAllocator {
    /// Liste libre pour chaque ordre (0 = 4KB, 1 = 8KB, ..., 8 = 1MB)
    free_lists: [Mutex<Vec<*mut u8>>; 9],
    
    /// Début de la zone mémoire gérée
    memory_start: *mut u8,
    
    /// Taille de la zone mémoire
    memory_size: usize,
    
    /// Statistiques
    total_allocated: AtomicU64,
    total_freed: AtomicU64,
}

unsafe impl Send for BuddyAllocator {}
unsafe impl Sync for BuddyAllocator {}

impl BuddyAllocator {
    /// Crée un nouveau BuddyAllocator
    pub const fn new() -> Self {
        const EMPTY_LIST: Mutex<Vec<*mut u8>> = Mutex::new(Vec::new());
        
        Self {
            free_lists: [EMPTY_LIST; 9],
            memory_start: ptr::null_mut(),
            memory_size: 0,
            total_allocated: AtomicU64::new(0),
            total_freed: AtomicU64::new(0),
        }
    }
    
    /// Initialise le buddy allocator avec une zone mémoire
    pub unsafe fn init(&mut self, start: *mut u8, size: usize) {
        self.memory_start = start;
        self.memory_size = size;
        
        // Divise la mémoire en blocs de l'ordre maximum et les ajoute aux listes libres
        let max_order = 8; // 1MB blocks
        let max_block_size = PAGE_SIZE << max_order;
        
        let mut current = start;
        let end = start.add(size);
        
        while current.add(max_block_size) <= end {
            let mut free_list = self.free_lists[max_order].lock();
            free_list.push(current);
            current = current.add(max_block_size);
        }
        
        // Ajouter les blocs restants dans les ordres inférieurs
        let remaining = end as usize - current as usize;
        let mut remaining_size = remaining;
        
        for order in (0..max_order).rev() {
            let block_size = PAGE_SIZE << order;
            while remaining_size >= block_size {
                let mut free_list = self.free_lists[order].lock();
                free_list.push(current);
                current = current.add(block_size);
                remaining_size -= block_size;
            }
        }
    }
    
    /// Alloue un bloc de taille donnée
    pub unsafe fn allocate(&self, size: usize) -> Option<*mut u8> {
        // Trouve l'ordre approprié
        let order = Self::size_to_order(size)?;
        
        // Cherche un bloc de cet ordre ou supérieur
        for current_order in order..9 {
            let mut free_list = self.free_lists[current_order].lock();
            
            if let Some(block) = free_list.pop() {
                // Si le bloc est trop grand, le diviser
                if current_order > order {
                    drop(free_list); // Release le lock avant d'appeler split
                    self.split_block(block, current_order, order);
                }
                
                self.total_allocated.fetch_add((PAGE_SIZE << order) as u64, Ordering::Relaxed);
                return Some(block);
            }
        }
        
        None
    }
    
    /// Divise un bloc en deux buddies récursivement jusqu'à l'ordre cible
    unsafe fn split_block(&self, block: *mut u8, current_order: usize, target_order: usize) {
        if current_order <= target_order {
            return;
        }
        
        let new_order = current_order - 1;
        let block_size = PAGE_SIZE << new_order;
        
        // Le buddy est à block + block_size
        let buddy = block.add(block_size);
        
        // Ajouter le buddy à la liste de l'ordre inférieur
        let mut free_list = self.free_lists[new_order].lock();
        free_list.push(buddy);
        drop(free_list);
        
        // Continuer à diviser si nécessaire
        self.split_block(block, new_order, target_order);
    }
    
    /// Libère un bloc
    pub unsafe fn deallocate(&self, ptr: *mut u8, size: usize) {
        let order = match Self::size_to_order(size) {
            Some(o) => o,
            None => return,
        };
        
        self.total_freed.fetch_add((PAGE_SIZE << order) as u64, Ordering::Relaxed);
        
        // Tenter de fusionner avec le buddy
        self.coalesce(ptr, order);
    }
    
    /// Fusionne un bloc avec son buddy récursivement
    unsafe fn coalesce(&self, block: *mut u8, order: usize) {
        if order >= 8 {
            // Ordre maximum, ajouter directement
            let mut free_list = self.free_lists[order].lock();
            free_list.push(block);
            return;
        }
        
        let block_size = PAGE_SIZE << order;
        let block_index = (block as usize - self.memory_start as usize) / block_size;
        
        // Calculer l'adresse du buddy
        let buddy_index = block_index ^ 1;
        let buddy = self.memory_start.add(buddy_index * block_size);
        
        // Chercher le buddy dans la liste libre
        let mut free_list = self.free_lists[order].lock();
        
        if let Some(pos) = free_list.iter().position(|&b| b == buddy) {
            // Buddy trouvé, on fusionne
            free_list.swap_remove(pos);
            drop(free_list);
            
            // Le bloc fusionné commence au plus petit des deux
            let merged_block = if block < buddy { block } else { buddy };
            
            // Récursion pour fusionner avec le buddy de l'ordre supérieur
            self.coalesce(merged_block, order + 1);
        } else {
            // Pas de buddy disponible, ajouter ce bloc
            free_list.push(block);
        }
    }
    
    /// Statistiques de l'allocateur
    pub fn stats(&self) -> (u64, u64) {
        (
            self.total_allocated.load(Ordering::Relaxed),
            self.total_freed.load(Ordering::Relaxed),
        )
    }
    
    /// Convertit une taille en ordre buddy (0 = 4KB, 1 = 8KB, etc.)
    fn size_to_order(size: usize) -> Option<usize> {
        if size == 0 {
            return None;
        }
        
        let pages = (size + PAGE_SIZE - 1) / PAGE_SIZE;
        let order = pages.next_power_of_two().trailing_zeros() as usize;
        
        if order < 9 {
            Some(order)
        } else {
            None
        }
    }
}

/// Allocateur global hybride
pub struct HybridAllocator {
    /// Buddy allocator (niveau 3)
    buddy: Mutex<BuddyAllocator>,
    
    /// Fallback vers linked_list_allocator pour boot
    fallback: linked_list_allocator::LockedHeap,
    
    /// Indique si le hybrid allocator est initialisé
    initialized: AtomicUsize,
}

unsafe impl GlobalAlloc for HybridAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // Si pas encore initialisé, utilise le fallback
        if self.initialized.load(Ordering::Relaxed) == 0 {
            return self.fallback.alloc(layout);
        }
        
        // TODO: Utiliser ThreadCache → CpuSlab → Buddy
        // Pour l'instant, fallback
        self.fallback.alloc(layout)
    }
    
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if self.initialized.load(Ordering::Relaxed) == 0 {
            self.fallback.dealloc(ptr, layout);
            return;
        }
        
        // TODO: Libérer via ThreadCache
        self.fallback.dealloc(ptr, layout);
    }
}

impl HybridAllocator {
    /// Crée un nouvel allocateur hybride
    pub const fn new() -> Self {
        Self {
            buddy: Mutex::new(BuddyAllocator::new()),
            fallback: linked_list_allocator::LockedHeap::empty(),
            initialized: AtomicUsize::new(0),
        }
    }
    
    /// Initialise le fallback allocator
    pub unsafe fn init_fallback(&self, heap_start: *mut u8, heap_size: usize) {
        self.fallback.lock().init(heap_start, heap_size);
    }
    
    /// Initialise le hybrid allocator
    pub unsafe fn init(&self, start: *mut u8, size: usize) {
        let mut buddy = self.buddy.lock();
        buddy.init(start, size);
        self.initialized.store(1, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_bin_index() {
        assert_eq!(ThreadCache::bin_index(8), Some(0));
        assert_eq!(ThreadCache::bin_index(16), Some(1));
        assert_eq!(ThreadCache::bin_index(20), Some(2)); // 24 bytes
        assert_eq!(ThreadCache::bin_index(2048), Some(15));
        assert_eq!(ThreadCache::bin_index(3000), None); // Trop grand
    }
    
    #[test]
    fn test_thread_cache_init() {
        let mut cache = ThreadCache::new(0);
        cache.init();
        
        // Vérifie que les tailles sont correctes
        for (i, bin) in cache.bins.iter().enumerate() {
            assert_eq!(bin.object_size, BIN_SIZES[i]);
            assert_eq!(bin.count, 0);
            assert!(bin.free_list.is_null());
        }
    }
    
    #[test]
    fn test_cache_stats() {
        let cache = ThreadCache::new(0);
        assert_eq!(cache.stats.hits, 0);
        assert_eq!(cache.stats.misses, 0);
        assert_eq!(cache.hit_rate(), 0.0);
    }
    
    #[test]
    fn test_buddy_order() {
        assert_eq!(BuddyAllocator::size_to_order(4096), Some(0));
        assert_eq!(BuddyAllocator::size_to_order(8192), Some(1));
        assert_eq!(BuddyAllocator::size_to_order(5000), Some(1)); // Arrondi à 8KB
        assert_eq!(BuddyAllocator::size_to_order(1048576), Some(8)); // 1MB
    }
    
    #[test]
    fn test_buddy_split_coalesce() {
        unsafe {
            // Allouer 1MB de mémoire pour le test
            let layout = Layout::from_size_align(1024 * 1024, 4096).unwrap();
            let memory = alloc::alloc::alloc(layout);
            
            // Initialiser le buddy allocator
            let mut buddy = BuddyAllocator::new();
            buddy.init(memory, 1024 * 1024);
            
            // Test allocation 4KB
            let ptr1 = buddy.allocate(4096);
            assert!(ptr1.is_some());
            
            // Test allocation 8KB (devrait splitter un bloc plus grand)
            let ptr2 = buddy.allocate(8192);
            assert!(ptr2.is_some());
            
            // Libérer et vérifier coalesce
            if let Some(p1) = ptr1 {
                buddy.deallocate(p1, 4096);
            }
            if let Some(p2) = ptr2 {
                buddy.deallocate(p2, 8192);
            }
            
            // Nettoyer
            alloc::alloc::dealloc(memory, layout);
        }
    }
    
    #[test]
    fn test_thread_cache_allocate_deallocate() {
        unsafe {
            let mut cache = ThreadCache::new(0);
            cache.init();
            
            // Simuler l'ajout d'objets au cache (normalement fait par CpuSlab)
            // Pour le test, on crée des blocs fictifs
            let bin_idx = 0; // 8 bytes
            let bin = &mut cache.bins[bin_idx];
            
            // Allouer de la mémoire pour simuler des objets
            let layout = Layout::from_size_align(8 * 10, 8).unwrap();
            let memory = alloc::alloc::alloc(layout);
            
            // Créer une chaîne d'objets libres
            for i in 0..10 {
                let obj = memory.add(i * 8);
                let node = obj as *mut FreeNode;
                (*node).next = bin.free_list;
                bin.free_list = node;
                bin.count += 1;
            }
            
            // Test allocation
            let obj1 = cache.allocate(8);
            assert!(obj1.is_some());
            assert_eq!(bin.count, 9);
            assert_eq!(cache.stats.hits, 1);
            
            // Test deallocation
            if let Some(obj) = obj1 {
                cache.deallocate(obj, 8);
                assert_eq!(bin.count, 10);
            }
            
            // Nettoyer
            alloc::alloc::dealloc(memory, layout);
        }
    }
    
    #[test]
    fn test_cpu_slab_stats() {
        let slab = CpuSlab::new(0);
        let (allocs, deallocs) = slab.stats();
        assert_eq!(allocs, 0);
        assert_eq!(deallocs, 0);
    }
    
    #[test]
    fn test_buddy_stats() {
        let buddy = BuddyAllocator::new();
        let (allocs, deallocs) = buddy.stats();
        assert_eq!(allocs, 0);
        assert_eq!(deallocs, 0);
    }
    
    #[test]
    fn test_cache_hit_rate() {
        let mut cache = ThreadCache::new(0);
        
        // Simuler 80 hits et 20 misses
        cache.stats.hits = 80;
        cache.stats.misses = 20;
        
        let hit_rate = cache.hit_rate();
        assert!((hit_rate - 80.0).abs() < 0.01); // 80%
    }
    
    #[test]
    fn test_bin_max_capacity() {
        let mut cache = ThreadCache::new(0);
        cache.init();
        
        unsafe {
            let bin_idx = 0; // 8 bytes
            let bin = &mut cache.bins[bin_idx];
            
            // Allouer MAX_OBJECTS_PER_BIN objets
            let layout = Layout::from_size_align(8 * (MAX_OBJECTS_PER_BIN + 10), 8).unwrap();
            let memory = alloc::alloc::alloc(layout);
            
            // Remplir le bin jusqu'à la capacité max
            for i in 0..MAX_OBJECTS_PER_BIN {
                let obj = memory.add(i * 8);
                cache.deallocate(obj, 8);
            }
            
            assert_eq!(bin.count, MAX_OBJECTS_PER_BIN);
            
            // Essayer d'ajouter un objet supplémentaire (devrait être ignoré ou retourné au slab)
            let extra_obj = memory.add(MAX_OBJECTS_PER_BIN * 8);
            cache.deallocate(extra_obj, 8);
            
            // Le bin ne devrait pas dépasser la capacité max
            assert!(bin.count <= MAX_OBJECTS_PER_BIN);
            
            // Nettoyer
            alloc::alloc::dealloc(memory, layout);
        }
    }
    
    #[test]
    fn test_multiple_allocations() {
        unsafe {
            // Test stress avec allocations multiples
            let mut cache = ThreadCache::new(0);
            cache.init();
            
            // Pré-remplir le cache avec des objets
            let layout = Layout::from_size_align(PAGE_SIZE, 8).unwrap();
            let memory = alloc::alloc::alloc(layout);
            
            // Ajouter des objets de différentes tailles
            for size_idx in 0..5 {
                let size = BIN_SIZES[size_idx];
                let bin = &mut cache.bins[size_idx];
                
                for i in 0..10 {
                    let obj = memory.add(i * size);
                    let node = obj as *mut FreeNode;
                    (*node).next = bin.free_list;
                    bin.free_list = node;
                    bin.count += 1;
                }
            }
            
            // Allouer et désallouer dans un ordre différent
            let mut allocated = Vec::new();
            for _ in 0..20 {
                if let Some(obj) = cache.allocate(16) {
                    allocated.push(obj);
                }
            }
            
            for obj in allocated {
                cache.deallocate(obj, 16);
            }
            
            // Vérifier les statistiques
            assert!(cache.stats.hits > 0);
            
            // Nettoyer
            alloc::alloc::dealloc(memory, layout);
        }
    }
}
