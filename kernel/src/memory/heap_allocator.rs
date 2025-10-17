//! Allocateur de tas (stub)
//! 
//! Le tas est géré par linked_list_allocator dans lib.rs

pub fn init_heap(_heap_start: core::ptr::NonNull<u8>, _heap_size: usize) {
    // TODO: Utiliser un allocateur buddy avancé
}
