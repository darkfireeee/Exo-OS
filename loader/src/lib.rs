#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod alloc_mmap;
pub mod dynamic_linker;
pub mod elf;
pub mod entry;
pub mod security;
pub mod syscall;

#[cfg(not(test))]
#[global_allocator]
static GLOBAL_ALLOC: alloc_mmap::MmapLoaderAlloc = alloc_mmap::MmapLoaderAlloc::new();
