// kernel/src/memory/virtual/address_space/mod.rs
//
// Module address_space — espaces d'adressage kernel et utilisateur.
// Couche 0 — aucune dépendance externe sauf `spin`.

pub mod kernel;
pub mod mapper;
pub mod tlb;
pub mod user;

pub use kernel::{KernelAddressSpace, KERNEL_AS};
pub use mapper::Mapper;
pub use tlb::{
    TlbFlushType, TlbStats, TlbShootdownQueue,
    flush_single, flush_range, flush_all, flush_all_including_global,
    shootdown, shootdown_sync, register_tlb_ipi_sender, TLB_QUEUE, TLB_STATS,
};
pub use user::{UserAddressSpace, UserAsStats, USER_MMAP_BASE, USER_STACK_SIZE};
