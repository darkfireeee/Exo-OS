//! Modular smoltcp TCP/UDP/ICMP + loopback stack for `network_server`.

mod iface_ops;
mod loopback_dev;
mod phy_dev;
mod socket_io;
mod storage;

use crate::socket_table::MAX_SOCKETS;
use smoltcp::iface::{Interface, SocketHandle};

pub(super) const LISTEN_BACKLOG_SLOTS: usize = 4;
pub(super) const EXTRA_SLOTS: usize = 33;
pub(super) const SOCKET_STORAGE_LEN: usize = MAX_SOCKETS + EXTRA_SLOTS;
pub(super) const MAX_LISTENERS: usize = 8;
pub(super) const EPOLLIN: u32 = 0x0001;
pub(super) const EPOLLOUT: u32 = 0x0004;
pub(super) const EPOLLERR: u32 = 0x0008;
pub(super) const EPOLLHUP: u32 = 0x0010;
pub(super) const EPOLLRDHUP: u32 = 0x2000;

#[inline]
pub const fn is_loopback_ipv4(ip: u32) -> bool {
    (ip & 0xff00_0000) == 0x7f00_0000
}

#[derive(Clone, Copy)]
pub(super) struct ListenerPool {
    pub(super) active: bool,
    pub(super) listener_exo_handle: u32,
    pub(super) local_addr: u32,
    pub(super) local_port: u16,
    pub(super) slots: [Option<usize>; LISTEN_BACKLOG_SLOTS],
}

impl ListenerPool {
    pub(super) const fn empty() -> Self {
        Self {
            active: false,
            listener_exo_handle: 0,
            local_addr: 0,
            local_port: 0,
            slots: [None; LISTEN_BACKLOG_SLOTS],
        }
    }
}

pub struct SmoltcpIface {
    pub(super) mac: [u8; 6],
    pub(super) ip: u32,
    pub(super) prefix_len: u8,
    pub(super) gateway: u32,
    pub(super) ingress_ticks: u64,
    pub(super) egress_ticks: u64,
    pub(super) iface: Option<Interface>,
    pub(super) lo_iface: Option<Interface>,
    pub(super) socket_handles: [Option<SocketHandle>; SOCKET_STORAGE_LEN],
    pub(super) socket_exo_handles: [u32; SOCKET_STORAGE_LEN],
    pub(super) listener_pools: [ListenerPool; MAX_LISTENERS],
    pub(super) dhcp_slot: Option<usize>,
}

pub enum TcpConnectStatus {
    Pending,
    Established,
    Failed,
}
