// kernel/src/memory/virtual/mod.rs
//
// Module virtual — gestion de la mémoire virtuelle.
// Couche 0 — aucune dépendance externe sauf `spin`.

pub mod address_space;
pub mod fault;
pub mod mmap;
pub mod page_table;
pub mod vma;

pub use address_space::{
    flush_all, flush_range, flush_single, register_tlb_ipi_sender, shootdown, shootdown_sync,
    KernelAddressSpace, Mapper, TlbFlushType, UserAddressSpace, UserAsStats, KERNEL_AS, TLB_QUEUE,
    TLB_STATS,
};

pub use page_table::{
    invlpg, read_cr3, should_enable_kpti, write_cr3, FrameAllocatorForWalk, PageTable,
    PageTableBuilder, PageTableEntry, PageTableLevel, PageTableWalker, WalkResult, KPTI,
};

pub use vma::{
    cow_break, find_gap, split_vma, validate_vma, CowBreakResult, CowFrameAllocator,
    VmaAllocParams, VmaBacking, VmaDescriptor, VmaFlags, VmaTree, MAX_VMAS_PER_PROCESS,
};

pub use fault::{
    handle_page_fault, FaultAllocator, FaultCause, FaultContext, FaultResult, FAULT_STATS,
};
pub use mmap::{
    do_brk, do_mmap, do_mprotect, do_munmap, map_shm_into_process, register_current_as_getter,
    CurrentAsGetterFn, MmapError, ShmMapError, ShmMapIntoResult,
};
