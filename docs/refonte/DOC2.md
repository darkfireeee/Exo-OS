# DOC2 - Memory, boot, TLB

## MEM-01

Architecture exception paths may call memory fault handlers, but memory remains
the owner of page-table semantics, VMA rules, CoW, demand paging, and physmap
translation.

## MEM-02

Boot memory order is strict:

1. Emergency pool.
2. Physical memory map and bootstrap bitmap.
3. Extended physmap.
4. Buddy allocator.
5. Slab/SLUB.
6. NUMA.
7. Kernel address-space registration.

## TLB-01

When a mapping can be visible on another CPU or another address space, update
page tables first, then flush locally, then send synchronous remote shootdown.
Local-only `invlpg` is valid only when the address space is current and not
running elsewhere.
