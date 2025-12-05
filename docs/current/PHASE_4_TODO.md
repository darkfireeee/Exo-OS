# Phase 4 TODO - Active Implementation

**Date**: December 5, 2025  
**Status**: 🚀 **IN PROGRESS**

---

## 🎯 Four-Pronged Attack Strategy

Instead of doing one at a time, we'll implement all four in parallel where possible to maximize progress.

---

## ✅ TODO A: Virtual Memory (PRIORITY 1)

### A1. Page Table Walking ⏳ NEXT
- [ ] Implement `walk_page_table()` helper
- [ ] Add `get_page_table_entry()` function
- [ ] Test page table traversal

### A2. Page Mapping
- [ ] Implement `map_page()` in `mapper.rs`
- [ ] Implement `unmap_page()` in `mapper.rs`
- [ ] Add `remap_page()` for permission changes
- [ ] Implement `is_mapped()` checker

### A3. TLB Management
- [ ] Add `flush_tlb_single()` with invlpg
- [ ] Add `flush_tlb_all()` with CR3 reload
- [ ] Add `flush_tlb_range()` for multiple pages
- [ ] Create TLB flush tracking metrics

### A4. Copy-On-Write (COW)
- [ ] Add COW flag to PageFlags
- [ ] Implement page fault handler for COW
- [ ] Add reference counting for physical pages
- [ ] Modify fork to mark pages as COW
- [ ] Test COW with write operations

### A5. Integration & Testing
- [ ] Update fork to use COW
- [ ] Add memory isolation tests
- [ ] Benchmark page operations
- [ ] Document memory layout

**Estimated time**: 2-3 days

---

## ✅ TODO B: VFS & File System (PRIORITY 3)

### B1. VFS Core Operations
- [ ] Complete `vfs_open()` implementation
- [ ] Complete `vfs_read()` implementation
- [ ] Complete `vfs_write()` implementation
- [ ] Complete `vfs_seek()` implementation
- [ ] Complete `vfs_close()` implementation

### B2. File Descriptor Management
- [ ] Create FdTable structure
- [ ] Implement `allocate_fd()`
- [ ] Implement `get_file_by_fd()`
- [ ] Implement `close_fd()`
- [ ] Add FD inheritance for fork

### B3. Directory Operations
- [ ] Implement `opendir()`
- [ ] Implement `readdir()`
- [ ] Implement `closedir()`
- [ ] Add directory caching

### B4. ext2 File System Driver
- [ ] Create `drivers/fs/ext2.rs`
- [ ] Parse ext2 superblock
- [ ] Read inode structures
- [ ] Implement block reading
- [ ] Add directory traversal
- [ ] Read-only operations first

### B5. Mount/Unmount
- [ ] Create mount point structure
- [ ] Implement `mount()` syscall
- [ ] Implement `umount()` syscall
- [ ] Add mount table

**Estimated time**: 3-4 days

---

## ✅ TODO C: exec() Implementation (PRIORITY 2)

### C1. ELF Parser
- [ ] Create `loader/elf.rs` module
- [ ] Parse ELF header (magic, class, endian)
- [ ] Parse program headers (PT_LOAD, PT_INTERP)
- [ ] Parse section headers
- [ ] Validate ELF file

### C2. Memory Loading
- [ ] Load ELF segments into memory
- [ ] Setup program break (brk)
- [ ] Allocate and setup stack
- [ ] Map ELF into virtual memory
- [ ] Handle BSS section (zero-fill)

### C3. Execution Transfer
- [ ] Create new address space
- [ ] Copy argv/envp to stack
- [ ] Setup initial registers
- [ ] Jump to entry point
- [ ] Cleanup old process state

### C4. exec() Syscall
- [ ] Implement `sys_execve()` in handlers
- [ ] Add argument/environment parsing
- [ ] Handle path resolution
- [ ] Add error handling

### C5. Testing
- [ ] Test with simple ELF (hello world)
- [ ] Test with arguments
- [ ] Test with environment vars
- [ ] Add exec tests to test suite

**Estimated time**: 2 days

---

## ✅ TODO D: SMP Multi-core (PRIORITY 4)

### D1. AP (Application Processor) Initialization
- [ ] Create `arch/x86_64/smp.rs` module
- [ ] Detect number of CPUs (ACPI/MP tables)
- [ ] Setup APIC for each CPU
- [ ] Send INIT-SIPI-SIPI sequence
- [ ] Wait for APs to boot

### D2. Per-CPU Data Structures
- [ ] Create per-CPU run queues
- [ ] Add per-CPU scheduler state
- [ ] Implement CPU-local storage
- [ ] Add per-CPU metrics

### D3. Load Balancer Activation
- [ ] Connect load balancer to scheduler
- [ ] Implement thread migration
- [ ] Add work-stealing logic
- [ ] Setup periodic balancing

### D4. IPI (Inter-Processor Interrupts)
- [ ] Implement IPI sending
- [ ] Add IPI handlers
- [ ] Add TLB shootdown IPI
- [ ] Add reschedule IPI

### D5. Synchronization
- [ ] Add per-CPU locks
- [ ] Implement spinlocks with backoff
- [ ] Add RCU primitives
- [ ] Test lock contention

**Estimated time**: 4-5 days

---

## 📊 Implementation Priority

```
Week 1:
  Day 1-2: A1, A2, C1 (page walking, mapping, ELF parser)
  Day 3:   A3, C2 (TLB, ELF loading)
  
Week 2:
  Day 4:   A4, C3 (COW, exec transfer)
  Day 5:   C4, C5, A5 (exec syscall, testing)
  Day 6-7: B1, B2 (VFS operations, FD table)
  
Week 3:
  Day 8-9:  B3, B4 (directories, ext2)
  Day 10:   B5 (mount/unmount)
  Day 11-12: D1, D2 (AP init, per-CPU)
  
Week 4:
  Day 13-14: D3, D4, D5 (load balancer, IPI, sync)
```

---

## 🎯 Quick Wins (Can Start Now)

### Parallel Track 1: Virtual Memory Foundation
- Start with page table walking
- Implement basic map/unmap
- Add TLB management

### Parallel Track 2: ELF Parser
- Parse ELF headers (no dependencies)
- Validate structure
- Test with example ELF

### Parallel Track 3: VFS Skeleton
- Define interfaces
- Create stub implementations
- Add FD table structure

### Parallel Track 4: SMP Detection
- Detect CPU count
- Parse ACPI tables
- Prepare for AP init

---

## 🚀 Starting Point: Implement All Foundations Now

I'll start implementing:
1. **A1**: Page table walking helpers
2. **C1**: ELF parser basics  
3. **B1**: VFS interface definitions
4. **D1**: CPU detection

This way we make progress on all fronts simultaneously!

---

## 📈 Progress Tracking

```
A: Virtual Memory  [▰▰▱▱▱▱▱▱▱▱]  20% (page walking started)
B: VFS & FS        [▱▱▱▱▱▱▱▱▱▱]   0%
C: exec()          [▱▱▱▱▱▱▱▱▱▱]   0%
D: SMP             [▱▱▱▱▱▱▱▱▱▱]   0%

Overall Phase 4:   [▰▱▱▱▱▱▱▱▱▱]   5%
```

Updates after each session!
