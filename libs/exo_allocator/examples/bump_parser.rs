//! Example: Using Bump allocator for temporary data

use exo_allocator::BumpAllocator;

fn main() {
    let bump = BumpAllocator::with_capacity(4096);

    println!("Bump allocator created:");
    println!("  Capacity: {} bytes", bump.capacity());
    println!("  Used: {} bytes", bump.used());
}
