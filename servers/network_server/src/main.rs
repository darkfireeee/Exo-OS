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

struct NetworkService {
    sockets: SocketTable,
    pool: NetBufPool,
    driver: DriverLink,
    device: ExoNetDevice,
    iface: SmoltcpIface,
    tcp_store: TcpStateStore,
    isolation: IsolationState,
    bootstrapped: bool,
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
        }
    }

    fn bootstrap(&mut self) {
        if self.bootstrapped {
            return;
        }
        self.pool =
            NetBufPool::init(VIRTIO_NET_HDR_SIZE_LEGACY).unwrap_or_else(|_| NetBufPool::empty());
        self.driver = DriverLink::connect_virtio_net(&self.pool);
        self.iface = SmoltcpIface::init(self.driver.mac(), 0x0a00_020f, 24);
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
            NET_OP_SENDMSG | NET_OP_RECVMSG => NetReply::error(exo_syscall_abi::ENOTSUP),
            NET_OP_SHUTDOWN => self.handle_shutdown(msg),
            NET_OP_GETSOCKNAME => self.handle_getsockname(msg),
            NET_OP_GETPEERNAME => self.handle_getpeername(msg),
            NET_OP_SOCKETPAIR => NetReply::error(exo_syscall_abi::ENOTSUP),
            NET_OP_SETSOCKOPT => self.handle_setsockopt(msg),
            NET_OP_GETSOCKOPT => self.handle_getsockopt(msg),
            NET_OP_CLOSE => self.handle_close(msg),
            _ => NetReply::error(exo_syscall_abi::EINVAL),
        }
    }

    fn handle_open(&mut self, msg: NetMsg) -> NetReply {
        let kind = match SocketKind::from_domain_type(msg.arg1 as u32, msg.arg2 as u32, msg.arg3) {
            Ok(kind) => kind,
            Err(err) => return NetReply::error(err),
        };
        match self.sockets.open(msg.sender_pid, kind) {
            Ok(snapshot) => socket_reply(0, &snapshot),
            Err(err) => NetReply::error(err),
        }
    }

    fn handle_bind(&mut self, msg: NetMsg) -> NetReply {
        match self
            .sockets
            .bind(msg.sender_pid, msg.fd, msg.arg1 as u32, msg.arg2 as u16)
        {
            Ok(snapshot) => socket_reply(0, &snapshot),
            Err(err) => NetReply::error(err),
        }
    }

    fn handle_connect(&mut self, msg: NetMsg) -> NetReply {
        match self
            .sockets
            .connect(msg.sender_pid, msg.fd, msg.arg1 as u32, msg.arg2 as u16)
        {
            Ok(snapshot) => socket_reply(0, &snapshot),
            Err(err) => NetReply::error(err),
        }
    }

    fn handle_listen(&mut self, msg: NetMsg) -> NetReply {
        match self.sockets.listen(msg.sender_pid, msg.fd, msg.arg1 as u32) {
            Ok(snapshot) => socket_reply(0, &snapshot),
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
            Ok(snapshot) => socket_reply(0, &snapshot).with_u32(16, 0),
            Err(err) => NetReply::error(err),
        }
    }

    fn handle_close(&mut self, msg: NetMsg) -> NetReply {
        match self.sockets.close(msg.sender_pid, msg.fd) {
            Ok(snapshot) => socket_reply(0, &snapshot),
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
                Some(msg) => NETWORK_SERVICE.lock().dispatch(msg),
                None => NetReply::error(exo_syscall_abi::EINVAL),
            };
            let _ = send_rpc_reply(call.reply_ep, call.cookie, &reply);
        }

        NETWORK_SERVICE.lock().tick();
    }
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
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nostack, nomem));
        }
    }
}
