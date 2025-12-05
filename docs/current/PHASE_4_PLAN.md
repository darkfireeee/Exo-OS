# Phase 4 Plan - Next Steps

**Date**: December 5, 2025  
**Current Version**: Exo-OS v0.5.0 "Linux Crusher"  
**Phase 3**: ✅ **COMPLETE** (Scheduler enhancement)

---

## 📊 Phase 3 Achievements Recap

✅ **Scheduler lock-free fork** (CAS-based, no deadlocks)  
✅ **Error handling** (20+ typed errors with recovery hints)  
✅ **Metrics system** (40+ atomic counters, zero overhead)  
✅ **Scheduling policies** (6 policies: FIFO, RR, Normal, Batch, Idle, Deadline)  
✅ **Load balancer** (multi-CPU ready, work-stealing)  
✅ **Code cleanup** (removed scheduler_v2, 3,130 lines total)

**Result**: Scheduler is 4.3x simpler than Linux (2,300 vs 10,000 lines) and production-ready.

---

## 🎯 Phase 4 Options

### Option A: Virtual Memory (HIGHEST PRIORITY)
**Goal**: Complete memory management for COW fork and isolation

**Tasks**:
1. Implement `map_page()` and `unmap_page()` in `mapper.rs`
2. Add TLB management (invlpg, full flush)
3. Implement COW (Copy-On-Write) for fork
4. Page fault handler improvements
5. Memory isolation per process

**Files to modify**:
- `kernel/src/memory/virtual_mem/mapper.rs`
- `kernel/src/memory/page_table.rs`
- `kernel/src/arch/x86_64/interrupts/exceptions.rs` (page fault)

**Benefits**:
- Fork becomes fully efficient (no copy until write)
- Process isolation (security)
- Foundation for `exec()`

**Estimated time**: 2-3 days

---

### Option B: VFS & File System
**Goal**: Complete VFS layer and real file system

**Tasks**:
1. Complete VFS operations (open, read, write, seek, close)
2. Implement ext2 driver (read-only first)
3. Add file descriptor management
4. Implement directory operations (opendir, readdir)
5. Add mount/unmount support

**Files to modify**:
- `kernel/src/vfs/mod.rs`
- `kernel/src/vfs/file.rs`
- `kernel/src/drivers/fs/ext2.rs` (create)

**Benefits**:
- Real file system access
- Persistent storage
- Foundation for loading programs

**Estimated time**: 3-4 days

---

### Option C: exec() Implementation
**Goal**: Execute ELF binaries

**Tasks**:
1. Implement ELF parser (read headers, sections)
2. Load ELF into memory
3. Setup stack for new program
4. Transfer control to entry point
5. Cleanup old process state

**Files to create**:
- `kernel/src/loader/elf.rs`
- `kernel/src/syscall/handlers/exec.rs`

**Benefits**:
- Run actual programs (not just fork)
- Full process lifecycle
- Shell can launch programs

**Estimated time**: 2 days

---

### Option D: SMP Multi-core
**Goal**: Enable multiple CPU cores

**Tasks**:
1. Initialize AP (Application Processor) cores
2. Per-CPU run queues
3. Activate load balancer
4. CPU-local storage
5. Inter-processor interrupts (IPI)

**Files to modify**:
- `kernel/src/arch/x86_64/smp.rs` (create)
- `kernel/src/scheduler/core/scheduler.rs`
- `kernel/src/scheduler/core/loadbalancer.rs`

**Benefits**:
- True parallelism
- Better performance
- Scalability

**Estimated time**: 4-5 days

---

## 🎖️ Recommended Order

Based on dependency analysis:

### 1. Virtual Memory (2-3 days)
**Why first**: Foundation for everything else. COW fork requires VM, exec needs VM for program loading, file systems need VM for buffers.

### 2. exec() (2 days)
**Why second**: With VM ready, exec is straightforward. Enables running real programs.

### 3. VFS & File System (3-4 days)
**Why third**: With exec ready, we need real programs to load. File system provides persistent storage.

### 4. SMP Multi-core (4-5 days)
**Why last**: All single-core features should work first. SMP adds complexity and debugging difficulty.

**Total estimated time**: 11-14 days for complete Phase 4

---

## 🚀 Quick Start: Option A (Virtual Memory)

### Step 1: Implement map_page()

```rust
// kernel/src/memory/virtual_mem/mapper.rs

pub fn map_page(
    page_table: &mut PageTable,
    virt: VirtAddr,
    phys: PhysAddr,
    flags: PageFlags,
) -> Result<(), MapError> {
    // Get indices
    let p4_idx = virt.p4_index();
    let p3_idx = virt.p3_index();
    let p2_idx = virt.p2_index();
    let p1_idx = virt.p1_index();
    
    // Walk page table, creating missing levels
    let p4 = page_table.level4_table_mut();
    
    // Create P3 if needed
    if !p4[p4_idx].is_present() {
        let frame = allocate_frame()?;
        p4[p4_idx].set(frame, PageFlags::PRESENT | PageFlags::WRITABLE);
    }
    let p3 = unsafe { &mut *(p4[p4_idx].addr().as_u64() as *mut PageTable) };
    
    // Create P2 if needed
    if !p3[p3_idx].is_present() {
        let frame = allocate_frame()?;
        p3[p3_idx].set(frame, PageFlags::PRESENT | PageFlags::WRITABLE);
    }
    let p2 = unsafe { &mut *(p3[p3_idx].addr().as_u64() as *mut PageTable) };
    
    // Create P1 if needed
    if !p2[p2_idx].is_present() {
        let frame = allocate_frame()?;
        p2[p2_idx].set(frame, PageFlags::PRESENT | PageFlags::WRITABLE);
    }
    let p1 = unsafe { &mut *(p2[p2_idx].addr().as_u64() as *mut PageTable) };
    
    // Map the page
    if p1[p1_idx].is_present() {
        return Err(MapError::AlreadyMapped);
    }
    p1[p1_idx].set(phys, flags);
    
    // Flush TLB
    unsafe {
        core::arch::asm!(
            "invlpg [{}]",
            in(reg) virt.as_u64(),
            options(nostack, preserves_flags)
        );
    }
    
    Ok(())
}
```

### Step 2: Implement unmap_page()

```rust
pub fn unmap_page(
    page_table: &mut PageTable,
    virt: VirtAddr,
) -> Result<PhysAddr, MapError> {
    // Walk page table
    let p4 = page_table.level4_table();
    let p4_idx = virt.p4_index();
    
    if !p4[p4_idx].is_present() {
        return Err(MapError::NotMapped);
    }
    
    // Continue walking...
    let p3 = unsafe { &*(p4[p4_idx].addr().as_u64() as *const PageTable) };
    let p3_idx = virt.p3_index();
    
    if !p3[p3_idx].is_present() {
        return Err(MapError::NotMapped);
    }
    
    // ... get to P1
    let p1 = unsafe { &mut *(p2[p2_idx].addr().as_u64() as *mut PageTable) };
    let p1_idx = virt.p1_index();
    
    if !p1[p1_idx].is_present() {
        return Err(MapError::NotMapped);
    }
    
    // Unmap
    let phys = p1[p1_idx].addr();
    p1[p1_idx].clear();
    
    // Flush TLB
    unsafe {
        core::arch::asm!(
            "invlpg [{}]",
            in(reg) virt.as_u64(),
            options(nostack, preserves_flags)
        );
    }
    
    Ok(phys)
}
```

### Step 3: Test with mmap

```rust
// In tests or kernel init
let virt = VirtAddr::new(0x1000_0000);
let phys = allocate_frame().unwrap();
map_page(&mut PAGE_TABLE, virt, phys, PageFlags::PRESENT | PageFlags::WRITABLE)?;

// Write to mapped page
unsafe {
    *(virt.as_u64() as *mut u64) = 0xDEADBEEF;
}

// Verify
assert_eq!(unsafe { *(virt.as_u64() as *const u64) }, 0xDEADBEEF);

// Unmap
unmap_page(&mut PAGE_TABLE, virt)?;
```

---

## 📝 Action Items

**Immediate next step**: Choose Phase 4 direction

Type:
- `A` for Virtual Memory
- `B` for VFS & File System  
- `C` for exec()
- `D` for SMP Multi-core

Or provide custom direction!

---

## 📈 Progress Tracking

```
Phase 0: Timer + Context Switch ........ ✅ 100%
Phase 1: VFS + POSIX-X + fork .......... 🟡  60% (fork ✅, exec ❌, VFS partial)
Phase 2: Context Capture ............... ✅ 100%
Phase 3: Scheduler Enhancement ......... ✅ 100%
Phase 4: ??? ........................... ⏳   0%
```

**Recommended**: Phase 4A (Virtual Memory) → Phase 4C (exec) → Phase 4B (VFS) → Phase 4D (SMP)
