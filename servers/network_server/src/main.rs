#![no_std]
#![no_main]
#![allow(dead_code)]

//! network_server V5 -- serveur reseau ExoNet IPC + fenêtre 16 Kio + loopback 127.0.0.1 + DHCP.

use core::cell::UnsafeCell;
use core::panic::PanicInfo;

use spin::Mutex;

pub(crate) use exo_network_server::{
    buf_pool, dhcp, protocol, routing, socket_table, stats, tcp_store,
};

mod driver_link;
mod smoltcp_iface;
mod transient_drain;
mod virtio_device;

use buf_pool::{NetBufPool, VIRTIO_NET_HDR_SIZE_MODERN};
use dhcp::DhcpAction;
use driver_link::DriverLink;
use transient_drain::IsolationState;
use protocol::{
    parse_driver_ctrl, parse_net_msg, parse_raw_call, recv_raw, register_endpoint, send_rpc_reply,
    send_rpc_reply_with_data, DriverCtrlMsg, MacReplyMsg, NetMsg, NetReply, RxReadyMsg,
    TxCompleteMsg, NET_CTRL_MAC_REPLY, NET_CTRL_RX_READY, NET_CTRL_TX_COMPLETE,
    NET_DATA_WINDOW_SIZE, NET_FLAG_USE_DATA_WINDOW, NET_INLINE_DATA_MAX, NET_OP_ACCEPT,
    NET_OP_BIND, NET_OP_CLOSE, NET_OP_CONNECT, NET_OP_GETPEERNAME, NET_OP_GETSOCKNAME,
    NET_OP_GETSOCKOPT, NET_OP_HANDSHAKE, NET_OP_LISTEN, NET_OP_OPEN, NET_OP_POLL, NET_OP_RECVFROM,
    NET_OP_RECVMSG, NET_OP_SENDMSG, NET_OP_SENDTO, NET_OP_SETSOCKOPT, NET_OP_SHUTDOWN,
    NET_OP_SOCKETPAIR, RAW_MSG_SIZE,
};
use routing::RouteTable;
use smoltcp_iface::{is_loopback_ipv4, SmoltcpIface, TcpConnectStatus};
use socket_table::{SocketKind, SocketSnapshot, SocketState, SocketTable, MAX_SOCKETS};
use stats::NetStats;
use tcp_store::TcpStateStore;
use virtio_device::ExoNetDevice;

const ECONNREFUSED: i64 = -111;
const EINPROGRESS: i64 = -115;
const MSG_PEEK: u32 = 0x02;

const DEFAULT_IPV4: u32 = 0x0a00_020f;
const DEFAULT_PREFIX_LEN: u8 = 24;
const DEFAULT_GATEWAY: u32 = 0x0a00_0202;
const DEFAULT_DNS: u32 = 0x0a00_0203;
const CONNECT_TIMEOUT_TICKS: u64 = 500; // ~10s at 20ms recv timeout

/// 4 KiB-aligned 16 KiB kernel-mediated data transfer window (owned by network_server).
/// Ring 0 `net_bridge` copies caller payload into/out of this window under a mutex,
/// avoiding the 128-byte inline IPC cap without exposing client address spaces to Ring 1.
#[repr(C, align(4096))]
struct AlignedDataWindow {
    buf: UnsafeCell<[u8; NET_DATA_WINDOW_SIZE]>,
}

// SAFETY: Accessed sequentially by network_server's single service loop (under NETWORK_SERVICE)
// and mediated by Ring 0 `net_bridge` while serialized by `NET_DATA_LOCK`.
unsafe impl Sync for AlignedDataWindow {}

static NET_DATA_WINDOW: AlignedDataWindow = AlignedDataWindow {
    buf: UnsafeCell::new([0u8; NET_DATA_WINDOW_SIZE]),
};

#[derive(Clone, Copy)]
struct PendingArpPacket {
    active: bool,
    snapshot: SocketSnapshot,
    data: [u8; NET_INLINE_DATA_MAX],
    len: usize,
}

impl PendingArpPacket {
    const fn empty() -> Self {
        Self {
            active: false,
            snapshot: SocketSnapshot::empty(),
            data: [0u8; NET_INLINE_DATA_MAX],
            len: 0,
        }
    }
}

struct NetworkService {
    sockets: SocketTable,
    pool: NetBufPool,
    driver: DriverLink,
    device: ExoNetDevice,
    iface: SmoltcpIface,
    routes: RouteTable,
    dhcp: dhcp::DhcpClient,
    dhcp_last_tx_tick: u64,
    stats: NetStats,
    tcp_store: TcpStateStore,
    isolation: IsolationState,
    bootstrapped: bool,
    ticks: u64,
    unsupported_msg_ops: u64,
    reported_no_hardware_route: bool,
    pending_arp: PendingArpPacket,
}

impl NetworkService {
    const fn new() -> Self {
        Self {
            sockets: SocketTable::new(),
            pool: NetBufPool::empty(),
            driver: DriverLink::empty(),
            device: ExoNetDevice::new(),
            iface: SmoltcpIface::empty(),
            routes: RouteTable::new(),
            dhcp: dhcp::DhcpClient::new(),
            dhcp_last_tx_tick: 0,
            stats: NetStats::new(),
            tcp_store: TcpStateStore::new_empty(),
            isolation: IsolationState::new(),
            bootstrapped: false,
            ticks: 0,
            unsupported_msg_ops: 0,
            reported_no_hardware_route: false,
            pending_arp: PendingArpPacket::empty(),
        }
    }

    fn bootstrap(&mut self) {
        if self.bootstrapped {
            return;
        }
        // Demand-fault and zero every page of NET_DATA_WINDOW so all 4 PTEs are present & writable.
        unsafe {
            let win = &mut *NET_DATA_WINDOW.buf.get();
            let mut off = 0usize;
            while off < win.len() {
                core::ptr::write_volatile(&mut win[off], 0);
                off += 4096;
            }
        }

        self.pool = match NetBufPool::init(VIRTIO_NET_HDR_SIZE_MODERN) {
            Ok(pool) => {
                debug_write(b"network_server: pool ready\n");
                pool
            }
            Err(err) => {
                debug_errno(b"network_server: pool dma errno ", err);
                NetBufPool::empty()
            }
        };
        self.driver = DriverLink::connect_net_driver(&self.pool);
        let (ip, prefix_len) = configured_ipv4();
        self.routes.clear();
        let _ = self.routes.add(0x7f00_0000, 8, 0, 0);
        let on_link = ip & routing::mask(prefix_len);
        let _ = self.routes.add(on_link, prefix_len, 0, 1);
        let _ = self.routes.add(0, 0, DEFAULT_GATEWAY, 10);
        self.dhcp.configure_mac(self.driver.mac());
        self.dhcp.start(ip);
        self.iface = SmoltcpIface::init(self.driver.mac(), ip, prefix_len);
        self.iface.ensure_loopback();
        // Ensure /etc/resolv.conf exists with QEMU SLIRP DNS default until DHCP overrides it.
        seed_default_resolv_conf(DEFAULT_DNS);
        let phoenix =
            unsafe { exo_syscall_abi::syscall0(exo_syscall_abi::SYS_EXO_PHOENIX_STATE_GET) };
        if phoenix == exo_syscall_abi::ExoPhoenixStateWire::Normal.as_syscall_arg() as i64 {
            self.isolation.restore();
        }
        self.bootstrapped = true;
    }

    fn tick(&mut self) {
        self.ticks = self.ticks.saturating_add(1);
        // Always pump loopback interface first (even without hardware NIC).
        let _ = self.iface.poll_loopback();
        self.advance_connecting_sockets();

        self.driver.ensure_connected(&self.pool);
        if !self.driver.hardware_ready() {
            self.driver.flush_released(&mut self.device);
            return;
        }

        let mut polls = 0usize;
        while self.iface.poll_one(&mut self.device, &self.pool) {
            polls += 1;
            if polls >= 32 {
                break;
            }
        }
        if polls > 0 && self.pending_arp.active {
            let snap = self.pending_arp.snapshot;
            let len = self.pending_arp.len;
            let mut buf = [0u8; NET_INLINE_DATA_MAX];
            buf[..len].copy_from_slice(&self.pending_arp.data[..len]);
            self.pending_arp.active = false;
            let _ = self.iface.send_socket_data(&snap, &buf[..len]);
        }
        self.poll_dhcp();
        let _ = self.iface.poll_egress(&mut self.device, &self.pool);
        self.driver.flush_tx(&mut self.device, &self.pool);
        self.driver.flush_released(&mut self.device);
        self.advance_connecting_sockets();
    }

    fn advance_connecting_sockets(&mut self) {
        let mut connecting = [(SocketSnapshot::empty(), 0u64); MAX_SOCKETS];
        let count = self.sockets.connecting_sockets(&mut connecting);
        for i in 0..count {
            let (snap, started) = connecting[i];
            match self.iface.tcp_connect_status(&snap) {
                TcpConnectStatus::Established => {
                    self.sockets.complete_tcp_connect_by_handle(snap.handle);
                }
                TcpConnectStatus::Failed => {
                    self.sockets
                        .fail_tcp_connect_by_handle(snap.handle, (-ECONNREFUSED) as i32);
                }
                TcpConnectStatus::Pending => {
                    if self.ticks.saturating_sub(started) >= CONNECT_TIMEOUT_TICKS {
                        self.sockets
                            .fail_tcp_connect_by_handle(snap.handle, (-exo_syscall_abi::ETIMEDOUT) as i32);
                    }
                }
            }
        }
    }

    fn poll_dhcp(&mut self) {
        let mut pkt = [0u8; 576];
        let now_ms = self.ticks.saturating_mul(20);
        // Ingest any incoming DHCP OFFER / ACK packet.
        while let Some(n) = self.iface.dhcp_recv(&mut pkt) {
            if let Some(lease) = self.dhcp.ingest(&pkt[..n], now_ms) {
                let gw = if lease.gateway != 0 {
                    lease.gateway
                } else {
                    DEFAULT_GATEWAY
                };
                self.iface.reconfigure_ipv4(lease.ip, lease.prefix_len, gw);
                self.routes.clear();
                let _ = self.routes.add(0x7f00_0000, 8, 0, 0);
                let on_link = lease.ip & routing::mask(lease.prefix_len);
                let _ = self.routes.add(on_link, lease.prefix_len, 0, 1);
                let _ = self.routes.add(0, 0, gw, 10);
                if lease.dns != 0 {
                    write_resolv_conf(lease.dns);
                }
            }
        }
        // Transmit DISCOVER / REQUEST / RENEW periodically without flooding.
        if self.ticks.saturating_sub(self.dhcp_last_tx_tick) < 50 && self.dhcp_last_tx_tick != 0 {
            return;
        }
        let action = self.dhcp.poll(now_ms);
        let len_opt = match action {
            DhcpAction::Discover => self.dhcp.build_discover(&mut pkt),
            DhcpAction::Request | DhcpAction::Renew => self.dhcp.build_request(&mut pkt),
            DhcpAction::None => None,
        };
        if let Some(len) = len_opt {
            if self.iface.dhcp_send(&pkt[..len]) {
                self.dhcp_last_tx_tick = self.ticks;
            }
        }
    }

    fn flush_released(&mut self) {
        self.driver.flush_released(&mut self.device);
    }

    fn handle_driver_ctrl(&mut self, ctrl: DriverCtrlMsg) {
        if !self.driver.is_active_sender(ctrl.sender_pid) {
            return;
        }
        match ctrl.msg_type {
            NET_CTRL_MAC_REPLY => {
                if ctrl.payload.len() >= core::mem::size_of::<MacReplyMsg>() {
                    let msg = unsafe {
                        core::ptr::read_unaligned(ctrl.payload.as_ptr() as *const MacReplyMsg)
                    };
                    if msg.opcode == NET_CTRL_MAC_REPLY {
                        self.driver.set_active_pid(ctrl.sender_pid);
                        self.driver.set_mac(msg.mac);
                        self.dhcp.configure_mac(msg.mac);
                        self.iface.set_mac(msg.mac);
                        debug_write(b"network_server: mac ready\n");
                    }
                }
            }
            NET_CTRL_RX_READY => {
                self.driver.set_active_pid(ctrl.sender_pid);
                if ctrl.payload.len() >= core::mem::size_of::<RxReadyMsg>() {
                    let msg = unsafe {
                        core::ptr::read_unaligned(ctrl.payload.as_ptr() as *const RxReadyMsg)
                    };
                    if msg.opcode == NET_CTRL_RX_READY {
                        let count = (msg.count as usize).min(msg.entries.len());
                        let mut idx = 0usize;
                        while idx < count {
                            let entry = msg.entries[idx];
                            if self.device.push_rx_from_driver(entry.pool_idx, entry.len) {
                                self.stats.note_rx(entry.len as u64);
                            } else {
                                self.stats.note_rx_drop();
                            }
                            idx += 1;
                        }
                    }
                }
            }
            NET_CTRL_TX_COMPLETE => {
                self.driver.set_active_pid(ctrl.sender_pid);
                if ctrl.payload.len() >= core::mem::size_of::<TxCompleteMsg>() {
                    let msg = unsafe {
                        core::ptr::read_unaligned(ctrl.payload.as_ptr() as *const TxCompleteMsg)
                    };
                    if msg.opcode == NET_CTRL_TX_COMPLETE {
                        let count = (msg.count as usize).min(msg.pool_idx.len());
                        let mut idx = 0usize;
                        while idx < count {
                            self.pool.tx_free(msg.pool_idx[idx]);
                            idx += 1;
                        }
                    }
                }
            }
            _ => {}
        }
        self.tick();
    }

    fn dispatch(&mut self, msg: NetMsg, data: &[u8]) -> NetReply {
        match msg.opcode {
            NET_OP_HANDSHAKE => self.handle_handshake(),
            NET_OP_POLL => self.handle_poll(msg),
            NET_OP_OPEN => self.handle_open(msg),
            NET_OP_BIND => self.handle_bind(msg),
            NET_OP_CONNECT => self.handle_connect(msg),
            NET_OP_LISTEN => self.handle_listen(msg),
            NET_OP_ACCEPT => self.handle_accept(msg),
            NET_OP_SENDTO => self.handle_sendto_data(msg, data),
            NET_OP_SENDMSG => self.handle_sendto_data(msg, data),
            NET_OP_RECVFROM => self.handle_recvfrom_data(msg).0,
            NET_OP_SHUTDOWN => self.handle_shutdown(msg),
            NET_OP_GETSOCKNAME => self.handle_getsockname(msg),
            NET_OP_GETPEERNAME => self.handle_getpeername(msg),
            NET_OP_SOCKETPAIR => self.handle_socketpair(msg),
            NET_OP_SETSOCKOPT => self.handle_setsockopt(msg, data),
            NET_OP_GETSOCKOPT => self.handle_getsockopt(msg),
            NET_OP_CLOSE => self.handle_close(msg),
            _ => NetReply::error(exo_syscall_abi::EINVAL),
        }
    }

    fn dispatch_raw_call(
        &mut self,
        payload: &[u8],
    ) -> (NetReply, [u8; NET_INLINE_DATA_MAX], usize) {
        let Some(msg) = parse_net_msg(payload) else {
            return (
                NetReply::error(exo_syscall_abi::EINVAL),
                [0; NET_INLINE_DATA_MAX],
                0,
            );
        };
        let data = if payload.len() > core::mem::size_of::<NetMsg>() {
            &payload[core::mem::size_of::<NetMsg>()..]
        } else {
            &[]
        };
        match msg.opcode {
            NET_OP_SENDTO | NET_OP_SENDMSG => {
                let reply = self.handle_sendto_data(msg, data);
                self.tick();
                (reply, [0; NET_INLINE_DATA_MAX], 0)
            }
            NET_OP_RECVFROM | NET_OP_RECVMSG => {
                self.tick();
                let result = self.handle_recvfrom_data(msg);
                self.tick();
                result
            }
            NET_OP_POLL => {
                self.tick();
                let reply = self.handle_poll(msg);
                (reply, [0; NET_INLINE_DATA_MAX], 0)
            }
            NET_OP_ACCEPT => {
                self.tick();
                let reply = self.handle_accept(msg);
                self.tick();
                (reply, [0; NET_INLINE_DATA_MAX], 0)
            }
            _ => {
                let reply = self.dispatch(msg, data);
                self.tick();
                (reply, [0; NET_INLINE_DATA_MAX], 0)
            }
        }
    }

    fn handle_handshake(&self) -> NetReply {
        let my_pid = unsafe { exo_syscall_abi::syscall0(exo_syscall_abi::SYS_GETPID) }.max(0) as u32;
        let window_va = NET_DATA_WINDOW.buf.get() as u64;
        NetReply::ok(0)
            .with_u32(0, my_pid)
            .with_u64(8, window_va)
            .with_u32(16, NET_DATA_WINDOW_SIZE as u32)
    }

    fn handle_poll(&mut self, msg: NetMsg) -> NetReply {
        let snap = match self.sockets.snapshot_owned(msg.sender_pid, msg.fd) {
            Ok(s) => s,
            Err(err) => return NetReply::error(err),
        };
        let (mask, avail) = self.iface.poll_socket(&snap);
        NetReply::ok(0)
            .with_u32(0, mask)
            .with_u32(4, avail)
            .with_u32(8, snap.so_error as u32)
    }

    fn handle_socketpair(&mut self, msg: NetMsg) -> NetReply {
        let domain = msg.arg1 as u32;
        let ty = msg.arg2 as u32;
        const AF_UNIX: u32 = 1;
        if domain != AF_UNIX {
            return NetReply::error(exo_syscall_abi::EAFNOSUPPORT);
        }
        const SOCK_STREAM_T: u32 = 1;
        const SOCK_DGRAM_T: u32 = 2;
        let kind = match ty & 0x0f {
            SOCK_STREAM_T | SOCK_DGRAM_T => SocketKind::Udp,
            _ => return NetReply::error(exo_syscall_abi::EINVAL),
        };
        let snap_a = match self.sockets.open(msg.sender_pid, kind) {
            Ok(s) => s,
            Err(e) => return NetReply::error(e),
        };
        let snap_b = match self.sockets.open(msg.sender_pid, kind) {
            Ok(s) => s,
            Err(e) => {
                let _ = self.sockets.close(msg.sender_pid, snap_a.handle);
                return NetReply::error(e);
            }
        };
        let _ = self.iface.register_socket(snap_a.handle, kind);
        let _ = self.iface.register_socket(snap_b.handle, kind);
        let loopback: u32 = 0x7f00_0001;
        let bound_a = match self.sockets.bind(msg.sender_pid, snap_a.handle, loopback, 0) {
            Ok(s) => s,
            Err(e) => return NetReply::error(e),
        };
        let bound_b = match self.sockets.bind(msg.sender_pid, snap_b.handle, loopback, 0) {
            Ok(s) => s,
            Err(e) => return NetReply::error(e),
        };
        let _ = self
            .sockets
            .connect(msg.sender_pid, snap_a.handle, loopback, bound_b.local_port);
        let _ = self
            .sockets
            .connect(msg.sender_pid, snap_b.handle, loopback, bound_a.local_port);
        let mut reply = NetReply::ok(0);
        reply.payload[0..4].copy_from_slice(&snap_a.handle.to_le_bytes());
        reply.payload[4..8].copy_from_slice(&snap_b.handle.to_le_bytes());
        reply
    }

    fn require_route(&mut self, remote_addr: u32) -> Result<(), i64> {
        if remote_addr == 0 || is_loopback_ipv4(remote_addr) {
            return Ok(());
        }
        if self.routes.lookup(remote_addr).is_none() {
            return Err(exo_syscall_abi::ENETUNREACH);
        }
        if self.driver.probe_hardware_now(&self.pool) {
            return Ok(());
        }
        if !self.reported_no_hardware_route {
            debug_write(b"network_server: no hardware route\n");
            self.reported_no_hardware_route = true;
        }
        Err(exo_syscall_abi::ENETDOWN)
    }

    fn handle_open(&mut self, msg: NetMsg) -> NetReply {
        let kind = match SocketKind::from_domain_type_privileged(
            msg.arg1 as u32,
            msg.arg2 as u32,
            msg.arg3,
            msg.sender_pid,
        ) {
            Ok(kind) => kind,
            Err(err) => return NetReply::error(err),
        };
        match self.sockets.open(msg.sender_pid, kind) {
            Ok(snapshot) => match self.iface.register_socket(snapshot.handle, kind) {
                Ok(()) => socket_reply(0, &snapshot),
                Err(err) => {
                    let _ = self.sockets.close(msg.sender_pid, snapshot.handle);
                    NetReply::error(err)
                }
            },
            Err(err) => NetReply::error(err),
        }
    }

    fn handle_bind(&mut self, msg: NetMsg) -> NetReply {
        self.iface.ensure_interface(&mut self.device, &self.pool);
        match self
            .sockets
            .bind(msg.sender_pid, msg.fd, msg.arg1 as u32, msg.arg2 as u16)
        {
            Ok(snapshot) => {
                self.iface.apply_socket_state(&snapshot);
                self.iface.apply_socket_options(&snapshot);
                socket_reply(0, &snapshot)
            }
            Err(err) => NetReply::error(err),
        }
    }

    fn handle_connect(&mut self, msg: NetMsg) -> NetReply {
        let remote_addr = msg.arg1 as u32;
        let remote_port = msg.arg2 as u16;
        let nonblock = (msg.arg4 & 1) != 0;

        if let Err(err) = self.require_route(remote_addr) {
            return NetReply::error(err);
        }
        self.iface.ensure_interface(&mut self.device, &self.pool);
        match self
            .sockets
            .connect_at_tick(msg.sender_pid, msg.fd, remote_addr, remote_port, self.ticks)
        {
            Ok(snapshot) => {
                if snapshot.kind != SocketKind::Tcp {
                    self.iface.apply_socket_state(&snapshot);
                    self.iface.apply_socket_options(&snapshot);
                    return socket_reply(0, &snapshot);
                }
                if snapshot.state == SocketState::Connected {
                    return socket_reply(0, &snapshot);
                }
                self.iface.apply_socket_state(&snapshot);
                self.iface.apply_socket_options(&snapshot);

                // Pump loopback immediately (3-way handshake on 127.0.0.1 completes synchronously)
                // or flush SYN egress to hardware NIC.
                if is_loopback_ipv4(remote_addr) {
                    let _ = self.iface.poll_loopback();
                } else {
                    let _ = self.iface.poll_egress(&mut self.device, &self.pool);
                    self.driver.flush_tx(&mut self.device, &self.pool);
                }

                match self.iface.tcp_connect_status(&snapshot) {
                    TcpConnectStatus::Established => {
                        match self.sockets.complete_tcp_connect(msg.sender_pid, msg.fd) {
                            Ok(connected) => socket_reply(0, &connected),
                            Err(err) => NetReply::error(err),
                        }
                    }
                    TcpConnectStatus::Failed => {
                        self.sockets
                            .fail_tcp_connect_by_handle(snapshot.handle, (-ECONNREFUSED) as i32);
                        NetReply::error(ECONNREFUSED)
                    }
                    TcpConnectStatus::Pending => {
                        if nonblock {
                            NetReply::error(EINPROGRESS)
                        } else {
                            NetReply::error(exo_syscall_abi::EAGAIN)
                        }
                    }
                }
            }
            Err(err) => NetReply::error(err),
        }
    }

    fn handle_listen(&mut self, msg: NetMsg) -> NetReply {
        self.iface.ensure_interface(&mut self.device, &self.pool);
        match self.sockets.listen(msg.sender_pid, msg.fd, msg.arg1 as u32) {
            Ok(snapshot) => {
                self.iface.apply_socket_state(&snapshot);
                socket_reply(0, &snapshot)
            }
            Err(err) => NetReply::error(err),
        }
    }

    fn handle_accept(&mut self, msg: NetMsg) -> NetReply {
        let listener = match self.sockets.snapshot_owned(msg.sender_pid, msg.fd) {
            Ok(s) => s,
            Err(err) => return NetReply::error(err),
        };
        if listener.kind != SocketKind::Tcp || listener.state != SocketState::Listening {
            return NetReply::error(exo_syscall_abi::EINVAL);
        }
        if !self.iface.has_acceptable_connection(listener.handle) {
            return NetReply::error(exo_syscall_abi::EAGAIN);
        }
        // Allocate child handle in SocketTable, transfer established smoltcp slot to it.
        let child = match self.sockets.open_accepted(
            msg.sender_pid,
            listener.local_addr,
            listener.local_port,
            0,
            0,
        ) {
            Ok(s) => s,
            Err(err) => return NetReply::error(err),
        };
        let (local_addr, local_port, remote_addr, remote_port) =
            match self.iface.accept_ready_connection(listener.handle, child.handle) {
                Ok(eps) => eps,
                Err(err) => {
                    let _ = self.sockets.close(msg.sender_pid, child.handle);
                    return NetReply::error(err);
                }
            };
        match self.sockets.update_endpoints(
            msg.sender_pid,
            child.handle,
            local_addr,
            local_port,
            remote_addr,
            remote_port,
        ) {
            Ok(updated) => NetReply::ok(updated.handle as i64)
                .with_u64(0, updated.handle as u64)
                .with_u32(8, remote_addr)
                .with_u16(12, remote_port),
            Err(err) => {
                self.iface.unregister_socket(child.handle);
                let _ = self.sockets.close(msg.sender_pid, child.handle);
                NetReply::error(err)
            }
        }
    }

    fn handle_sendto_data(&mut self, msg: NetMsg, inline_data: &[u8]) -> NetReply {
        let use_window = (msg.arg4 & NET_FLAG_USE_DATA_WINDOW) != 0;
        let len = msg.arg1.min(u32::MAX as u64) as usize;
        if use_window {
            if len > NET_DATA_WINDOW_SIZE {
                return NetReply::error(exo_syscall_abi::EMSGSIZE);
            }
        } else if !inline_data.is_empty() && inline_data.len() != len {
            return NetReply::error(exo_syscall_abi::EINVAL);
        }

        let target_addr = if msg.arg2 != 0 {
            msg.arg2 as u32
        } else {
            match self.sockets.snapshot_owned(msg.sender_pid, msg.fd) {
                Ok(snapshot) => snapshot.remote_addr,
                Err(err) => return NetReply::error(err),
            }
        };
        if let Err(err) = self.require_route(target_addr) {
            return NetReply::error(err);
        }
        let snapshot = match self.sockets.send_to(
            msg.sender_pid,
            msg.fd,
            len as u32,
            msg.arg2 as u32,
            msg.arg3 as u16,
        ) {
            Ok(snapshot) => snapshot,
            Err(err) => return NetReply::error(err),
        };
        self.iface.ensure_interface(&mut self.device, &self.pool);
        self.iface.apply_socket_state(&snapshot);

        // SAFETY: `NET_DATA_WINDOW` is owned by network_server and serialized with Ring 0 `NET_DATA_LOCK`.
        let data: &[u8] = if use_window {
            unsafe { &(&*NET_DATA_WINDOW.buf.get())[..len] }
        } else {
            inline_data
        };

        let sent_len = if !data.is_empty() {
            match self.iface.send_socket_data(&snapshot, data) {
                Ok(sent) => {
                    self.stats.note_tx(sent as u64);
                    sent
                }
                Err(err) => return NetReply::error(err),
            }
        } else {
            0
        };

        if is_loopback_ipv4(snapshot.remote_addr) {
            let _ = self.iface.poll_loopback();
        } else {
            let only_arp_sent = self.iface.poll_egress(&mut self.device, &self.pool);
            self.driver.flush_tx(&mut self.device, &self.pool);
            if only_arp_sent && !data.is_empty() && data.len() <= NET_INLINE_DATA_MAX {
                self.pending_arp.active = true;
                self.pending_arp.snapshot = snapshot;
                self.pending_arp.len = data.len();
                self.pending_arp.data[..data.len()].copy_from_slice(data);
            }
        }
        socket_reply(sent_len as i64, &snapshot)
    }

    fn handle_recvfrom_data(
        &mut self,
        msg: NetMsg,
    ) -> (NetReply, [u8; NET_INLINE_DATA_MAX], usize) {
        let mut inline_out = [0u8; NET_INLINE_DATA_MAX];
        let use_window = (msg.arg3 & NET_FLAG_USE_DATA_WINDOW) != 0;
        let peek = (msg.arg3 & MSG_PEEK) != 0;

        let _ = self.iface.poll_loopback();
        let mut polls = 0usize;
        while self.iface.poll_one(&mut self.device, &self.pool) {
            polls += 1;
            if polls >= 16 {
                break;
            }
        }
        if polls > 0 && self.pending_arp.active {
            let snap = self.pending_arp.snapshot;
            let plen = self.pending_arp.len;
            let mut pbuf = [0u8; NET_INLINE_DATA_MAX];
            pbuf[..plen].copy_from_slice(&self.pending_arp.data[..plen]);
            self.pending_arp.active = false;
            let _ = self.iface.send_socket_data(&snap, &pbuf[..plen]);
            let _ = self.iface.poll_egress(&mut self.device, &self.pool);
            self.driver.flush_tx(&mut self.device, &self.pool);
        }

        let before = match self.sockets.snapshot_owned(msg.sender_pid, msg.fd) {
            Ok(snapshot) => snapshot,
            Err(err) => return (NetReply::error(err), inline_out, 0),
        };
        let max_cap = if use_window {
            NET_DATA_WINDOW_SIZE
        } else {
            NET_INLINE_DATA_MAX
        };
        let budget = (msg.arg1 as usize).min(max_cap);
        if budget == 0 {
            return (socket_reply(0, &before), inline_out, 0);
        }

        let recv_res = if use_window {
            // SAFETY: serialized under NETWORK_SERVICE and Ring 0 `NET_DATA_LOCK`.
            let win = unsafe {
                core::slice::from_raw_parts_mut(NET_DATA_WINDOW.buf.get().cast::<u8>(), budget)
            };
            self.iface.recv_socket_data(&before, win, peek)
        } else {
            self.iface
                .recv_socket_data(&before, &mut inline_out[..budget], peek)
        };

        let (delivered, peer_addr, peer_port) = match recv_res {
            Ok(result) => result,
            Err(err) => return (NetReply::error(err), inline_out, 0),
        };
        let snapshot = if !peek {
            match self
                .sockets
                .record_recv(msg.sender_pid, msg.fd, delivered as u32)
            {
                Ok(snapshot) => snapshot,
                Err(err) => return (NetReply::error(err), inline_out, 0),
            }
        } else {
            before
        };

        // Pump loopback ACKs / window updates right after draining TCP RX buffer!
        if delivered > 0 && is_loopback_ipv4(snapshot.remote_addr) {
            let _ = self.iface.poll_loopback();
        }

        let mut reply = socket_reply(delivered as i64, &snapshot);
        if peer_addr != 0 {
            reply = reply.with_u32(8, peer_addr).with_u16(12, peer_port);
        }
        let inline_len = if use_window { 0 } else { delivered };
        (reply, inline_out, inline_len)
    }

    fn handle_shutdown(&mut self, msg: NetMsg) -> NetReply {
        let how = msg.arg1 as u32;
        match self.sockets.shutdown_how(msg.sender_pid, msg.fd, how) {
            Ok(snapshot) => {
                self.iface
                    .shutdown_tcp(&snapshot, how, &mut self.device, &self.pool);
                self.driver.flush_tx(&mut self.device, &self.pool);
                socket_reply(0, &snapshot)
            }
            Err(err) => NetReply::error(err),
        }
    }

    fn handle_getsockname(&mut self, msg: NetMsg) -> NetReply {
        match self.sockets.snapshot_owned(msg.sender_pid, msg.fd) {
            Ok(snapshot) => {
                let local_addr = if snapshot.local_addr != 0 {
                    snapshot.local_addr
                } else if is_loopback_ipv4(snapshot.remote_addr) {
                    0x7f00_0001
                } else if snapshot.state == SocketState::Connected {
                    self.iface.ip()
                } else {
                    0
                };
                NetReply::ok(0)
                    .with_u64(0, snapshot.handle as u64)
                    .with_u32(8, local_addr)
                    .with_u16(12, snapshot.local_port)
            }
            Err(err) => NetReply::error(err),
        }
    }

    fn handle_getpeername(&mut self, msg: NetMsg) -> NetReply {
        match self.sockets.snapshot_owned(msg.sender_pid, msg.fd) {
            Ok(snapshot)
                if snapshot.remote_addr != 0
                    && (snapshot.remote_port != 0 || snapshot.kind == SocketKind::Raw) =>
            {
                NetReply::ok(0)
                    .with_u64(0, snapshot.handle as u64)
                    .with_u32(8, snapshot.remote_addr)
                    .with_u16(12, snapshot.remote_port)
            }
            Ok(_) => NetReply::error(exo_syscall_abi::ENOTCONN),
            Err(err) => NetReply::error(err),
        }
    }

    fn handle_setsockopt(&mut self, msg: NetMsg, optval: &[u8]) -> NetReply {
        let level = msg.arg1 as i32;
        let optname = msg.arg2 as i32;
        match self
            .sockets
            .set_option(msg.sender_pid, msg.fd, level, optname, optval)
        {
            Ok(snapshot) => {
                self.iface.apply_socket_options(&snapshot);
                socket_reply(0, &snapshot)
            }
            Err(err) => NetReply::error(err),
        }
    }

    fn handle_getsockopt(&mut self, msg: NetMsg) -> NetReply {
        let level = msg.arg1 as i32;
        let optname = msg.arg2 as i32;
        match self
            .sockets
            .get_option(msg.sender_pid, msg.fd, level, optname)
        {
            Ok((snapshot, val_bytes, val_len)) => {
                let mut reply = socket_reply(0, &snapshot);
                reply.payload[16..24].copy_from_slice(&val_bytes);
                reply = reply.with_u32(24, val_len as u32);
                reply
            }
            Err(err) => NetReply::error(err),
        }
    }

    fn handle_close(&mut self, msg: NetMsg) -> NetReply {
        match self.sockets.close(msg.sender_pid, msg.fd) {
            Ok(snapshot) => {
                self.iface
                    .close_socket_graceful(&snapshot, &mut self.device, &self.pool);
                self.driver.flush_tx(&mut self.device, &self.pool);
                socket_reply(0, &snapshot)
            }
            Err(err) => NetReply::error(err),
        }
    }
}

static NETWORK_SERVICE: Mutex<NetworkService> = Mutex::new(NetworkService::new());

#[no_mangle]
pub extern "C" fn _start() -> ! {
    register_endpoint();
    {
        let mut service = NETWORK_SERVICE.lock();
        service.bootstrap();
    }

    let mut raw = [0u8; RAW_MSG_SIZE];
    loop {
        let n = match recv_raw(&mut raw) {
            Ok(n) => n,
            Err(_) => {
                let mut service = NETWORK_SERVICE.lock();
                service.tick();
                service.flush_released();
                continue;
            }
        };
        if n == 0 {
            NETWORK_SERVICE.lock().tick();
            continue;
        }

        if let Some(call) = parse_raw_call(&raw[..n]) {
            let (reply, data, data_len) = {
                let mut service = NETWORK_SERVICE.lock();
                service.dispatch_raw_call(call.payload)
            };
            let _ = if data_len == 0 {
                send_rpc_reply(call.reply_ep, call.cookie, &reply)
            } else {
                send_rpc_reply_with_data(call.reply_ep, call.cookie, &reply, &data[..data_len])
            };
        } else if let Some(ctrl) = parse_driver_ctrl(&raw[..n]) {
            NETWORK_SERVICE.lock().handle_driver_ctrl(ctrl);
        } else {
            NETWORK_SERVICE.lock().tick();
        }
    }
}

fn seed_default_resolv_conf(dns_ip: u32) {
    let path = b"/etc/resolv.conf\0";
    let fd =
        unsafe { exo_syscall_abi::syscall2(exo_syscall_abi::SYS_OPEN, path.as_ptr() as u64, 0) };
    if fd >= 0 {
        let _ = unsafe { exo_syscall_abi::syscall1(exo_syscall_abi::SYS_CLOSE, fd as u64) };
        return;
    }
    write_resolv_conf(dns_ip);
}

fn write_resolv_conf(dns_ip: u32) {
    // Ensure /etc exists (ignore EEXIST).
    let etc = b"/etc\0";
    let _ = unsafe { exo_syscall_abi::syscall2(exo_syscall_abi::SYS_MKDIR, etc.as_ptr() as u64, 0o755) };
    let path = b"/etc/resolv.conf\0";
    // O_WRONLY (1) | O_CREAT (0x40) | O_TRUNC (0x200)
    let fd = unsafe {
        exo_syscall_abi::syscall3(
            exo_syscall_abi::SYS_OPEN,
            path.as_ptr() as u64,
            1 | 0x40 | 0x200,
            0o644,
        )
    };
    if fd < 0 {
        return;
    }
    let mut line = [0u8; 48];
    let prefix = b"nameserver ";
    line[..prefix.len()].copy_from_slice(prefix);
    let mut pos = prefix.len();
    let octets = dns_ip.to_be_bytes();
    for (i, &oct) in octets.iter().enumerate() {
        pos += fmt_u8(oct, &mut line[pos..]);
        if i < 3 {
            line[pos] = b'.';
            pos += 1;
        }
    }
    line[pos] = b'\n';
    pos += 1;
    let _ = unsafe {
        exo_syscall_abi::syscall3(
            exo_syscall_abi::SYS_WRITE,
            fd as u64,
            line.as_ptr() as u64,
            pos as u64,
        )
    };
    let _ = unsafe { exo_syscall_abi::syscall1(exo_syscall_abi::SYS_CLOSE, fd as u64) };
}

fn fmt_u8(v: u8, out: &mut [u8]) -> usize {
    if v >= 100 {
        out[0] = b'0' + (v / 100);
        out[1] = b'0' + ((v / 10) % 10);
        out[2] = b'0' + (v % 10);
        3
    } else if v >= 10 {
        out[0] = b'0' + (v / 10);
        out[1] = b'0' + (v % 10);
        2
    } else {
        out[0] = b'0' + v;
        1
    }
}

fn configured_ipv4() -> (u32, u8) {
    let mut buf = [0u8; 128];
    let path = b"/etc/network.conf\0";
    let fd =
        unsafe { exo_syscall_abi::syscall2(exo_syscall_abi::SYS_OPEN, path.as_ptr() as u64, 0) };
    if fd < 0 {
        return (DEFAULT_IPV4, DEFAULT_PREFIX_LEN);
    }
    let n = unsafe {
        exo_syscall_abi::syscall3(
            exo_syscall_abi::SYS_READ,
            fd as u64,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        )
    };
    let _ = unsafe { exo_syscall_abi::syscall1(exo_syscall_abi::SYS_CLOSE, fd as u64) };
    if n <= 0 {
        return (DEFAULT_IPV4, DEFAULT_PREFIX_LEN);
    }
    parse_network_config(&buf[..n as usize]).unwrap_or((DEFAULT_IPV4, DEFAULT_PREFIX_LEN))
}

fn parse_network_config(buf: &[u8]) -> Option<(u32, u8)> {
    let mut i = 0usize;
    while i < buf.len() {
        if buf[i].is_ascii_digit() {
            return parse_ipv4_cidr(&buf[i..]);
        }
        i += 1;
    }
    None
}

fn parse_ipv4_cidr(buf: &[u8]) -> Option<(u32, u8)> {
    let mut i = 0usize;
    let mut octets = [0u8; 4];
    let mut part = 0usize;
    while part < 4 {
        let mut value = 0u32;
        let mut digits = 0usize;
        while i < buf.len() && buf[i].is_ascii_digit() {
            value = value
                .saturating_mul(10)
                .saturating_add((buf[i] - b'0') as u32);
            if value > 255 {
                return None;
            }
            digits += 1;
            i += 1;
        }
        if digits == 0 {
            return None;
        }
        octets[part] = value as u8;
        if part != 3 {
            if i >= buf.len() || buf[i] != b'.' {
                return None;
            }
            i += 1;
        }
        part += 1;
    }

    let mut prefix = DEFAULT_PREFIX_LEN;
    if i < buf.len() && buf[i] == b'/' {
        i += 1;
        let mut value = 0u32;
        let mut digits = 0usize;
        while i < buf.len() && buf[i].is_ascii_digit() {
            value = value
                .saturating_mul(10)
                .saturating_add((buf[i] - b'0') as u32);
            digits += 1;
            i += 1;
        }
        if digits == 0 || value > 32 {
            return None;
        }
        prefix = value as u8;
    }

    Some((
        ((octets[0] as u32) << 24)
            | ((octets[1] as u32) << 16)
            | ((octets[2] as u32) << 8)
            | octets[3] as u32,
        prefix,
    ))
}

fn socket_reply(status: i64, snapshot: &SocketSnapshot) -> NetReply {
    NetReply::ok(status)
        .with_u64(0, snapshot.handle as u64)
        .with_u32(
            8,
            if snapshot.remote_addr != 0 {
                snapshot.remote_addr
            } else {
                snapshot.local_addr
            },
        )
        .with_u16(
            12,
            if snapshot.remote_port != 0 {
                snapshot.remote_port
            } else {
                snapshot.local_port
            },
        )
        .with_u64(16, snapshot.tx_bytes)
        .with_u64(24, snapshot.rx_bytes)
        .with_u32(32, snapshot.pending_rx)
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    debug_write(b"network_server: panic\n");
    let _ = unsafe {
        exo_syscall_abi::syscall1(
            exo_syscall_abi::SYS_EXO_PHOENIX_STATE_SET,
            exo_syscall_abi::ExoPhoenixStateWire::Normal.as_syscall_arg(),
        )
    };
    unsafe {
        let _ = exo_syscall_abi::syscall1(exo_syscall_abi::SYS_EXIT, 1);
    }
    loop {
        core::hint::spin_loop();
    }
}

fn debug_write(bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    unsafe {
        let _ = exo_syscall_abi::syscall3(
            exo_syscall_abi::SYS_EXO_LOG,
            bytes.as_ptr() as u64,
            bytes.len() as u64,
            1,
        );
    }
}

fn debug_errno(prefix: &[u8], err: i64) {
    debug_write(prefix);
    let negative = err < 0;
    let mut value = if negative {
        err.wrapping_neg() as u64
    } else {
        err as u64
    };
    if negative {
        debug_write(b"-");
    }
    let mut digits = [0u8; 20];
    let mut pos = digits.len();
    if value == 0 {
        pos -= 1;
        digits[pos] = b'0';
    } else {
        while value != 0 {
            pos -= 1;
            digits[pos] = b'0' + (value % 10) as u8;
            value /= 10;
        }
    }
    debug_write(&digits[pos..]);
    debug_write(b"\n");
}
