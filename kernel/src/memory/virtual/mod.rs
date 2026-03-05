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
    KernelAddressSpace, KERNEL_AS,
    Mapper,
    TlbFlushType, flush_single, flush_range, flush_all,
    shootdown, shootdown_sync, register_tlb_ipi_sender,
    TLB_QUEUE, TLB_STATS,
    UserAddressSpace, UserAsStats,
};

pub use page_table::{
    PageTableEntry, PageTable, PageTableLevel,
    PageTableWalker, WalkResult, FrameAllocatorForWalk,
    PageTableBuilder,
    KPTI, should_enable_kpti,
    read_cr3, write_cr3, invlpg,
};

pub use vma::{
    VmaDescriptor, VmaFlags, VmaBacking,
    VmaTree, MAX_VMAS_PER_PROCESS,
    VmaAllocParams, find_gap, split_vma, validate_vma,
    CowFrameAllocator, CowBreakResult, cow_break,
};

pub use fault::{
    FaultCause, FaultContext, FaultResult,
    handle_page_fault, FAULT_STATS, FaultAllocator,
};
pub use mmap::{
    do_mmap, do_munmap, do_mprotect, do_brk,
    MmapError, CurrentAsGetterFn, register_current_as_getter,
};