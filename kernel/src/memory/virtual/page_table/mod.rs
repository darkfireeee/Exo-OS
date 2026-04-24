// kernel/src/memory/virtual/page_table/mod.rs
//
// Module page_table — tables de pages x86_64.
// Couche 0 — aucune dépendance externe sauf `spin`.

pub mod builder;
pub mod kpti_split;
pub mod walker;
pub mod x86_64;

pub use x86_64::{
    invlpg, phys_to_table, phys_to_table_mut, phys_to_table_ref, read_cr3, write_cr3, PageTable,
    PageTableEntry, PageTableLevel,
};

pub use builder::PageTableBuilder;
pub use kpti_split::{should_enable_kpti, KptiState, KptiTable, KPTI};
pub use walker::{FrameAllocatorForWalk, PageTableWalker, WalkResult};
