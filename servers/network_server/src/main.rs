#![no_std]
#![no_main]
#![allow(dead_code)]

//! network_server V4 -- serveur reseau IPC-only.
//!
//! Cette implementation suit le plan `EXOOS_NETWORK_MODULE_V4.md`:
//! protocole fixe NetMsg/NetReply, table de sockets bornee, pool DMA optionnel,
//! lien driver par IPC et cycle RX `released_buf -> RxReleaseMsg`.

use core::panic::PanicInfo;

use spin::Mutex;

mod buf_pool;
mod dhcp;
mod driver_link;
mod icmp;
mod isolation;
mod protocol;
mod routing;
mod smoltcp_iface;
mod socket_table;
mod stats;
mod tcp_store;
mod virtio_device;

use buf_pool::{NetBufPool, VIRTIO_NET_HDR_SIZE_MODERN};
use driver_link::DriverLink;
use isolation::IsolationState;
use protocol::{
    parse_driver_ctrl, parse_net_msg, parse_raw_call, recv_raw, register_endpoint, send_rpc_reply,
    send_rpc_reply_with_data, DriverCtrlMsg, MacReplyMsg, NetMsg, NetReply, RxReadyMsg,
    TxCompleteMsg, NET_CTRL_MAC_REPLY, NET_CTRL_RX_READY, NET_CTRL_TX_COMPLETE,
    NET_INLINE_DATA_MAX, NET_OP_ACCEPT, NET_OP_BIND, NET_OP_CLOSE, NET_OP_CONNECT,
    NET_OP_GETPEERNAME, NET_OP_GETSOCKNAME, NET_OP_GETSOCKOPT, NET_OP_LISTEN, NET_OP_OPEN,
    NET_OP_RECVFROM, NET_OP_RECVMSG, NET_OP_SENDMSG, NET_OP_SENDTO, NET_OP_SETSOCKOPT,
    NET_OP_SHUTDOWN, NET_OP_SOCKETPAIR, RAW_MSG_SIZE,
};
use routing::RouteTable;
use smoltcp_iface::{SmoltcpIface, TcpConnectStatus};
use socket_table::{SocketKind, SocketTable};
use stats::NetStats;
use tcp_store::TcpStateStore;
use virtio_device::ExoNetDevice;

const DEFAULT_IPV4: u32 = 0x0a00_020f;
const DEFAULT_PREFIX_LEN: u8 = 24;

struct NetworkService {
    sockets: SocketTable,
    pool: NetBufPool,
    driver: DriverLink,
    device: ExoNetDevice,
    iface: SmoltcpIface,
    routes: RouteTable,
    dhcp: dhcp::DhcpClient,
    stats: NetStats,
    tcp_store: TcpStateStore,
    isolation: IsolationState,
    bootstrapped: bool,
    ticks: u64,
    unsupported_msg_ops: u64,
    reported_no_hardware_route: bool,
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
            stats: NetStats::new(),
            tcp_store: TcpStateStore::new_empty(),
            isolation: IsolationState::new(),
            bootstrapped: false,
            ticks: 0,
            unsupported_msg_ops: 0,
            reported_no_hardware_route: false,
        }
    }

    fn bootstrap(&mut self) {
        if self.bootstrapped {
            return;
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
        let on_link = ip & routing::mask(prefix_len);
        let _ = self.routes.add(on_link, prefix_len, 0, 0);
        let _ = self.routes.add(0, 0, 0x0a00_0202, 10);
        self.dhcp.configure_mac(self.driver.mac());
        self.dhcp.start(ip);
        self.iface = SmoltcpIface::init(self.driver.mac(), ip, prefix_len);
        let phoenix =
            unsafe { exo_syscall_abi::syscall0(exo_syscall_abi::SYS_EXO_PHOENIX_STATE_GET) };
        if phoenix == exo_syscall_abi::ExoPhoenixStateWire::Normal.as_syscall_arg() as i64 {
            self.isolation.restore();
        }
        self.bootstrapped = true;
    }

    fn tick(&mut self) {
        self.ticks = self.ticks.saturating_add(1);
        self.driver.ensure_connected(&self.pool);
        if !self.driver.hardware_ready() {
            self.driver.flush_released(&mut self.device);
            let _ = self.dhcp.poll(self.ticks);
            return;
        }
        let mut polls = 0usize;
        while self.iface.poll_one(&mut self.device, &self.pool) {
            polls += 1;
            if polls >= 32 {
                break;
            }
        }
        self.iface.poll_egress(&mut self.device, &self.pool);
        self.driver.flush_tx(&mut self.device, &self.pool);
        self.driver.flush_released(&mut self.device);
        let _ = self.dhcp.poll(self.ticks);
    }

    fn flush_released(&mut self) {
        self.driver.flush_released(&mut self.device);
    }

    fn handle_driver_ctrl(&mut self, ctrl: DriverCtrlMsg) {
        match ctrl.msg_type {
            NET_CTRL_MAC_REPLY => {
                if ctrl.payload.len() >= core::mem::size_of::<MacReplyMsg>() {
                    let msg = unsafe {
                        core::ptr::read_unaligned(ctrl.payload.as_ptr() as *const MacReplyMsg)
                    };
                    if msg.opcode == NET_CTRL_MAC_REPLY {
                        self.driver.set_mac(msg.mac);
                        self.dhcp.configure_mac(msg.mac);
                        self.iface.set_mac(msg.mac);
                        debug_write(b"network_server: mac ready\n");
                    }
                }
            }
            NET_CTRL_RX_READY => {
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

    fn dispatch(&mut self, msg: NetMsg) -> NetReply {
        match msg.opcode {
            NET_OP_OPEN => self.handle_open(msg),
            NET_OP_BIND => self.handle_bind(msg),
            NET_OP_CONNECT => self.handle_connect(msg),
            NET_OP_LISTEN => self.handle_listen(msg),
            NET_OP_ACCEPT => self.handle_accept(msg),
            NET_OP_SENDTO => self.handle_sendto(msg),
            NET_OP_RECVFROM => self.handle_recvfrom(msg),
            NET_OP_SENDMSG | NET_OP_RECVMSG => self.unsupported_msg_reply(),
            NET_OP_SHUTDOWN => self.handle_shutdown(msg),
            NET_OP_GETSOCKNAME => self.handle_getsockname(msg),
            NET_OP_GETPEERNAME => self.handle_getpeername(msg),
            NET_OP_SOCKETPAIR => self.unsupported_msg_reply(),
            NET_OP_SETSOCKOPT => self.handle_setsockopt(msg),
            NET_OP_GETSOCKOPT => self.handle_getsockopt(msg),
            NET_OP_CLOSE => self.handle_close(msg),
            _ => NetReply::error(exo_syscall_abi::EINVAL),
        }
    }

    fn dispatch_and_tick(&mut self, msg: NetMsg) -> NetReply {
        let reply = self.dispatch(msg);
        self.tick();
        reply
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
            NET_OP_SENDTO => {
                let reply = self.handle_sendto_data(msg, data);
                self.tick();
                (reply, [0; NET_INLINE_DATA_MAX], 0)
            }
            NET_OP_RECVFROM => {
                self.tick();
                let result = self.handle_recvfrom_data(msg);
                self.tick();
                result
            }
            _ => (self.dispatch_and_tick(msg), [0; NET_INLINE_DATA_MAX], 0),
        }
    }

    fn unsupported_msg_reply(&mut self) -> NetReply {
        self.unsupported_msg_ops = self.unsupported_msg_ops.saturating_add(1);
        NetReply::error(exo_syscall_abi::EOPNOTSUPP)
    }

    fn require_hardware_route(&mut self, remote_addr: u32) -> Result<(), i64> {
        if remote_addr == 0 {
            return Ok(());
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
        let kind = match SocketKind::from_domain_type(msg.arg1 as u32, msg.arg2 as u32, msg.arg3) {
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
        match self
            .sockets
            .bind(msg.sender_pid, msg.fd, msg.arg1 as u32, msg.arg2 as u16)
        {
            Ok(snapshot) => {
                self.iface.apply_socket_state(&snapshot);
                socket_reply(0, &snapshot)
            }
            Err(err) => NetReply::error(err),
        }
    }

    fn handle_connect(&mut self, msg: NetMsg) -> NetReply {
        if let Err(err) = self.require_hardware_route(msg.arg1 as u32) {
            return NetReply::error(err);
        }
        match self
            .sockets
            .connect(msg.sender_pid, msg.fd, msg.arg1 as u32, msg.arg2 as u16)
        {
            Ok(snapshot) => {
                if snapshot.kind != SocketKind::Tcp {
                    self.iface.apply_socket_state(&snapshot);
                    return socket_reply(0, &snapshot);
                }
                match self.iface.tcp_connect_status(&snapshot) {
                    TcpConnectStatus::Failed => {
                        self.iface.poll_egress(&mut self.device, &self.pool);
                        self.iface.apply_socket_state(&snapshot);
                        match self.iface.tcp_connect_status(&snapshot) {
                            TcpConnectStatus::Established => {
                                match self.sockets.complete_tcp_connect(msg.sender_pid, msg.fd) {
                                    Ok(connected) => socket_reply(0, &connected),
                                    Err(err) => NetReply::error(err),
                                }
                            }
                            _ => NetReply::error(exo_syscall_abi::EAGAIN),
                        }
                    }
                    TcpConnectStatus::Established => {
                        match self.sockets.complete_tcp_connect(msg.sender_pid, msg.fd) {
                            Ok(connected) => socket_reply(0, &connected),
                            Err(err) => NetReply::error(err),
                        }
                    }
                    TcpConnectStatus::Pending => {
                        self.iface.poll_egress(&mut self.device, &self.pool);
                        NetReply::error(exo_syscall_abi::EAGAIN)
                    }
                }
            }
            Err(err) => NetReply::error(err),
        }
    }

    fn handle_listen(&mut self, msg: NetMsg) -> NetReply {
        match self.sockets.listen(msg.sender_pid, msg.fd, msg.arg1 as u32) {
            Ok(snapshot) => {
                self.iface.apply_socket_state(&snapshot);
                socket_reply(0, &snapshot)
            }
            Err(err) => NetReply::error(err),
        }
    }

    fn handle_accept(&mut self, msg: NetMsg) -> NetReply {
        match self.sockets.accept(msg.sender_pid, msg.fd) {
            Ok(snapshot) => socket_reply(snapshot.handle as i64, &snapshot),
            Err(err) => NetReply::error(err),
        }
    }

    fn handle_sendto(&mut self, msg: NetMsg) -> NetReply {
        self.handle_sendto_data(msg, &[])
    }

    fn handle_sendto_data(&mut self, msg: NetMsg, data: &[u8]) -> NetReply {
        let len = msg.arg1.min(u32::MAX as u64) as u32;
        if !data.is_empty() && data.len() != len as usize {
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
        if let Err(err) = self.require_hardware_route(target_addr) {
            return NetReply::error(err);
        }
        let snapshot = match self.sockets.send_to(
            msg.sender_pid,
            msg.fd,
            len,
            msg.arg2 as u32,
            msg.arg3 as u16,
        ) {
            Ok(snapshot) => snapshot,
            Err(err) => return NetReply::error(err),
        };
        self.iface.apply_socket_state(&snapshot);
        if !data.is_empty() {
            match self.iface.send_socket_data(&snapshot, data) {
                Ok(sent) if sent == data.len() => self.stats.note_tx(sent as u64),
                Ok(_) => return NetReply::error(exo_syscall_abi::EAGAIN),
                Err(err) => return NetReply::error(err),
            }
        }
        socket_reply(len as i64, &snapshot)
    }

    fn handle_recvfrom(&mut self, msg: NetMsg) -> NetReply {
        let (reply, _, _) = self.handle_recvfrom_data(msg);
        reply
    }

    fn handle_recvfrom_data(
        &mut self,
        msg: NetMsg,
    ) -> (NetReply, [u8; NET_INLINE_DATA_MAX], usize) {
        let mut data = [0u8; NET_INLINE_DATA_MAX];
        let before = match self.sockets.snapshot_owned(msg.sender_pid, msg.fd) {
            Ok(snapshot) => snapshot,
            Err(err) => return (NetReply::error(err), data, 0),
        };
        let budget = (msg.arg1 as usize).min(data.len());
        if budget == 0 {
            return (socket_reply(0, &before), data, 0);
        }
        let (delivered, peer_addr, peer_port) =
            match self.iface.recv_socket_data(&before, &mut data[..budget]) {
                Ok(result) => result,
                Err(err) => return (NetReply::error(err), data, 0),
            };
        let snapshot = match self
            .sockets
            .record_recv(msg.sender_pid, msg.fd, delivered as u32)
        {
            Ok(snapshot) => snapshot,
            Err(err) => return (NetReply::error(err), data, 0),
        };
        let mut reply = socket_reply(delivered as i64, &snapshot);
        if peer_addr != 0 {
            reply = reply.with_u32(8, peer_addr).with_u16(12, peer_port);
        }
        (reply, data, delivered)
    }

    fn handle_shutdown(&mut self, msg: NetMsg) -> NetReply {
        match self.sockets.shutdown(msg.sender_pid, msg.fd) {
            Ok(snapshot) => socket_reply(0, &snapshot),
            Err(err) => NetReply::error(err),
        }
    }

    fn handle_getsockname(&mut self, msg: NetMsg) -> NetReply {
        match self.sockets.snapshot_owned(msg.sender_pid, msg.fd) {
            Ok(snapshot) => socket_reply(0, &snapshot),
            Err(err) => NetReply::error(err),
        }
    }

    fn handle_getpeername(&mut self, msg: NetMsg) -> NetReply {
        match self.sockets.snapshot_owned(msg.sender_pid, msg.fd) {
            Ok(snapshot)
                if snapshot.remote_addr != 0
                    && (snapshot.remote_port != 0 || snapshot.kind == SocketKind::Raw) =>
            {
                socket_reply(0, &snapshot)
            }
            Ok(_) => NetReply::error(exo_syscall_abi::ENOTCONN),
            Err(err) => NetReply::error(err),
        }
    }

    fn handle_setsockopt(&mut self, msg: NetMsg) -> NetReply {
        match self.sockets.snapshot_owned(msg.sender_pid, msg.fd) {
            Ok(snapshot) => socket_reply(0, &snapshot),
            Err(err) => NetReply::error(err),
        }
    }

    fn handle_getsockopt(&mut self, msg: NetMsg) -> NetReply {
        match self.sockets.snapshot_owned(msg.sender_pid, msg.fd) {
            Ok(snapshot) => socket_reply(0, &snapshot)
                .with_u32(16, 0)
                .with_u32(36, self.unsupported_msg_ops.min(u32::MAX as u64) as u32),
            Err(err) => NetReply::error(err),
        }
    }

    fn handle_close(&mut self, msg: NetMsg) -> NetReply {
        match self.sockets.close(msg.sender_pid, msg.fd) {
            Ok(snapshot) => {
                self.iface.unregister_socket(snapshot.handle);
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

fn socket_reply(status: i64, snapshot: &socket_table::SocketSnapshot) -> NetReply {
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
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nostack, nomem));
        }
    }
}

fn debug_write(bytes: &[u8]) {
    for &byte in bytes {
        #[cfg(target_arch = "x86_64")]
        unsafe {
            core::arch::asm!("out 0xE9, al", in("al") byte, options(nomem, nostack));
        }
        #[cfg(not(target_arch = "x86_64"))]
        let _ = byte;
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
