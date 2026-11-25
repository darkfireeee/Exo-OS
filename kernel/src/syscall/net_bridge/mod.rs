#![allow(dead_code)]
//! # syscall/net_bridge — Unified ExoNet BSD sockets <-> `network_server` bridge (V5)
//!
//! Segregates the Ring 0 network bridge into:
//! - `rpc`         : `network_server` endpoint cache, wire call & zero-trust permission check
//! - `data_window` : 16 KiB `NET_DATA_WINDOW` sleepable lock & remote address-space copy
//! - `socket_map`  : `PSEUDO_NET_SOCKET_TAG` `OBJECT_TABLE` registration & poll readiness
//! - `ops_connect` : `socket`, `bind`, `connect`, `listen`, `accept`, `shutdown`, `socketpair`
//! - `ops_io`      : `sendto`, `recvfrom`, `sendmsg`, `recvmsg`, `setsockopt`, `getsockopt`

pub mod data_window;
pub mod ops_connect;
pub mod ops_io;
mod rpc;
pub mod socket_map;

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub use exo_wire::net::{
    NetMsg, NetReply, NET_FLAG_USE_DATA_WINDOW, NET_OP_ACCEPT, NET_OP_BIND, NET_OP_CLOSE,
    NET_OP_CONNECT, NET_OP_GETPEERNAME, NET_OP_GETSOCKNAME, NET_OP_GETSOCKOPT, NET_OP_HANDSHAKE,
    NET_OP_LISTEN, NET_OP_OPEN, NET_OP_POLL, NET_OP_RECVFROM, NET_OP_RECVMSG, NET_OP_SENDMSG,
    NET_OP_SENDTO, NET_OP_SETSOCKOPT, NET_OP_SHUTDOWN, NET_OP_SOCKETPAIR,
};

pub use ops_connect::*;
pub use ops_io::*;
pub(in crate::syscall::net_bridge) use rpc::*;
pub use socket_map::*;

pub(super) const NET_DATA_WINDOW_MAX: usize = exo_wire::net::NET_DATA_WINDOW_SIZE;
pub(super) const AF_UNIX: i32 = exo_abi::AF_UNIX as i32;
pub(super) const AF_INET: u16 = exo_abi::AF_INET;
pub(super) const SOCK_STREAM: i32 = exo_abi::SOCK_STREAM;
pub(super) const SOCK_DGRAM: i32 = exo_abi::SOCK_DGRAM;
pub(super) const SOCKADDR_IN_LEN: usize = 16;
pub(super) const IOV_MAX: u64 = 1024;
pub(super) const NET_HANDLE_TAG: u32 = 0x4000_0000;
pub(super) const NET_HANDLE_TAG_MASK: u32 = 0xf000_0000;
pub(super) const NET_INLINE_DATA_MAX: usize = 128;
pub(super) const NET_REPLY_DATA_OFFSET: usize = core::mem::size_of::<NetReply>();
pub(super) const SOL_SOCKET: i32 = exo_abi::SOL_SOCKET;
pub(super) const SO_RCVTIMEO: i32 = 20;
pub(super) const SO_SNDTIMEO: i32 = 21;
pub(super) const MSG_PEEK: u32 = 0x02;
pub(super) const MSG_DONTWAIT: u32 = 0x40;
pub(super) const MSG_NOSIGNAL: u32 = 0x4000;
pub(super) const CONNECT_TIMEOUT_NS: u64 = 10_000_000_000;
pub(super) const NET_WAIT_SLICE_NS: u64 = 2_000_000;

pub(super) static NET_READY: AtomicBool = AtomicBool::new(false);
pub(super) static NETWORK_ENDPOINT: AtomicU64 = AtomicU64::new(0);

/// Activates the network bridge after kernel base initialization.
///
/// # Safety
/// Call once from the BSP boot path, after IPC initialization.
pub unsafe fn net_bridge_preinit() {
    NET_READY.store(true, Ordering::Release);
}

#[inline]
pub fn bridge_result(result: Result<i64, i64>) -> i64 {
    match result {
        Ok(value) => value,
        Err(errno) => errno,
    }
}
