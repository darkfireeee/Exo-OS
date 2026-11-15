// Layer 3: File system
// Ring 0, no_std
//
// fs/ is the highest layer of the kernel — depends on memory/, scheduler/,
// process/, security/. Initialized in Phase 7 of kernel_init().

pub mod elf_loader_impl;
/// ExoFS — native Exo-OS file system (epoch-journaled)
pub mod exofs;
