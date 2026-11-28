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
    boot_log, lookup_endpoint, peer_is_any_service, peer_restrictions, peer_service_class,
    recv_ipc_timeout, register_endpoint, ring1_cfg_panic_exit_handler, ring1_nostd_panic_handler,
    ring1_panic_exit_handler, ring1_panic_handler, send_reply, sigreturn_trampoline,
    EXO_RESTRICT_NO_INET, EXO_RESTRICT_NO_NETWORK, EXO_RESTRICT_SANDBOX_FULL, SVC_AUDIO_SERVER,
    SVC_CRYPTO_SERVER, SVC_DEVICE_SERVER, SVC_EXOSH, SVC_EXO_COMPOSITOR, SVC_EXO_SHIELD,
    SVC_FB_SERVER, SVC_INIT_SERVER, SVC_INPUT_SERVER, SVC_IPC_BROKER, SVC_MEMORY_SERVER,
    SVC_NETWORK_SERVER, SVC_PS2_DRIVER, SVC_SCHEDULER_SERVER, SVC_TTY_SERVER, SVC_UNKNOWN,
    SVC_USB_SERVER, SVC_VFS_SERVER, SVC_VIRTIO_DRIVER,
};
pub use exo_wire::*;
