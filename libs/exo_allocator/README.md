# exo_allocator

Custom memory allocators for Exo-OS userspace applications.

## Features

- **SlabAllocator** : Fixed-size object pools optimized for descriptors, tasks, and network buffers
- **BumpAllocator** : Arena allocator for temporary data (parsing, request handling)
- **MimallocWrapper** : GlobalAlloc implementation using Microsoft's mimalloc (high-performance general-purpose allocator)

## Architecture

```
exo_allocator/
├── src/
│   ├── lib.rs          # Public API exports
│   ├── slab.rs         # Slab allocator implementation
│   ├── bump.rs         # Bump/Arena allocator
│   ├── mimalloc.rs     # Mimalloc FFI bindings + GlobalAlloc
│   ├── telemetry.rs    # Allocation tracking hooks
│   └── oom.rs          # Out-of-memory handler
├── vendor/mimalloc/    # Microsoft mimalloc C sources
└── benches/            # Performance comparisons
```

## Usage

### Slab Allocator (Fixed-size objects)

```rust
use exo_allocator::Slab;

// Create slab for 64-byte objects
let slab = Slab::new(64, 1024); // 64 bytes, 1024 objects capacity

let ptr = slab.alloc().expect("Allocation failed");
// ... use object ...
unsafe { slab.free(ptr); }
```

### Bump Allocator (Arena/scratch space)

```rust
use exo_allocator::Bump;

let arena = Bump::with_capacity(4096);
let data = arena.alloc_slice(&[1, 2, 3, 4]);
// All allocations freed when arena drops
```

### Mimalloc as Global Allocator

```rust
use exo_allocator::Mimalloc;

#[global_allocator]
static GLOBAL: Mimalloc = Mimalloc;

fn main() {
    let v = vec![1, 2, 3]; // Uses mimalloc
}
```

## Benchmarks

```bash
cargo bench --package exo_allocator
```

Typical results (vs alternatives):
- **Slab**: 3-5x faster than general allocator for fixed-size objects
- **Bump**: 10-100x faster for temporary allocations
- **Mimalloc**: 10-30% faster than jemalloc, 2x faster than system allocator

## Safety

- Slab and Bump allocators are `unsafe` APIs (manual memory management)
- Mimalloc wrapper provides safe `GlobalAlloc` interface
- All allocators implement telemetry hooks for debugging leaks

## Performance Tuning

### Slab Configuration
- Object size should be power of 2 for best alignment
- Capacity determines initial memory reservation

### Mimalloc Features
- Compiled with `-DMI_SECURE=4` for security features
- Optimized for x86_64 with `-march=native`
- Large page support enabled (requires OS configuration)

## References

- [Mimalloc Paper](https://www.microsoft.com/en-us/research/publication/mimalloc-free-list-sharding-in-action/)
- [Slab Allocator Design](https://en.wikipedia.org/wiki/Slab_allocation)
