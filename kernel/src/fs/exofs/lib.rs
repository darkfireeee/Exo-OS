// ExoFS — système de fichiers natif Exo-OS
// Ring 0, no_std, Rust
// Feature flags nécessaires au module fs/exofs/
#![no_std]
#![feature(allocator_api)]
#![feature(try_reserve_kind)]
#![feature(const_size_of_val)]
#![feature(atomic_from_mut)]
#![allow(dead_code)]
#![allow(unused_imports)]
