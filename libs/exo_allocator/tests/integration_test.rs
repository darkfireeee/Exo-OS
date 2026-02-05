use exo_allocator::*;

#[test]
fn test_slab_basic() {
    let slab = SlabAllocator::new(64, 1024);
    assert_eq!(slab.object_size(), 64);
    assert_eq!(slab.capacity(), 1024);
}

#[test]
fn test_bump_basic() {
    let bump = BumpAllocator::with_capacity(4096);
    assert_eq!(bump.capacity(), 4096);
}

#[test]
fn test_telemetry() {
    assert_eq!(Telemetry::total_allocated(), 0);
}
