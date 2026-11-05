// kernel/src/memory/virtual/vma/mod.rs
//
// VMA module — virtual memory area management.
// Layer 0 — no external dependency except `spin`.

pub mod cow;
pub mod descriptor;
pub mod operations;
pub mod tree;

pub use cow::{cow_break, mark_vma_cow, CowBreakResult, CowFrameAllocator, COW_STATS};
pub use descriptor::{VmaBacking, VmaDescriptor, VmaFlags};
pub use operations::{
    find_gap, mprotect_vma, split_vma, validate_vma, MprotectResult, SplitResult, VmaAllocParams,
};
pub use tree::{VmaTree, VmaTreeIter, MAX_VMAS_PER_PROCESS};
