# Page Splitting Design - Exo-OS Memory Management

## Analysis Date: 2026-02-05

## Problem Statement

### Current Architecture
- **Boot mapping:** 0-8GB mapped with 2MB huge pages (4096 huge pages)
  - boot.asm creates: P4[0] → P3 → P2_tables[0..7] → 512×2MB pages each
  - Total: 8 tables × 512 entries × 2MB = 8GB
  
- **Conflict:** ELF loader tries to map 4KB pages at 0x40000000 (1GB)
  - Falls inside existing huge page #512 (at 1GB boundary)
  - page_table.rs:255 returns: `"Cannot map inside huge page"`

### Root Cause
```rust
// Current code - BLOCKS mapping inside huge pages
else if entry.is_huge() {
    return Err(MemoryError::InternalError("Cannot map inside huge page"));
}
```

## Solution Design

### 1. Page Splitting Algorithm

**Goal:** Break a 2MB huge page into 512×4KB normal pages transparently

**Steps:**
1. **Detect:** huge page flag in PDE (Page Directory Entry)
2. **Allocate:** new PT (Page Table) with 512 entries
3. **Populate:** Each PT entry maps same physical memory:
   ```
   Huge page: 0x200000 (2MB base)
   → PT[0] = 0x200000 + 0×4KB
   → PT[1] = 0x200000 + 1×4KB
   → ...
   → PT[511] = 0x200000 + 511×4KB
   ```
4. **Replace:** PDE entry (huge flag) → PDE entry (table pointer)
5. **Flush:** TLB for affected addresses
6. **Continue:** Original mapping operation proceeds

**Complexity:** O(1) - allocate 1 frame, initialize 512 entries

### 2. Memory Layout Optimization

**Current userspace strategy:** Start at 8GB (mmap.rs:99)
```rust
next_addr: 0x2_0000_0000,  // 8GB - after huge pages
```

**Problem:** ELF loader tests use fixed addresses (0x10000000, 0x40000000)
- These fall in huge page zone (0-8GB)
- Forces splits even for tests

**Solution:**
- **Keep 8GB+ for mmap()** - no splits needed ✅
- **Add split capability** - for fixed mappings that MUST be in 0-8GB
- **Document strategy** - prefer high addresses for userspace

### 3. Implementation Strategy

#### Phase 1: Core Splitting (CRITICAL)
```rust
// In page_table.rs
impl PageTableWalker {
    fn split_huge_page(
        &mut self,
        level: usize,
        huge_entry: &PageTableEntry
    ) -> MemoryResult<PageTable> {
        // 1. Validate: must be huge page at level 2 (PDE)
        // 2. Allocate: new PT (level 1)
        // 3. Extract: base physical address from huge PTE
        // 4. Fill: 512 entries (base + i×4KB)
        // 5. Preserve: original flags (except huge bit)
        // 6. Return: new PT to replace huge entry
    }
}
```

#### Phase 2: Integration with map()
```rust
pub fn map(...) -> MemoryResult<()> {
    // ... existing code ...
    
    } else if entry.is_huge() {
        // NEW: Split instead of error
        log::debug!("[MMU] Splitting huge page at level {}", level);
        let new_table = self.split_huge_page(level, entry)?;
        
        // Replace huge entry with table pointer
        *entry = PageTableEntry::new_frame(
            new_table.physical_address(),
            PageTableFlags::new().present().writable().user(),
        );
        
        // Don't forget the table
        core::mem::forget(new_table);
    }
    
    current_address = entry.address();
}
```

#### Phase 3: Safety & Validation
- **TLB flushing:** All 512 pages (2MB range)
- **Flags preservation:** R/W/X/User bits maintained
- **Level check:** Only split at level 2 (PDE → PT)
- **Error handling:** Allocation failures handled gracefully

#### Phase 4: Performance Optimizations
- **Split tracking:** BTreeMap<PhysicalAddress, bool> to avoid re-splitting
- **Lazy initialization:** Don't zero pages (preserve huge page data)
- **Batch TLB flush:** Single INVLPG per 4KB instead of per-entry

### 4. Testing Strategy

#### Unit Tests
```rust
#[test]
fn test_split_huge_page_level2() {
    // Create walker with huge page at 1GB
    // Split it
    // Verify 512 PT entries created
    // Verify mapping preserved
}

#[test]
fn test_map_inside_split_huge_page() {
    // Map at 0x40000000 (inside huge page)
    // Verify split occurs automatically
    // Verify new 4KB mapping works
}
```

#### Integration Tests
```rust
// exec_tests.rs - existing tests should now PASS
test_load_elf_basic() {
    // Maps at 0x40000000
    // Should auto-split huge page #512
    // Continue with normal mapping
}
```

### 5. Risk Analysis

| Risk | Impact | Mitigation |
|------|--------|-----------|
| TLB coherency issues | HIGH | Flush all 512 pages immediately |
| Memory fragmentation | MED | Track splits, limit to needed pages only |
| Performance regression | LOW | Split is rare, O(1) operation |
| Flags mismatch | MED | Validate flags match before/after split |
| Recursive splits | NONE | Only level 2→1 supported |

### 6. Expected Outcomes

**Before:**
```
[EXEC] About to call mmap: addr=0x40000000, size=0x1000
[ERROR] Cannot map inside huge page
KERNEL PANIC
```

**After:**
```
[MMU] Splitting huge page at 0x40000000 (PDE #512)
[MMU] Created PT with 512 entries (0x40000000-0x40200000)
[EXEC] About to call mmap: addr=0x40000000, size=0x1000
[EXEC] Mapped at 0x40000000
[TEST] ✅ test_load_elf_basic PASSED
```

### 7. Future Enhancements

1. **Huge page reassembly:** Merge 512 contiguous 4KB → 1×2MB
2. **1GB page support:** Split P3 huge pages (if PSE-1GB enabled)
3. **CoW for splits:** Share PT entries until write
4. **Statistics:** Track split count, memory overhead

## Implementation Checklist

- [ ] Add `split_huge_page()` to PageTableWalker
- [ ] Modify `map()` to call split instead of error
- [ ] Add TLB flush for split range
- [ ] Add unit tests for splitting
- [ ] Test exec_tests with split enabled
- [ ] Add logging for diagnostics
- [ ] Document in memory architecture docs
- [ ] Performance benchmark (split overhead)

## Code Size Estimate
- Core splitting: ~80 lines
- Integration: ~20 lines
- Tests: ~100 lines
- Documentation: ~50 lines
**Total: ~250 lines**

## Timeline
- Implementation: 1-2 hours
- Testing: 30 min
- Documentation: 20 min
**Total: ~3 hours**
