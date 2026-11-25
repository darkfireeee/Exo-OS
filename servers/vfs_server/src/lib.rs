#![no_std]
#![allow(dead_code, unused_imports)]

//! Pure host-testable library surface for `vfs_server`.

pub mod compat;
pub mod mount_table;
pub mod ops;
pub mod translation_layer;
