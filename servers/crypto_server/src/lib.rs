#![no_std]

//! Pure host-testable library surface for `crypto_server` (SRV-04).

extern crate blake3;

pub mod keystore;
pub mod wire;
pub mod xchacha20;
