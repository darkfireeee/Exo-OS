# 🧱 Heap Allocator

## Architecture

L'allocateur heap utilise une combinaison de:
1. **Slab allocator** pour les petites allocations (≤4KB)
2. **Buddy allocator** pour les grandes allocations (>4KB)

## Slab Allocator

### Classes de Taille

```rust
const SLAB_SIZES: [usize; 8] = [
    16,     // Class 0
    32,     // Class 1
    64,     // Class 2
    128,    // Class 3
    256,    // Class 4
    512,    // Class 5
    1024,   // Class 6
    2048,   // Class 7
];
```

### Structure Slab

```
┌─────────────────────────────────────────────────────────────┐
│                         Slab (4KB)                           │
├─────────────────────────────────────────────────────────────┤
│ Header │ Free List │ Object 0 │ Object 1 │ ... │ Object N  │
│ 64B    │ Bitmap    │          │          │     │           │
└─────────────────────────────────────────────────────────────┘
```

### API Interne

```rust
impl SlabAllocator {
    pub fn alloc(&mut self, size: usize) -> *mut u8 {
        let class = size_to_class(size);
        
        // Chercher un slab avec de l'espace
        if let Some(slab) = self.partial_slabs[class].pop() {
            return slab.alloc_object();
        }
        
        // Créer nouveau slab
        let slab = self.create_slab(class);
        slab.alloc_object()
    }
    
    pub fn free(&mut self, ptr: *mut u8) {
        let slab = Slab::from_ptr(ptr);
        slab.free_object(ptr);
        
        if slab.is_empty() {
            self.free_slabs[slab.class].push(slab);
        }
    }
}
```

## Buddy Allocator

### Concept

```
Order 0: 4KB   ████████████████████████████████
Order 1: 8KB   ████████████████ ████████████████
Order 2: 16KB  ████████ ████████ ████████ ████████
Order 3: 32KB  ████ ████ ████ ████ ████ ████ ████ ████
...
Order 9: 2MB   █ █
```

### Algorithme

```rust
impl BuddyAllocator {
    pub fn alloc(&mut self, size: usize) -> *mut u8 {
        let order = size_to_order(size);
        
        // Chercher un bloc libre de cet ordre
        for o in order..MAX_ORDER {
            if let Some(block) = self.free_lists[o].pop() {
                // Split si nécessaire
                while o > order {
                    let buddy = block.split();
                    self.free_lists[o - 1].push(buddy);
                    o -= 1;
                }
                return block.addr;
            }
        }
        
        None
    }
    
    pub fn free(&mut self, ptr: *mut u8, order: usize) {
        let mut block = Block::new(ptr, order);
        
        // Coalesce avec buddy si possible
        while block.order < MAX_ORDER {
            let buddy = block.buddy_addr();
            if self.is_free(buddy, block.order) {
                self.free_lists[block.order].remove(buddy);
                block = block.merge_with_buddy();
            } else {
                break;
            }
        }
        
        self.free_lists[block.order].push(block);
    }
}
```

## Global Allocator

```rust
#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

// Initialisation
pub fn init_heap(start: usize, size: usize) {
    unsafe {
        ALLOCATOR.lock().init(start, size);
    }
}
```

## Statistiques

```rust
pub struct HeapStats {
    pub total_bytes: usize,
    pub used_bytes: usize,
    pub free_bytes: usize,
    pub allocations: usize,
    pub deallocations: usize,
    pub fragmentation: f32,
}
```
