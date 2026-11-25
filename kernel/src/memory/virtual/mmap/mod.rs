//! # memory/virtual/mmap — User Virtual Memory Operations (`mmap`, `munmap`, `mprotect`, `mremap`, `brk`, SHM)
//!
//! Segregated into focused submodules:
//! - `helpers`           : `MmapError`, `MremapZeroCopy`, address-space hook, protection helpers, rollback
//! - `anon_shared`       : `do_mmap`, `do_mmap_in_as`, `do_mmap_shared_frames`, `do_share_range_between_as`
//! - `munmap_shootdown`  : `do_munmap`, `do_munmap_in_as`, `do_munmap_range_in_as`, batched TLB shootdown
//! - `mprotect`          : `do_mprotect`, `do_mprotect_verified_file_exec`, `do_mprotect_in_as`
//! - `mremap_brk`        : `do_mremap_zero_copy`, `do_brk`
//! - `ipc_shm`           : `ShmProviderFns` injection & `map_shm_into_process`
//!
//! LAYER 0 — upper layers inject callbacks via `register_current_as_getter` / `register_shm_provider`.

mod anon_shared;
mod helpers;
mod ipc_shm;
mod mprotect;
mod mremap_brk;
mod munmap_shootdown;
#[cfg(test)]
mod tests;

pub use anon_shared::{
    do_mmap, do_mmap_in_as, do_mmap_shared_frames, do_share_range_between_as,
};
pub use helpers::{register_current_as_getter, CurrentAsGetterFn, MmapError, MremapZeroCopy};
use helpers::{
    free_removed_vma, get_current_user_as, prot_to_page_flags, prot_to_vma_flags,
    record_exec_mapping_denied, rollback_anon_pages, rollback_shm_into_process, validate_prot,
    PtAllocOnly, MAP_FIXED, MAP_FIXED_NOREPLACE, MAP_SHARED, PROT_EXEC, PROT_READ, PROT_WRITE,
};
#[allow(unused_imports)]
use helpers::{MAP_ANON, MAP_PRIVATE};
pub use ipc_shm::{
    map_shm_into_process, register_shm_provider, ShmMapError, ShmMapIntoResult, ShmPagePhysFn,
    ShmProviderFns, ShmRegionInfo, ShmRegionInfoFn, ShmRegisterMappingFn, ShmReleaseRegionFn,
};
pub use mprotect::{do_mprotect, do_mprotect_in_as, do_mprotect_verified_file_exec};
pub use mremap_brk::{do_brk, do_mremap_zero_copy};
pub use munmap_shootdown::{do_munmap, do_munmap_in_as, do_munmap_range_in_as};
