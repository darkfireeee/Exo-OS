# MEMORY API SPECIFICATION

**État** : ✅ PHASES 1 & 2 COMPLÈTES (40%)  
**Responsable** : Copilot (Zone 2)  
**Début** : Maintenant  
**Temps écoulé** : 2 heures  
**ETA Completion totale** : 2-3 heures restantes  

---

## 📋 Objectif

Implémenter le système de gestion mémoire d'Exo-OS avec 3 composants :
1. **Physical Frame Allocator** (buddy system)
2. **Virtual Memory Manager** (4-level paging + TLB)
3. **Hybrid Kernel Allocator** (thread-local + CPU slab + buddy)

---

## 🎯 Phase 1 : Physical Frame Allocator (2h)

### Fichier : `kernel/src/memory/frame_allocator.rs`

```rust
// API publique cible
pub fn alloc_frame() -> Result<PhysAddr, MemoryError>;
pub fn free_frame(addr: PhysAddr) -> Result<(), MemoryError>;
pub fn alloc_contiguous(count: usize) -> Result<PhysAddr, MemoryError>;
pub fn free_contiguous(addr: PhysAddr, count: usize) -> Result<(), MemoryError>;
```

### Implémentation

**Buddy Allocator** :
- Ordres 0→12 : 4KB → 16MB (2^12 pages × 4KB = 16MB)
- Bitmap pour tracking : 1 bit par frame + listes chaînées par ordre
- Coalescing automatique lors du free()
- Stratégie : First-fit avec recherche d'ordre supérieur si épuisé

**Structure** :
```rust
pub struct BuddyAllocator {
    free_lists: [LinkedList<PhysAddr>; 13], // 0→12
    bitmap: Bitmap, // 1 bit par frame 4KB
    base_addr: PhysAddr,
    total_frames: usize,
}
```

**Algorithme alloc_frame()** :
1. Chercher frame libre dans ordre 0 (4KB)
2. Si vide, chercher ordre 1 (8KB) et split en 2×4KB
3. Remonter jusqu'à ordre 12 si nécessaire
4. Marquer frame utilisée dans bitmap
5. Retourner PhysAddr

**Algorithme free_frame()** :
1. Marquer frame libre dans bitmap
2. Chercher buddy frame (addr XOR (1 << order))
3. Si buddy libre, coalescer (fusionner en ordre+1)
4. Répéter récursivement jusqu'à buddy occupé ou ordre max

---

## 🎯 Phase 2 : Virtual Memory Manager (2h)

### Fichier : `kernel/src/memory/page_table.rs`

```rust
// API publique cible
pub fn map_page(virt: VirtAddr, phys: PhysAddr, flags: PageFlags) -> Result<(), MemoryError>;
pub fn unmap_page(virt: VirtAddr) -> Result<(), MemoryError>;
pub fn translate(virt: VirtAddr) -> Option<PhysAddr>;
pub fn flush_tlb(virt: VirtAddr);
pub fn flush_tlb_all();
```

### Implémentation

**4-Level Page Table Walker** :
- P4 (PML4) → P3 (PDPT) → P2 (PD) → P1 (PT) → Page 4KB
- Support 2MB huge pages (P2 → Page directement)
- Support 1GB huge pages (P3 → Page directement)

**Structure** :
```rust
pub struct PageTable {
    entries: [PageTableEntry; 512],
}

pub struct PageTableEntry(u64);

impl PageTableEntry {
    fn is_present(&self) -> bool;
    fn is_huge(&self) -> bool;
    fn set_addr(&mut self, addr: PhysAddr);
    fn set_flags(&mut self, flags: PageFlags);
}
```

**TLB Management** :
- PCID (Process Context ID) pour éviter flush global
- invlpg pour invalidation d'une seule page
- Compteur de shootdown TLB pour statistiques

**Algorithme map_page()** :
1. Décomposer VirtAddr en indices [P4][P3][P2][P1]
2. Walker P4 → P3 → P2 → P1 (créer tables si absentes)
3. Écrire PhysAddr + flags dans P1[index]
4. flush_tlb(virt) pour invalider cache

---

## 🎯 Phase 3 : Hybrid Kernel Allocator (2h)

### Fichier : `kernel/src/memory/heap_allocator.rs`

```rust
// API publique cible
pub unsafe fn kmalloc(size: usize) -> *mut u8;
pub unsafe fn kfree(ptr: *mut u8);
pub unsafe fn krealloc(ptr: *mut u8, new_size: usize) -> *mut u8;
```

### Implémentation

**3-Level Hybrid Strategy** :

1. **Thread-Local Slab (≤256 bytes, ~8 cycles)** :
   - Cache thread-local sans lock
   - Tailles fixes : 16, 32, 64, 128, 256 bytes
   - Free list par taille

2. **CPU Slab (≤4KB, ~50 cycles)** :
   - Cache per-CPU avec atomic lock
   - Tailles fixes : 512, 1024, 2048, 4096 bytes
   - Partage entre threads du même CPU

3. **Buddy Fallback (>4KB, ~200 cycles)** :
   - Utilise frame_allocator pour grandes allocations
   - Allocations contiguës multi-pages

**Structure** :
```rust
pub struct HybridAllocator {
    thread_local: ThreadLocalSlab,
    cpu_slab: CpuSlab,
    buddy: BuddyAllocator,
}

struct ThreadLocalSlab {
    free_lists: [FreeList; 5], // 16→256
}

struct CpuSlab {
    free_lists: [AtomicFreeList; 4], // 512→4096
}
```

**Algorithme kmalloc(size)** :
1. Round up size to next power of 2
2. Si ≤256 : chercher dans thread_local
3. Si miss ou >256 et ≤4KB : chercher dans cpu_slab
4. Si miss ou >4KB : appeler buddy allocator
5. Retourner pointeur

**Algorithme kfree(ptr)** :
1. Déterminer taille depuis metadata (ou size map)
2. Si ≤256 : retourner à thread_local
3. Si ≤4KB : retourner à cpu_slab
4. Si >4KB : retourner à buddy
5. Coalescing automatique dans buddy

---

## 🎯 Phase 4 : Documentation & Tests (1-2h)

### INTERFACES.md

Ajouter section "Memory API" :

```markdown
## MEMORY API

### Physical Frame Allocator
- `alloc_frame() -> Result<PhysAddr, MemoryError>`
- `free_frame(PhysAddr) -> Result<(), MemoryError>`
- `alloc_contiguous(count) -> Result<PhysAddr, MemoryError>`

### Virtual Memory Manager
- `map_page(virt, phys, flags) -> Result<(), MemoryError>`
- `unmap_page(virt) -> Result<(), MemoryError>`
- `translate(virt) -> Option<PhysAddr>`

### Kernel Allocator
- `kmalloc(size) -> *mut u8`
- `kfree(ptr)`
- `krealloc(ptr, new_size) -> *mut u8`

### Usage pour Gemini
```rust
// POSIX mmap() implementation
use crate::memory::{map_page, alloc_frame, PageFlags};

pub fn sys_mmap(addr: VirtAddr, len: usize, prot: i32) -> Result<VirtAddr, SyscallError> {
    let pages = (len + 4095) / 4096;
    for i in 0..pages {
        let phys = alloc_frame()?;
        let virt = addr + i * 4096;
        map_page(virt, phys, PageFlags::from_prot(prot))?;
    }
    Ok(addr)
}
```
```

### Tests Unitaires

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_buddy_alloc_free() {
        let frame = alloc_frame().unwrap();
        assert!(frame.is_aligned(4096));
        free_frame(frame).unwrap();
    }

    #[test]
    fn test_buddy_coalescing() {
        let f1 = alloc_frame().unwrap();
        let f2 = alloc_frame().unwrap();
        assert_eq!(f2, f1 + 4096); // Buddy adjacent
        free_frame(f1).unwrap();
        free_frame(f2).unwrap();
        // Devrait coalescer en bloc 8KB
    }

    #[test]
    fn test_page_mapping() {
        let virt = VirtAddr::new(0x1000_0000);
        let phys = alloc_frame().unwrap();
        map_page(virt, phys, PageFlags::WRITABLE).unwrap();
        assert_eq!(translate(virt), Some(phys));
        unmap_page(virt).unwrap();
    }

    #[test]
    fn test_kmalloc_thread_local() {
        let ptr = unsafe { kmalloc(64) };
        assert!(!ptr.is_null());
        unsafe { kfree(ptr); }
    }
}
```

### Benchmarks

```rust
#[bench]
fn bench_buddy_alloc(b: &mut Bencher) {
    b.iter(|| {
        let frame = alloc_frame().unwrap();
        free_frame(frame).unwrap();
    });
    // Target : <200 cycles
}

#[bench]
fn bench_kmalloc_small(b: &mut Bencher) {
    b.iter(|| {
        let ptr = unsafe { kmalloc(64) };
        unsafe { kfree(ptr); }
    });
    // Target : <8 cycles (thread-local hit)
}
```

---

## 📊 Critères de Succès

- [x] ✅ Phase 1 : Frame allocator compile sans erreur - COMPLET (600 lignes)
- [x] ✅ Phase 2 : Page mapper compile sans erreur - COMPLET (700 lignes)
- [ ] 🟡 Phase 3 : Kernel allocator - EXISTANT (linked-list simple, pas 3-levels)
- [ ] ⏳ Phase 4 : Tests unitaires passent (≥5 tests) - À FAIRE
- [ ] ⏳ Benchmarks : buddy <200 cycles, kmalloc small <10 cycles - À FAIRE
- [x] ✅ Documentation : INTERFACES.md mis à jour avec exemples - COMPLET
- [x] ✅ Gemini notifié : Memory API disponible dans STATUS_GEMINI - FAIT

---

## 🔗 Dépendances

**Requis maintenant** :
- Boot fonctionnel (✅ FAIT)
- Multiboot2 memory map parsed (✅ dans boot.c)

**Bloque actuellement** :
- POSIX-X mmap/brk (Gemini attend Memory API)
- IPC Fusion Rings (needs allocations)
- Scheduler (needs thread stacks)

**Débloquer après** :
- IPC (peut allouer ring buffers)
- Syscalls (peut allouer syscall tables)
- Gemini POSIX-X (peut implémenter mmap)

---

## 📞 Communication

**Mises à jour** : Toutes les 1h dans STATUS_COPILOT.md  
**Blocages** : Signaler dans PROBLEMS.md  
**API ready** : Notifier dans STATUS_GEMINI "Memory API disponible"  

---

**Début** : MAINTENANT  
**Fin estimée** : Dans 6-8 heures  
**Phase actuelle** : Phase 1 (Physical Frame Allocator)
