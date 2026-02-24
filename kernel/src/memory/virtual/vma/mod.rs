// kernel/src/memory/virtual/vma/mod.rs
//
// Module VMA — gestion des régions mémoire virtuelles.
// Couche 0 — aucune dépendance externe sauf `spin`.

pub mod cow;
pub mod descriptor;
pub mod operations;
pub mod tree;

pub use descriptor::{VmaDescriptor, VmaFlags, VmaBacking};
pub use tree::{VmaTree, VmaTreeIter, MAX_VMAS_PER_PROCESS};
pub use operations::{
    VmaAllocParams, find_gap, split_vma, SplitResult,
    mprotect_vma, MprotectResult, validate_vma,
};
pub use cow::{CowFrameAllocator, CowBreakResult, cow_break, mark_vma_cow, COW_STATS};
