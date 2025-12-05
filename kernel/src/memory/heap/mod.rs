//! Heap allocator
//! 
//! Implémente un allocateur hybride 3-niveaux:
//! - Thread-local cache (≤256B, ~8 cycles)
//! - CPU slab (≤4KB, ~50 cycles)
//! - Buddy allocator (>4KB, ~200 cycles)
//! 
//! INTERRUPT SAFETY: Utilise InterruptGuard pour désactiver les interrupts
//! pendant les critical sections et éviter les deadlocks avec timer IRQ

pub mod thread_cache;
pub mod cpu_slab;
pub mod size_class;
pub mod statistics;
pub mod hybrid_allocator;

use core::alloc::{GlobalAlloc, Layout};
use core::ptr::{self, NonNull};
use core::mem;
use spin::Mutex;

/// RAII guard qui désactive les interrupts pendant sa durée de vie
/// Utilisé pour protéger les critical sections de l'allocateur
struct InterruptGuard {
    were_enabled: bool,
}

impl InterruptGuard {
    #[inline]
    fn new() -> Self {
        let were_enabled = are_interrupts_enabled();
        if were_enabled {
            disable_interrupts();
        }
        Self { were_enabled }
    }
}

impl Drop for InterruptGuard {
    #[inline]
    fn drop(&mut self) {
        if self.were_enabled {
            enable_interrupts();
        }
    }
}

/// Vérifie si les interrupts sont activés (bit IF dans RFLAGS)
#[inline]
fn are_interrupts_enabled() -> bool {
    let rflags: u64;
    unsafe {
        core::arch::asm!("pushfq; pop {}", out(reg) rflags, options(nomem, preserves_flags));
    }
    rflags & 0x200 != 0 // IF bit
}

/// Désactive les interrupts (CLI)
#[inline]
fn disable_interrupts() {
    unsafe {
        core::arch::asm!("cli", options(nomem, nostack));
    }
}

/// Active les interrupts (STI)
#[inline]
fn enable_interrupts() {
    unsafe {
        core::arch::asm!("sti", options(nomem, nostack));
    }
}

/// Taille minimale d'un bloc (doit contenir au moins ListNode)
const MIN_BLOCK_SIZE: usize = mem::size_of::<ListNode>();

/// Nœud de la liste chaînée des blocs libres
struct ListNode {
    size: usize,
    next: Option<NonNull<ListNode>>,
}

impl ListNode {
    const fn new(size: usize) -> Self {
        ListNode { size, next: None }
    }

    fn start_addr(&self) -> usize {
        self as *const Self as usize
    }

    fn end_addr(&self) -> usize {
        self.start_addr() + self.size
    }
}

/// Heap allocator principal
pub struct Heap {
    head: Option<NonNull<ListNode>>,
    heap_start: usize,
    heap_size: usize,
    allocated: usize,
}

// Heap is safe to send between threads because it's always protected by a Mutex
unsafe impl Send for Heap {}

impl Heap {
    /// Crée un heap vide
    pub const fn empty() -> Self {
        Heap {
            head: None,
            heap_start: 0,
            heap_size: 0,
            allocated: 0,
        }
    }

    /// Initialise le heap avec une région mémoire
    /// 
    /// # Safety
    /// La région [heap_start, heap_start + heap_size) doit être valide et non utilisée
    pub unsafe fn init(&mut self, heap_start: usize, heap_size: usize) {
        self.heap_start = heap_start;
        self.heap_size = heap_size;
        self.allocated = 0;

        // Créer le premier nœud libre couvrant tout le heap
        let node_ptr = heap_start as *mut ListNode;
        node_ptr.write(ListNode::new(heap_size));
        self.head = Some(NonNull::new_unchecked(node_ptr));
    }

    /// Alloue un bloc de mémoire
    pub fn allocate(&mut self, layout: Layout) -> Result<NonNull<u8>, ()> {
        // INTERRUPT SAFETY: Disable interrupts during allocation to prevent deadlock
        let _interrupt_guard = InterruptGuard::new();
        
        // Ajuster la taille pour l'alignement et la taille minimale
        let size = layout.size().max(MIN_BLOCK_SIZE);
        let size = align_up(size, layout.align());
        
        // Validation: Check for overflow and reasonable size
        if size > self.heap_size || size == 0 {
            return Err(());
        }

        // Chercher un bloc libre assez grand (first-fit)
        if let Some((region, alloc_start)) = self.find_region(size, layout.align()) {
            let alloc_end = alloc_start + size;

            // Retirer le bloc de la liste et potentiellement créer un nouveau bloc
            // pour le reste de l'espace
            let excess_size = region.end_addr() - alloc_end;
            // FIX CRITIQUE : Ne créer un nouveau nœud que s'il y a assez d'espace (>= MIN_BLOCK_SIZE)
            // et que l'adresse est correctement alignée
            let node_align = core::mem::align_of::<ListNode>();
            let aligned_alloc_end = align_up(alloc_end, node_align);
            let adjusted_excess = region.end_addr().saturating_sub(aligned_alloc_end);
            
            if adjusted_excess >= MIN_BLOCK_SIZE && aligned_alloc_end < region.end_addr() {
                // Il reste de l'espace, créer un nouveau nœud
                let new_node = ListNode::new(adjusted_excess);
                unsafe {
                    let new_node_ptr = aligned_alloc_end as *mut ListNode;
                    new_node_ptr.write(new_node);
                    self.insert_node(NonNull::new_unchecked(new_node_ptr));
                }
            }

            self.allocated += size;
            Ok(NonNull::new(alloc_start as *mut u8).unwrap())
        } else {
            Err(())
        }
    }

    /// Désalloue un bloc de mémoire
    /// 
    /// # Safety
    /// ptr doit avoir été alloué avec ce heap allocator
    pub unsafe fn deallocate(&mut self, ptr: NonNull<u8>, layout: Layout) {
        // INTERRUPT SAFETY: Disable interrupts during deallocation
        let _interrupt_guard = InterruptGuard::new();
        
        let size = layout.size().max(MIN_BLOCK_SIZE);
        let size = align_up(size, layout.align());
        
        // Validation: Verify pointer is within heap bounds
        let ptr_addr = ptr.as_ptr() as usize;
        if ptr_addr < self.heap_start || ptr_addr >= self.heap_start + self.heap_size {
            // Invalid pointer - could panic here, but for robustness just return
            // In production, should log error
            return;
        }

        // Créer un nouveau nœud libre
        let new_node = ListNode::new(size);
        let new_node_ptr = ptr.as_ptr() as *mut ListNode;
        new_node_ptr.write(new_node);
        
        self.insert_node(NonNull::new_unchecked(new_node_ptr));
        
        // Update allocated counter with saturation to prevent underflow
        self.allocated = self.allocated.saturating_sub(size);
    }

    /// Trouve une région libre assez grande
    fn find_region(&mut self, size: usize, align: usize) -> Option<(&'static mut ListNode, usize)> {
        let mut current = self.head;
        let mut prev: Option<NonNull<ListNode>> = None;

        while let Some(mut node_ptr) = current {
            let node = unsafe { node_ptr.as_mut() };
            
            // Calculer l'adresse de début alignée
            let alloc_start = align_up(node.start_addr(), align);
            let alloc_end = alloc_start.saturating_add(size);

            // FIX : Vérifier que l'allocation tient dans le nœud avec alignement
            if alloc_start >= node.start_addr() && alloc_end <= node.end_addr() {
                // Bloc trouvé ! Le retirer de la liste
                let next = node.next;
                
                if let Some(mut prev_ptr) = prev {
                    unsafe { prev_ptr.as_mut().next = next; }
                } else {
                    self.head = next;
                }
                
                return Some((node, alloc_start));
            }

            // Continuer avec le nœud suivant
            prev = Some(node_ptr);
            current = node.next;
        }

        None
    }

    /// Insère un nœud dans la liste chaînée (triée par adresse)
    unsafe fn insert_node(&mut self, mut new_node: NonNull<ListNode>) {
        let new_node_ref = new_node.as_mut();

        if self.head.is_none() {
            new_node_ref.next = None;
            self.head = Some(new_node);
            return;
        }

        // Chercher la position d'insertion
        let mut current = self.head;
        let mut prev: Option<NonNull<ListNode>> = None;

        while let Some(mut node_ptr) = current {
            let node = node_ptr.as_mut();
            
            if new_node_ref.start_addr() < node.start_addr() {
                // Insérer avant ce nœud
                new_node_ref.next = Some(node_ptr);
                
                if let Some(mut prev_ptr) = prev {
                    prev_ptr.as_mut().next = Some(new_node);
                } else {
                    self.head = Some(new_node);
                }

                // Fusionner avec les nœuds adjacents si possible
                self.try_merge(new_node);
                return;
            }

            prev = Some(node_ptr);
            current = node.next;
        }

        // Ajouter à la fin
        if let Some(mut prev_ptr) = prev {
            prev_ptr.as_mut().next = Some(new_node);
            new_node_ref.next = None;
            self.try_merge(new_node);
        }
    }

    /// Tente de fusionner un nœud avec ses voisins
    unsafe fn try_merge(&mut self, mut node: NonNull<ListNode>) {
        let node_ref = node.as_mut();

        // Fusionner avec le nœud suivant si adjacent
        if let Some(mut next_ptr) = node_ref.next {
            let next = next_ptr.as_mut();
            if node_ref.end_addr() == next.start_addr() {
                // Fusionner
                node_ref.size += next.size;
                node_ref.next = next.next;
            }
        }
    }

    /// Retourne les statistiques du heap
    pub fn stats(&self) -> HeapStats {
        HeapStats {
            total_size: self.heap_size,
            allocated: self.allocated,
            free: self.heap_size - self.allocated,
        }
    }
}

/// Statistiques du heap
#[derive(Debug, Clone, Copy)]
pub struct HeapStats {
    pub total_size: usize,
    pub allocated: usize,
    pub free: usize,
}

/// Wrapper thread-safe pour le heap
pub struct LockedHeap {
    inner: Mutex<Heap>,
}

impl LockedHeap {
    pub const fn empty() -> Self {
        LockedHeap {
            inner: Mutex::new(Heap::empty()),
        }
    }

    /// Initialise le heap
    /// 
    /// # Safety
    /// La région [heap_start, heap_start + heap_size) doit être valide
    pub unsafe fn init(&self, heap_start: usize, heap_size: usize) {
        self.inner.lock().init(heap_start, heap_size);
    }

    /// Retourne les statistiques du heap
    pub fn stats(&self) -> HeapStats {
        self.inner.lock().stats()
    }
}

unsafe impl GlobalAlloc for LockedHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.inner
            .lock()
            .allocate(layout)
            .ok()
            .map_or(ptr::null_mut(), |ptr| ptr.as_ptr())
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if let Some(non_null) = NonNull::new(ptr) {
            self.inner.lock().deallocate(non_null, layout);
        }
    }
}

/// Aligne une valeur vers le haut
fn align_up(addr: usize, align: usize) -> usize {
    (addr + align - 1) & !(align - 1)
}
