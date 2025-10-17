//! Allocateur de tas pour le noyau utilisant le système buddy
//! 
//! Ce module implémente un allocateur de tas basé sur le système buddy,
//! qui est efficace pour gérer des allocations de tailles variées avec
/// une fragmentation minimale.

use alloc::alloc::{GlobalAlloc, Layout};
use core::ptr::NonNull;
use spin::Mutex;

/// Taille minimale d'un bloc d'allocation (16 octets)
const MIN_BLOCK_SIZE: usize = 16;

/// Nombre maximum de niveaux dans l'arbre buddy (pour 4 GiB de mémoire)
const MAX_LEVELS: usize = 28; // log2(4GiB / 16B)

/// Structure représentant un bloc de mémoire
#[derive(Debug)]
struct Block {
    /// Pointeur vers le début du bloc
    ptr: NonNull<u8>,
    /// Taille du bloc en octets
    size: usize,
    /// Indique si le bloc est libre
    free: bool,
    /// Niveau dans l'arbre buddy
    level: usize,
    /// Pointeur vers le bloc buddy
    buddy: Option<*mut Block>,
}

/// Structure représentant l'allocateur de tas buddy
pub struct BuddyHeapAllocator {
    /// Racine de l'arbre buddy
    root: Option<*mut Block>,
    /// Listes de blocs libres pour chaque niveau
    free_lists: [Option<*mut Block>; MAX_LEVELS],
    /// Taille totale du tas
    heap_size: usize,
    /// Espace de mémoire disponible pour le tas
    heap_space: Option<NonNull<u8>>,
}

impl BuddyHeapAllocator {
    /// Crée un nouvel allocateur de tas buddy
    /// 
    /// # Returns
    /// 
    /// Une nouvelle instance de `BuddyHeapAllocator`
    pub fn new() -> Self {
        Self {
            root: None,
            free_lists: [None; MAX_LEVELS],
            heap_size: 0,
            heap_space: None,
        }
    }
    
    /// Initialise le tas avec une plage de mémoire
    /// 
    /// # Arguments
    /// 
    /// * `heap_start` - Début de la plage de mémoire
    /// * `heap_size` - Taille de la plage de mémoire
    pub fn init(&mut self, heap_start: NonNull<u8>, heap_size: usize) {
        self.heap_space = Some(heap_start);
        self.heap_size = heap_size;
        
        // Calculer le niveau de la racine
        let root_level = (heap_size.next_power_of_two().trailing_zeros() as usize) - (MIN_BLOCK_SIZE.trailing_zeros() as usize);
        
        // Créer le bloc racine
        let root_block = unsafe {
            let layout = Layout::new::<Block>();
            let ptr = alloc::alloc::alloc(layout);
            if ptr.is_null() {
                panic!("failed to allocate memory for root block");
            }
            let block = ptr as *mut Block;
            (*block).ptr = heap_start;
            (*block).size = heap_size;
            (*block).free = true;
            (*block).level = root_level;
            (*block).buddy = None;
            block
        };
        
        self.root = Some(root_block);
        self.free_lists[root_level] = Some(root_block);
    }
    
    /// Alloue un bloc de mémoire de la taille spécifiée
    /// 
    /// # Arguments
    /// 
    /// * `size` - Taille du bloc à allouer
    /// 
    /// # Returns
    /// 
    /// Un pointeur vers le bloc alloué, ou `None` si l'allocation échoue
    fn allocate_block(&mut self, size: usize) -> Option<*mut Block> {
        if size == 0 {
            return None;
        }
        
        // Calculer le niveau requis pour cette taille
        let required_level = (size.next_power_of_two().trailing_zeros() as usize) - (MIN_BLOCK_SIZE.trailing_zeros() as usize);
        
        // Trouver un bloc libre de niveau approprié
        let mut level = required_level;
        while level < MAX_LEVELS && self.free_lists[level].is_none() {
            level += 1;
        }
        
        if level >= MAX_LEVELS {
            return None; // Pas de bloc disponible
        }
        
        // Récupérer le bloc libre
        let block = self.free_lists[level].unwrap();
        self.free_lists[level] = None;
        
        // Diviser le bloc si nécessaire
        let mut current_level = level;
        while current_level > required_level {
            let split_result = self.split_block(block);
            if split_result.is_none() {
                // Impossible de diviser le bloc, le remettre dans la liste libre
                self.free_lists[current_level] = Some(block);
                return None;
            }
            
            let (left, right) = split_result.unwrap();
            self.free_lists[current_level - 1] = Some(right);
            current_level -= 1;
        }
        
        // Marquer le bloc comme utilisé
        unsafe {
            (*block).free = false;
        }
        
        Some(block)
    }
    
    /// Divise un bloc en deux blocs buddy
    /// 
    /// # Arguments
    /// 
    /// * `block` - Bloc à diviser
    /// 
    /// # Returns
    /// 
    /// Un tuple contenant les deux blocs résultants, ou `None` si la division échoue
    fn split_block(&mut self, block: *mut Block) -> Option<(*mut Block, *mut Block)> {
        unsafe {
            let block_size = (*block).size / 2;
            let block_level = (*block).level - 1;
            
            // Allouer de la mémoire pour les nouveaux blocs
            let layout = Layout::new::<Block>();
            let left_ptr = alloc::alloc::alloc(layout) as *mut Block;
            let right_ptr = alloc::alloc::alloc(layout) as *mut Block;
            
            if left_ptr.is_null() || right_ptr.is_null() {
                // Libérer la mémoire allouée si une des allocations échoue
                if !left_ptr.is_null() {
                    alloc::alloc::dealloc(left_ptr as *mut u8, layout);
                }
                if !right_ptr.is_null() {
                    alloc::alloc::dealloc(right_ptr as *mut u8, layout);
                }
                return None;
            }
            
            // Initialiser le bloc gauche
            (*left_ptr).ptr = (*block).ptr;
            (*left_ptr).size = block_size;
            (*left_ptr).free = true;
            (*left_ptr).level = block_level;
            (*left_ptr).buddy = Some(right_ptr);
            
            // Initialiser le bloc droit
            (*right_ptr).ptr = NonNull::new_unchecked((*block).ptr.as_ptr().add(block_size));
            (*right_ptr).size = block_size;
            (*right_ptr).free = true;
            (*right_ptr).level = block_level;
            (*right_ptr).buddy = Some(left_ptr);
            
            // Libérer l'ancien bloc
            alloc::alloc::dealloc(block as *mut u8, layout);
            
            Some((left_ptr, right_ptr))
        }
    }
    
    /// Libère un bloc de mémoire
    /// 
    /// # Arguments
    /// 
    /// * `block` - Bloc à libérer
    fn deallocate_block(&mut self, block: *mut Block) {
        unsafe {
            (*block).free = true;
            
            // Essayer de fusionner avec le buddy
            let mut current_block = block;
            while (*current_block).level < MAX_LEVELS - 1 {
                let buddy = (*current_block).buddy;
                if buddy.is_none() || !(*buddy.unwrap()).free {
                    break; // Le buddy n'est pas libre, impossible de fusionner
                }
                
                // Fusionner avec le buddy
                let merged_block = self.merge_blocks(current_block, buddy.unwrap());
                self.free_lists[(*current_block).level] = None;
                self.free_lists[(*current_block).level] = None;
                current_block = merged_block;
            }
            
            // Ajouter le bloc à la liste libre appropriée
            self.free_lists[(*current_block).level] = Some(current_block);
        }
    }
    
    /// Fusionne deux blocs buddy en un seul bloc
    /// 
    /// # Arguments
    /// 
    /// * `left` - Premier bloc
    /// * `right` - Second bloc
    /// 
    /// # Returns
    /// 
    /// Un pointeur vers le bloc fusionné
    fn merge_blocks(&mut self, left: *mut Block, right: *mut Block) -> *mut Block {
        unsafe {
            // Allouer de la mémoire pour le bloc fusionné
            let layout = Layout::new::<Block>();
            let merged_ptr = alloc::alloc::alloc(layout) as *mut Block;
            
            if merged_ptr.is_null() {
                panic!("failed to allocate memory for merged block");
            }
            
            // Le bloc fusionné pointe vers le début du bloc gauche
            (*merged_ptr).ptr = (*left).ptr;
            (*merged_ptr).size = (*left).size * 2;
            (*merged_ptr).free = true;
            (*merged_ptr).level = (*left).level + 1;
            (*merged_ptr).buddy = None;
            
            // Libérer les anciens blocs
            alloc::alloc::dealloc(left as *mut u8, layout);
            alloc::alloc::dealloc(right as *mut u8, layout);
            
            merged_ptr
        }
    }
}

unsafe impl GlobalAlloc for BuddyHeapAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size = layout.size();
        let align = layout.align();
        
        // S'assurer que la taille est au moins la taille minimale d'un bloc
        let size = size.max(MIN_BLOCK_SIZE);
        
        // Allouer un bloc
        let allocator = &mut *(self as *const BuddyHeapAllocator as *mut BuddyHeapAllocator);
        let block = allocator.allocate_block(size);
        
        if block.is_none() {
            return core::ptr::null_mut();
        }
        
        let block = block.unwrap();
        let ptr = (*block).ptr.as_ptr();
        
        // Aligner le pointeur si nécessaire
        if align > MIN_BLOCK_SIZE {
            let offset = ptr.align_offset(align);
            if offset < (*block).size {
                ptr.add(offset)
            } else {
                core::ptr::null_mut()
            }
        } else {
            ptr
        }
    }
    
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let size = layout.size();
        
        // S'assurer que la taille est au moins la taille minimale d'un bloc
        let size = size.max(MIN_BLOCK_SIZE);
        
        // Trouver le bloc correspondant à ce pointeur
        let allocator = &mut *(self as *const BuddyHeapAllocator as *mut BuddyHeapAllocator);
        
        // Pour simplifier, nous supposons que nous pouvons trouver le bloc
        // Dans une implémentation réelle, il faudrait une structure de données
        // pour suivre les allocations
        if let Some(root) = allocator.root {
            let block = allocator.find_block_for_ptr(root, ptr);
            if !block.is_null() {
                allocator.deallocate_block(block);
            }
        }
    }
}

impl BuddyHeapAllocator {
    /// Trouve le bloc correspondant à un pointeur
    /// 
    /// # Arguments
    /// 
    /// * `block` - Bloc de départ pour la recherche
    /// * `ptr` - Pointeur à rechercher
    /// 
    /// # Returns
    /// 
    /// Un pointeur vers le bloc correspondant, ou un pointeur nul si non trouvé
    fn find_block_for_ptr(&self, block: *mut Block, ptr: *mut u8) -> *mut Block {
        unsafe {
            if (*block).free {
                return core::ptr::null_mut();
            }
            
            let block_start = (*block).ptr.as_ptr();
            let block_end = block_start.add((*block).size);
            
            if ptr >= block_start && ptr < block_end {
                return block;
            }
            
            // Rechercher dans les sous-blocs
            if let Some(buddy) = (*block).buddy {
                let result = self.find_block_for_ptr(buddy, ptr);
                if !result.is_null() {
                    return result;
                }
            }
            
            core::ptr::null_mut()
        }
    }
}

/// Allocateur global pour le tas du noyau
#[global_allocator]
pub static HEAP_ALLOCATOR: Mutex<BuddyHeapAllocator> = Mutex::new(BuddyHeapAllocator::new());

/// Initialise le tas du noyau
/// 
/// # Arguments
/// 
/// * `heap_start` - Début de la plage de mémoire pour le tas
/// * `heap_size` - Taille de la plage de mémoire pour le tas
pub fn init_heap(heap_start: NonNull<u8>, heap_size: usize) {
    HEAP_ALLOCATOR.lock().init(heap_start, heap_size);
}

/// Fonction de gestion des erreurs d'allocation
#[alloc_error_handler]
fn alloc_error_handler(layout: Layout) -> ! {
    panic!("allocation error: {:?}", layout)
}