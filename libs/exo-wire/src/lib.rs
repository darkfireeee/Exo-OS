#![no_std]
//! `exo-wire` — Canonical `#[repr(C)]` IPC wire protocol layouts for Exo-OS v0.2.0.
//!
//! Depends only on [`exo_abi`], keeping wire definitions strictly decoupled from
//! server state machines, driver hardware logic, and Ring 0 internals.

pub mod audio;
pub mod crypto;
pub mod display;
pub mod input;
pub mod ipc;
pub mod net;
pub mod netgrant;
pub mod storage;
pub mod tty;

pub use audio::*;
pub use crypto::*;
pub use display::*;
pub use input::*;
pub use ipc::*;
pub use net::*;
pub use netgrant::*;
pub use storage::*;
pub use tty::*;
