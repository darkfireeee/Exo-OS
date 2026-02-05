//! Example: Using Slab allocator for fixed-size objects

use exo_allocator::SlabAllocator;

fn main() {
    // Create slab for 64-byte objects
    let slab = SlabAllocator::new(64, 1024);
    
    println!("Slab allocator created:");
    println!("  Object size: {} bytes", slab.object_size());
    println!("  Capacity: {} objects", slab.capacity());
    println!("  Total memory: {} bytes", slab.object_size() * slab.capacity());
}
