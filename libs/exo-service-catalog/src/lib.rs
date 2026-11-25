#![no_std]

//! Consolidated static port-policy & `PhoenixSafe` recovery metadata catalog.
//!
//! Replaces the 8 standalone metadata stubs (`exo-services`, `exo-network`,
//! `exo-fs`, `exo-crypto`, `exo-device`, `exo-graphics`, `exo-posix`, `exo-runtime`)
//! so runtime/ABI crates (`exo-abi`, `exo-wire`, `exo-srv-rt`, `exo-rt`) stay
//! clearly separated from static compatibility/port policy catalogs.

pub mod crypto;
pub mod device;
pub mod fs;
pub mod graphics;
pub mod network;
pub mod posix;
pub mod runtime;
pub mod services;
