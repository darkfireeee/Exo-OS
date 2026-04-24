#![no_std]
#![no_main]

//! # network_server — plan de contrôle réseau Ring 1
//!
//! Ce serveur fournit une surface réseau cohérente pour les phases GI-04..06 :
//! - table de sockets TCP/UDP/RAW par processus ;
//! - table de routage et état lien ;
//! - journal des envois/réceptions en file bornée ;
//! - points de contrôle backend (virtio, DPDK, XDP).

use core::panic::PanicInfo;

use spin::Mutex;

mod dpdk_bridge;
mod socket;
mod stack;
mod xdp;

use dpdk_bridge::{BackendMode, DpdkBridge};
use socket::api::{
    read_u16, read_u32, read_u64, recv_request, register_endpoint, send_heartbeat, send_reply,
    NetworkReply, NetworkRequest, NETWORK_MSG_BACKEND_SET, NETWORK_MSG_BIND, NETWORK_MSG_CLOSE,
    NETWORK_MSG_CONNECT, NETWORK_MSG_DRIVER_ATTACH, NETWORK_MSG_HEARTBEAT, NETWORK_MSG_ICMP_ECHO,
    NETWORK_MSG_LINK_SET, NETWORK_MSG_RECV, NETWORK_MSG_ROUTE_ADD, NETWORK_MSG_ROUTE_QUERY,
    NETWORK_MSG_SEND, NETWORK_MSG_SOCKET_OPEN, NETWORK_MSG_STATS, NETWORK_MSG_XDP_ATTACH,
};
use socket::bsd_socket::{SocketKind, SocketTable};
use socket::io_uring_sock::{InflightQueue, OperationKind};
use stack::ethernet::EthernetPort;
use stack::icmp::IcmpTracker;
use stack::ip::RouteTable;
use stack::tcp::TcpControlPlane;
use stack::udp::UdpPortAllocator;
use xdp::XdpProgramTable;

struct NetworkService {
    sockets: SocketTable,
    inflight: InflightQueue,
    ethernet: EthernetPort,
    routes: RouteTable,
    udp_ports: UdpPortAllocator,
    tcp: TcpControlPlane,
    icmp: IcmpTracker,
    backend: DpdkBridge,
    xdp: XdpProgramTable,
}

impl NetworkService {
    const fn new() -> Self {
        Self {
            sockets: SocketTable::new(),
            inflight: InflightQueue::new(),
            ethernet: EthernetPort::new(),
            routes: RouteTable::new(),
            udp_ports: UdpPortAllocator::new(),
            tcp: TcpControlPlane::new(),
            icmp: IcmpTracker::new(),
            backend: DpdkBridge::new(),
            xdp: XdpProgramTable::new(),
        }
    }

    fn handle_open(&mut self, sender_pid: u32, payload: &[u8]) -> NetworkReply {
        let raw_kind = match read_u32(payload, 0) {
            Ok(value) => value,
            Err(err) => return NetworkReply::error(err),
        };
        let flags = match read_u32(payload, 4) {
            Ok(value) => value,
            Err(err) => return NetworkReply::error(err),
        };

        let Some(kind) = SocketKind::from_u32(raw_kind) else {
            return NetworkReply::error(exo_syscall_abi::EAFNOSUPPORT);
        };

        match self.sockets.open(sender_pid, kind, flags) {
            Ok(snapshot) => NetworkReply::ok(
                snapshot.handle,
                snapshot.owner_pid as u64,
                self.sockets.count_by_owner(sender_pid) as u64,
                snapshot.kind.as_u32() | (self.sockets.active_count() << 8),
            ),
            Err(err) => NetworkReply::error(err),
        }
    }

    fn handle_bind(&mut self, sender_pid: u32, payload: &[u8]) -> NetworkReply {
        let handle = match read_u64(payload, 0) {
            Ok(value) => value,
            Err(err) => return NetworkReply::error(err),
        };
        let local_addr = match read_u32(payload, 8) {
            Ok(value) => value,
            Err(err) => return NetworkReply::error(err),
        };
        let mut local_port = match read_u16(payload, 12) {
            Ok(value) => value,
            Err(err) => return NetworkReply::error(err),
        };

        if local_port == 0 {
            local_port = self.udp_ports.allocate();
        }

        match self
            .sockets
            .bind(sender_pid, handle, local_addr, local_port)
        {
            Ok(snapshot) => NetworkReply::ok(
                snapshot.handle,
                snapshot.local_addr as u64,
                snapshot.local_port as u64,
                snapshot.state.as_u32() | (snapshot.flags << 8),
            ),
            Err(err) => NetworkReply::error(err),
        }
    }

    fn handle_connect(&mut self, sender_pid: u32, payload: &[u8]) -> NetworkReply {
        let handle = match read_u64(payload, 0) {
            Ok(value) => value,
            Err(err) => return NetworkReply::error(err),
        };
        let remote_addr = match read_u32(payload, 8) {
            Ok(value) => value,
            Err(err) => return NetworkReply::error(err),
        };
        let remote_port = match read_u16(payload, 12) {
            Ok(value) => value,
            Err(err) => return NetworkReply::error(err),
        };

        if self.routes.lookup(remote_addr).is_none() && remote_addr != 0x7f00_0001 {
            return NetworkReply::error(exo_syscall_abi::ENETUNREACH);
        }

        let socket = match self.sockets.snapshot_owned(sender_pid, handle) {
            Ok(snapshot) => snapshot,
            Err(err) => return NetworkReply::error(err),
        };
        if socket.local_port == 0 {
            let port = self.udp_ports.allocate();
            if let Err(err) =
                self.sockets
                    .assign_ephemeral_port(sender_pid, handle, socket.local_addr, port)
            {
                return NetworkReply::error(err);
            }
        }

        match self
            .sockets
            .connect(sender_pid, handle, remote_addr, remote_port)
        {
            Ok(snapshot) => {
                if snapshot.kind == SocketKind::Tcp {
                    self.tcp.activate(snapshot.handle);
                }
                NetworkReply::ok(
                    snapshot.handle,
                    snapshot.remote_addr as u64,
                    snapshot.remote_port as u64,
                    snapshot.state.as_u32() | ((snapshot.nice_queue_hint() as u32) << 8),
                )
            }
            Err(err) => NetworkReply::error(err),
        }
    }

    fn handle_send(&mut self, sender_pid: u32, payload: &[u8]) -> NetworkReply {
        let handle = match read_u64(payload, 0) {
            Ok(value) => value,
            Err(err) => return NetworkReply::error(err),
        };
        let len = match read_u32(payload, 8) {
            Ok(value) => value,
            Err(err) => return NetworkReply::error(err),
        };
        let _flags = match read_u32(payload, 12) {
            Ok(value) => value,
            Err(err) => return NetworkReply::error(err),
        };

        let socket = match self.sockets.snapshot_owned(sender_pid, handle) {
            Ok(snapshot) => snapshot,
            Err(err) => return NetworkReply::error(err),
        };

        if self.ethernet.snapshot().driver_pid == 0 {
            return NetworkReply::error(exo_syscall_abi::ENODEV);
        }
        if self.ethernet.snapshot().link_state != stack::ethernet::LinkState::Up {
            return NetworkReply::error(exo_syscall_abi::ENETDOWN);
        }

        match socket.kind {
            SocketKind::Udp => {
                if let Err(err) = self.udp_ports.validate_len(len) {
                    return NetworkReply::error(err);
                }
            }
            SocketKind::Tcp | SocketKind::Raw => {}
        }

        let route = if socket.remote_addr == 0x7f00_0001 {
            None
        } else {
            self.routes.lookup(socket.remote_addr)
        };
        if socket.remote_addr != 0x7f00_0001 && route.is_none() {
            return NetworkReply::error(exo_syscall_abi::ENETUNREACH);
        }

        let cookie = match self.inflight.submit_send(handle, len) {
            Ok(cookie) => cookie,
            Err(err) => return NetworkReply::error(err),
        };
        let updated = match self.sockets.note_send(sender_pid, handle, len) {
            Ok(snapshot) => snapshot,
            Err(err) => return NetworkReply::error(err),
        };
        self.ethernet.record_tx(1);
        if updated.kind == SocketKind::Tcp {
            self.tcp.note_send(handle, len);
        }

        if self.backend.snapshot().mode == BackendMode::Xdp {
            self.xdp.record_packet(sender_pid, false);
        }

        // Boucle locale minimale : un socket connecté reçoit une complétion bornée.
        let _ = self
            .sockets
            .inject_rx(handle, len.min(self.udp_ports.budget().max_datagram));

        NetworkReply::ok(
            updated.handle,
            len as u64,
            cookie,
            route
                .map(|snapshot| (snapshot.interface_id as u32) | (snapshot.flags << 8))
                .unwrap_or(updated.pending_tx),
        )
    }

    fn handle_recv(&mut self, sender_pid: u32, payload: &[u8]) -> NetworkReply {
        let handle = match read_u64(payload, 0) {
            Ok(value) => value,
            Err(err) => return NetworkReply::error(err),
        };
        let budget = match read_u32(payload, 8) {
            Ok(value) => value,
            Err(err) => return NetworkReply::error(err),
        };

        match self.sockets.take_rx(sender_pid, handle, budget) {
            Ok(snapshot) => {
                self.ethernet.record_rx(1);
                if snapshot.kind == SocketKind::Tcp {
                    let delivered = budget.min(snapshot.pending_rx.saturating_add(budget));
                    self.tcp.note_recv(handle, delivered);
                }
                let completion = self.inflight.complete_next_for(handle);
                NetworkReply::ok(
                    snapshot.handle,
                    snapshot.rx_bytes,
                    completion.map(|entry| entry.cookie).unwrap_or(0),
                    completion
                        .map(|entry| entry.op.as_u32() | (entry.len << 8))
                        .unwrap_or(OperationKind::Recv.as_u32()),
                )
            }
            Err(err) if err == exo_syscall_abi::EAGAIN => {
                let _ = self.sockets.queue_recv(sender_pid, handle);
                let _ = self.inflight.submit_recv(handle, budget);
                NetworkReply::error(err)
            }
            Err(err) => NetworkReply::error(err),
        }
    }

    fn handle_close(&mut self, sender_pid: u32, payload: &[u8]) -> NetworkReply {
        let handle = match read_u64(payload, 0) {
            Ok(value) => value,
            Err(err) => return NetworkReply::error(err),
        };

        match self.sockets.close(sender_pid, handle) {
            Ok(snapshot) => {
                self.tcp.close(handle);
                let cancelled = self.inflight.cancel_handle(handle);
                NetworkReply::ok(
                    snapshot.handle,
                    cancelled as u64,
                    snapshot.tx_bytes,
                    snapshot.flags,
                )
            }
            Err(err) => NetworkReply::error(err),
        }
    }

    fn handle_route_add(&mut self, sender_pid: u32, payload: &[u8]) -> NetworkReply {
        if sender_pid != 1 && sender_pid != 6 {
            return NetworkReply::error(exo_syscall_abi::EPERM);
        }

        let destination = match read_u32(payload, 0) {
            Ok(value) => value,
            Err(err) => return NetworkReply::error(err),
        };
        let prefix_len = match payload.get(4) {
            Some(value) => *value,
            None => return NetworkReply::error(exo_syscall_abi::EINVAL),
        };
        let metric = match read_u16(payload, 6) {
            Ok(value) => value,
            Err(err) => return NetworkReply::error(err),
        };
        let next_hop = match read_u32(payload, 8) {
            Ok(value) => value,
            Err(err) => return NetworkReply::error(err),
        };
        let interface_id = match read_u16(payload, 12) {
            Ok(value) => value,
            Err(err) => return NetworkReply::error(err),
        };
        let flags = match read_u32(payload, 16) {
            Ok(value) => value,
            Err(err) => return NetworkReply::error(err),
        };

        match self.routes.add_route(
            destination,
            prefix_len,
            next_hop,
            metric,
            interface_id,
            flags,
        ) {
            Ok(route) => NetworkReply::ok(
                route.destination as u64,
                route.next_hop as u64,
                route.metric as u64,
                ((route.interface_id as u32) << 8) | route.prefix_len as u32,
            ),
            Err(err) => NetworkReply::error(err),
        }
    }

    fn handle_route_query(&mut self, payload: &[u8]) -> NetworkReply {
        let target = match read_u32(payload, 0) {
            Ok(value) => value,
            Err(err) => return NetworkReply::error(err),
        };

        match self.routes.lookup(target) {
            Some(route) => NetworkReply::ok(
                target as u64,
                route.next_hop as u64,
                route.metric as u64,
                ((route.interface_id as u32) << 8) | route.prefix_len as u32,
            ),
            None => NetworkReply::error(exo_syscall_abi::ENOENT),
        }
    }

    fn handle_driver_attach(&mut self, sender_pid: u32, payload: &[u8]) -> NetworkReply {
        if sender_pid != 1 && sender_pid != 6 && sender_pid != 9 {
            return NetworkReply::error(exo_syscall_abi::EPERM);
        }

        let driver_pid = match read_u32(payload, 0) {
            Ok(value) => value,
            Err(err) => return NetworkReply::error(err),
        };
        let mtu = match read_u32(payload, 4) {
            Ok(value) => value,
            Err(err) => return NetworkReply::error(err),
        };
        let mac_bits = match read_u64(payload, 8) {
            Ok(value) => value,
            Err(err) => return NetworkReply::error(err),
        };
        let queue_pairs = match read_u16(payload, 16) {
            Ok(value) => value,
            Err(err) => return NetworkReply::error(err),
        };
        let backend_mode = match read_u32(payload, 20) {
            Ok(value) => value,
            Err(err) => return NetworkReply::error(err),
        };

        let snapshot = self.ethernet.attach_driver(
            driver_pid,
            mtu,
            [
                (mac_bits & 0xff) as u8,
                ((mac_bits >> 8) & 0xff) as u8,
                ((mac_bits >> 16) & 0xff) as u8,
                ((mac_bits >> 24) & 0xff) as u8,
                ((mac_bits >> 32) & 0xff) as u8,
                ((mac_bits >> 40) & 0xff) as u8,
            ],
            queue_pairs,
        );
        let mode = BackendMode::from_u32(backend_mode).unwrap_or(BackendMode::Virtio);
        let backend = self.backend.configure(mode, snapshot.queue_pairs, 1);
        NetworkReply::ok(
            snapshot.driver_pid as u64,
            snapshot.mtu as u64,
            mac_low64(snapshot.mac),
            snapshot.link_state.as_u32()
                | (backend.mode.as_u32() << 8)
                | ((snapshot.queue_pairs as u32) << 16),
        )
    }

    fn handle_link_set(&mut self, sender_pid: u32, payload: &[u8]) -> NetworkReply {
        if sender_pid != 1 && sender_pid != 6 && sender_pid != self.ethernet.snapshot().driver_pid {
            return NetworkReply::error(exo_syscall_abi::EPERM);
        }

        let up = match read_u32(payload, 0) {
            Ok(value) => value != 0,
            Err(err) => return NetworkReply::error(err),
        };
        let rx_frames = match read_u32(payload, 4) {
            Ok(value) => value,
            Err(err) => return NetworkReply::error(err),
        };
        let tx_frames = match read_u32(payload, 8) {
            Ok(value) => value,
            Err(err) => return NetworkReply::error(err),
        };
        let drops = match read_u32(payload, 12) {
            Ok(value) => value,
            Err(err) => return NetworkReply::error(err),
        };

        let snapshot = self.ethernet.set_link_state(up);
        self.ethernet.record_rx(rx_frames);
        self.ethernet.record_tx(tx_frames);
        self.ethernet.record_drop(drops);
        NetworkReply::ok(
            snapshot.driver_pid as u64,
            self.ethernet.snapshot().tx_frames,
            self.ethernet.snapshot().rx_frames,
            snapshot.link_state.as_u32()
                | ((self.ethernet.snapshot().drop_frames.min(u32::MAX as u64) as u32) << 8),
        )
    }

    fn handle_icmp_echo(&mut self, payload: &[u8]) -> NetworkReply {
        let target = match read_u32(payload, 0) {
            Ok(value) => value,
            Err(err) => return NetworkReply::error(err),
        };
        let payload_len = match read_u16(payload, 4) {
            Ok(value) => value,
            Err(err) => return NetworkReply::error(err),
        };

        if self.routes.lookup(target).is_none() && target != 0x7f00_0001 {
            return NetworkReply::error(exo_syscall_abi::ENETUNREACH);
        }

        let echo = match self.icmp.issue_echo(target, payload_len) {
            Ok(snapshot) => snapshot,
            Err(err) => return NetworkReply::error(err),
        };
        let latency_ms = (payload_len as u32 / 64).saturating_add(1);
        let _ = self.icmp.complete(echo.token, latency_ms);
        let _ = self.inflight.submit_echo(echo.token, payload_len as u32);
        NetworkReply::ok(
            echo.token,
            target as u64,
            ((latency_ms as u64) << 48)
                | ((echo.payload_len as u64) << 32)
                | echo.last_latency_ms as u64,
            echo.completed_count.saturating_add(1) | (echo.sent_count.min(0xffff) << 16),
        )
    }

    fn handle_stats(&mut self, sender_pid: u32, payload: &[u8]) -> NetworkReply {
        let selector = match read_u32(payload, 0) {
            Ok(value) => value,
            Err(err) => return NetworkReply::error(err),
        };
        let handle = match read_u64(payload, 8) {
            Ok(value) => value,
            Err(err) => return NetworkReply::error(err),
        };

        match selector {
            0 => {
                let snapshot = self.ethernet.snapshot();
                let backend = self.backend.snapshot();
                NetworkReply::ok(
                    snapshot.driver_pid as u64,
                    snapshot.tx_frames,
                    snapshot.rx_frames,
                    snapshot.link_state.as_u32() | (backend.mode.as_u32() << 8),
                )
            }
            1 => match self.sockets.snapshot_owned(sender_pid, handle) {
                Ok(snapshot) => NetworkReply::ok(
                    snapshot.handle,
                    snapshot.tx_bytes,
                    snapshot.rx_bytes,
                    snapshot.state.as_u32() | (snapshot.kind.as_u32() << 8),
                ),
                Err(err) => NetworkReply::error(err),
            },
            2 => match self.xdp.snapshot(sender_pid) {
                Some(snapshot) => NetworkReply::ok(
                    ((snapshot.owner_pid as u64) << 32) | snapshot.prog_id as u64,
                    snapshot.packets,
                    snapshot.drops,
                    snapshot.flags,
                ),
                None => NetworkReply::error(exo_syscall_abi::ENOENT),
            },
            3 => {
                let snapshot = self.backend.snapshot();
                NetworkReply::ok(
                    snapshot.mode.as_u32() as u64,
                    snapshot.lcore_mask,
                    snapshot.queue_pairs as u64,
                    snapshot.attached as u32,
                )
            }
            4 => match self.tcp.snapshot(handle) {
                Some(snapshot) => NetworkReply::ok(
                    snapshot.handle,
                    snapshot.sent_bytes,
                    snapshot.recv_bytes,
                    snapshot.cwnd_bytes ^ (snapshot.rtt_ms << 16),
                ),
                None => NetworkReply::error(exo_syscall_abi::ENOENT),
            },
            5 => NetworkReply::ok(
                self.routes.count() as u64,
                self.inflight.depth() as u64,
                self.xdp.count() as u64,
                self.udp_ports.budget().queue_slots as u32,
            ),
            _ => NetworkReply::error(exo_syscall_abi::EINVAL),
        }
    }

    fn handle_xdp_attach(&mut self, sender_pid: u32, payload: &[u8]) -> NetworkReply {
        let prog_id = match read_u32(payload, 0) {
            Ok(value) => value,
            Err(err) => return NetworkReply::error(err),
        };
        let flags = match read_u32(payload, 4) {
            Ok(value) => value,
            Err(err) => return NetworkReply::error(err),
        };

        match self.xdp.attach(sender_pid, prog_id, flags) {
            Ok(snapshot) => NetworkReply::ok(
                snapshot.prog_id as u64,
                snapshot.packets,
                snapshot.drops,
                snapshot.flags,
            ),
            Err(err) => NetworkReply::error(err),
        }
    }

    fn handle_backend_set(&mut self, sender_pid: u32, payload: &[u8]) -> NetworkReply {
        if sender_pid != 1 && sender_pid != 6 {
            return NetworkReply::error(exo_syscall_abi::EPERM);
        }

        let mode = match read_u32(payload, 0) {
            Ok(value) => value,
            Err(err) => return NetworkReply::error(err),
        };
        let queue_pairs = match read_u16(payload, 4) {
            Ok(value) => value,
            Err(err) => return NetworkReply::error(err),
        };
        let lcore_mask = match read_u64(payload, 8) {
            Ok(value) => value,
            Err(err) => return NetworkReply::error(err),
        };

        let Some(mode) = BackendMode::from_u32(mode) else {
            return NetworkReply::error(exo_syscall_abi::EINVAL);
        };
        let snapshot = self.backend.configure(mode, queue_pairs, lcore_mask);
        NetworkReply::ok(
            snapshot.mode.as_u32() as u64,
            snapshot.lcore_mask,
            snapshot.queue_pairs as u64,
            snapshot.attached as u32,
        )
    }
}

static NETWORK_SERVICE: Mutex<NetworkService> = Mutex::new(NetworkService::new());

#[no_mangle]
pub extern "C" fn _start() -> ! {
    register_endpoint();
    let mut request = NetworkRequest::zeroed();

    loop {
        match recv_request(&mut request) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(_) => continue,
        }

        let reply = if request.msg_type == NETWORK_MSG_HEARTBEAT {
            send_heartbeat()
        } else {
            dispatch(&request)
        };

        let _ = send_reply(request.sender_pid, &reply);
    }
}

fn dispatch(request: &NetworkRequest) -> NetworkReply {
    let mut service = NETWORK_SERVICE.lock();

    match request.msg_type {
        NETWORK_MSG_SOCKET_OPEN => service.handle_open(request.sender_pid, &request.payload),
        NETWORK_MSG_BIND => service.handle_bind(request.sender_pid, &request.payload),
        NETWORK_MSG_CONNECT => service.handle_connect(request.sender_pid, &request.payload),
        NETWORK_MSG_SEND => service.handle_send(request.sender_pid, &request.payload),
        NETWORK_MSG_RECV => service.handle_recv(request.sender_pid, &request.payload),
        NETWORK_MSG_CLOSE => service.handle_close(request.sender_pid, &request.payload),
        NETWORK_MSG_ROUTE_ADD => service.handle_route_add(request.sender_pid, &request.payload),
        NETWORK_MSG_ROUTE_QUERY => service.handle_route_query(&request.payload),
        NETWORK_MSG_DRIVER_ATTACH => {
            service.handle_driver_attach(request.sender_pid, &request.payload)
        }
        NETWORK_MSG_LINK_SET => service.handle_link_set(request.sender_pid, &request.payload),
        NETWORK_MSG_ICMP_ECHO => service.handle_icmp_echo(&request.payload),
        NETWORK_MSG_STATS => service.handle_stats(request.sender_pid, &request.payload),
        NETWORK_MSG_XDP_ATTACH => service.handle_xdp_attach(request.sender_pid, &request.payload),
        NETWORK_MSG_BACKEND_SET => service.handle_backend_set(request.sender_pid, &request.payload),
        _ => NetworkReply::error(exo_syscall_abi::EINVAL),
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        // SAFETY: panic terminale pour un serveur no_std monothread.
        unsafe {
            core::arch::asm!("hlt", options(nostack, nomem));
        }
    }
}

fn mac_low64(mac: [u8; 6]) -> u64 {
    (mac[0] as u64)
        | ((mac[1] as u64) << 8)
        | ((mac[2] as u64) << 16)
        | ((mac[3] as u64) << 24)
        | ((mac[4] as u64) << 32)
        | ((mac[5] as u64) << 40)
}
