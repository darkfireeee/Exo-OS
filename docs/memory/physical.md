# 📦 Physical Memory Management

## Frame Allocator

### Bitmap Allocator

```rust
pub struct BitmapAllocator {
    /// Bitmap: 1 bit per 4KB frame
    bitmap: &'static mut [u64],
    
    /// Total frames
    total_frames: usize,
    
    /// Free frames count
    free_frames: AtomicUsize,
    
    /// Base physical address
    base: PhysicalAddress,
    
    /// Next search hint (buddy-style)
    next_hint: AtomicUsize,
}
```

### API

```rust
// Allouer une frame
let frame = frame_allocator::alloc_frame()?;

// Allouer frames contiguës
let frames = frame_allocator::alloc_contiguous(16)?;

// Libérer
frame_allocator::free_frame(frame);
```

### Zones de Mémoire

```rust
pub enum MemoryZone {
    /// DMA zone (< 16MB) - pour périphériques ISA
    Dma,
    /// DMA32 zone (< 4GB) - pour périphériques 32-bit
    Dma32,
    /// Normal zone (tout le reste)
    Normal,
}
```

## Statistiques

```rust
pub struct FrameStats {
    pub total: usize,
    pub free: usize,
    pub used: usize,
    pub reserved: usize,
}

let stats = frame_allocator::stats();
```
