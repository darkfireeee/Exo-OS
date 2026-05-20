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
mod driver_link;
mod isolation;
mod protocol;
mod smoltcp_iface;
mod socket_table;
mod tcp_store;
mod virtio_device;

use buf_pool::{NetBufPool, VIRTIO_NET_HDR_SIZE_LEGACY};
use driver_link::DriverLink;
use isolation::IsolationState;
use protocol::{
    parse_net_msg, parse_raw_call, recv_raw, register_endpoint, send_rpc_reply, NetMsg, NetReply,
    NET_OP_ACCEPT, NET_OP_BIND, NET_OP_CLOSE, NET_OP_CONNECT, NET_OP_GETPEERNAME,
    NET_OP_GETSOCKNAME, NET_OP_GETSOCKOPT, NET_OP_LISTEN, NET_OP_OPEN, NET_OP_RECVFROM,
    NET_OP_RECVMSG, NET_OP_SENDMSG, NET_OP_SENDTO, NET_OP_SETSOCKOPT, NET_OP_SHUTDOWN,
    NET_OP_SOCKETPAIR, RAW_MSG_SIZE,
};
use smoltcp_iface::SmoltcpIface;
use socket_table::{SocketKind, SocketTable};
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
    tcp_store: TcpStateStore,
    isolation: IsolationState,
    bootstrapped: bool,
    unsupported_msg_ops: u64,
}

impl NetworkService {
    const fn new() -> Self {
        Self {
            sockets: SocketTable::new(),
            pool: NetBufPool::empty(),
            driver: DriverLink::empty(),
            device: ExoNetDevice::new(),
            iface: SmoltcpIface::empty(),
            tcp_store: TcpStateStore::new_empty(),
            isolation: IsolationState::new(),
            bootstrapped: false,
            unsupported_msg_ops: 0,
        }
    }

    fn bootstrap(&mut self) {
        if self.bootstrapped {
            return;
        }
        self.pool =
            NetBufPool::init(VIRTIO_NET_HDR_SIZE_LEGACY).unwrap_or_else(|_| NetBufPool::empty());
        self.driver = DriverLink::connect_virtio_net(&self.pool);
        let (ip, prefix_len) = configured_ipv4();
        self.iface = SmoltcpIface::init(self.driver.mac(), ip, prefix_len);
        let phoenix =
            unsafe { exo_syscall_abi::syscall0(exo_syscall_abi::SYS_EXO_PHOENIX_STATE_GET) };
        if phoenix == exo_syscall_abi::ExoPhoenixStateWire::Normal.as_syscall_arg() as i64 {
            self.isolation.restore();
        }
        self.bootstrapped = true;
    }

    fn tick(&mut self) {
        self.iface.poll_one(&mut self.device, &self.pool);
        self.iface.poll_egress(&mut self.device, &self.pool);
        self.driver.flush_released(&mut self.device);
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

    fn unsupported_msg_reply(&mut self) -> NetReply {
        self.unsupported_msg_ops = self.unsupported_msg_ops.saturating_add(1);
        NetReply::error(exo_syscall_abi::EOPNOTSUPP)
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
        match self
            .sockets
            .connect(msg.sender_pid, msg.fd, msg.arg1 as u32, msg.arg2 as u16)
        {
            Ok(snapshot) => {
                self.iface.apply_socket_state(&snapshot);
                socket_reply(0, &snapshot)
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
        let len = msg.arg1.min(u32::MAX as u64) as u32;
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
        if self.pool.ready() {
            let _ = self.device.submit_tx(&self.pool, len as usize);
        }
        socket_reply(len as i64, &snapshot)
    }

    fn handle_recvfrom(&mut self, msg: NetMsg) -> NetReply {
        let before = match self.sockets.snapshot_owned(msg.sender_pid, msg.fd) {
            Ok(snapshot) => snapshot,
            Err(err) => return NetReply::error(err),
        };
        let budget = msg.arg1.min(u32::MAX as u64) as u32;
        let delivered = before.pending_rx.min(budget.max(1));
        match self.sockets.recv_from(msg.sender_pid, msg.fd, budget) {
            Ok(snapshot) => socket_reply(delivered as i64, &snapshot),
            Err(err) => NetReply::error(err),
        }
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
            Ok(snapshot) if snapshot.remote_port != 0 => socket_reply(0, &snapshot),
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
                NETWORK_SERVICE.lock().tick();
                continue;
            }
        };
        if n == 0 {
            NETWORK_SERVICE.lock().tick();
            continue;
        }

        if let Some(call) = parse_raw_call(&raw[..n]) {
            let reply = match parse_net_msg(call.payload) {
                Some(msg) => NETWORK_SERVICE.lock().dispatch_and_tick(msg),
                None => NetReply::error(exo_syscall_abi::EINVAL),
            };
            let _ = send_rpc_reply(call.reply_ep, call.cookie, &reply);
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
