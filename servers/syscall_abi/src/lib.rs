#![no_std]
//! Compatibility facade re-exporting canonical contracts from `libs/exo-abi`,
//! `libs/exo-wire`, and `libs/exo-srv-rt`.

// Ensure `libexo_srv_rt.rlib` stays in the link line so `-u _exo_ring1_entry` resolves.
extern crate exo_srv_rt;

pub mod cap_abi {
    pub use exo_abi::cap::*;
}

pub mod display_abi {
    pub use exo_wire::display::*;
}

pub mod netgrant_abi {
    pub use exo_wire::netgrant::*;
}

pub use exo_abi::*;
pub use exo_srv_rt::{
    boot_log, lookup_endpoint, recv_ipc_timeout, register_endpoint, ring1_cfg_panic_exit_handler,
    ring1_nostd_panic_handler, ring1_panic_exit_handler, ring1_panic_handler, send_reply,
    sigreturn_trampoline,
};
pub use exo_wire::*;
