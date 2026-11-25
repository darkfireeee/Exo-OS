use super::loopback_dev::LoopbackIpDevice;
use super::phy_dev::ExoSmoltcpDevice;
use super::storage::{
    endpoint_to_v4, ip_addr, ip_address_to_v4, ip_listen_addr, ipv4_from_u32, socket_set,
    UDP_BUFFER_SIZE,
};
use super::{is_loopback_ipv4, SmoltcpIface, TcpConnectStatus};
use crate::buf_pool::NetBufPool;
use crate::socket_table::{SocketKind, SocketSnapshot, SocketState};
use crate::virtio_device::ExoNetDevice;
use exo_syscall_abi as syscall;
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::socket::{icmp, tcp, udp};
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpAddress, IpCidr, IpEndpoint, IpListenEndpoint};

impl SmoltcpIface {
    pub fn send_socket_data(
        &mut self,
        snapshot: &SocketSnapshot,
        data: &[u8],
    ) -> Result<usize, i64> {
        if data.is_empty() {
            return Ok(0);
        }
        let Some(slot) = self.socket_slot_by_exo_handle(snapshot.handle) else {
            return Err(syscall::EBADF);
        };
        let Some(handle) = self.socket_handles[slot] else {
            return Err(syscall::EBADF);
        };
        if snapshot.remote_addr == 0
            || (snapshot.remote_port == 0 && snapshot.kind != SocketKind::Raw)
        {
            return Err(syscall::ENOTCONN);
        }

        let mut sockets = socket_set();
        match snapshot.kind {
            SocketKind::Udp => {
                if data.len() > UDP_BUFFER_SIZE {
                    return Err(syscall::EMSGSIZE);
                }
                let socket = sockets.get_mut::<udp::Socket>(handle);
                if !socket.is_open() && snapshot.local_port != 0 {
                    let _ = socket.bind(IpListenEndpoint {
                        addr: ip_listen_addr(snapshot.local_addr),
                        port: snapshot.local_port,
                    });
                }
                if !socket.can_send() {
                    return Err(syscall::EAGAIN);
                }
                socket
                    .send_slice(
                        data,
                        IpEndpoint::new(ip_addr(snapshot.remote_addr), snapshot.remote_port),
                    )
                    .map_err(|_| syscall::EAGAIN)?;
                Ok(data.len())
            }
            SocketKind::Tcp => {
                let socket = sockets.get_mut::<tcp::Socket>(handle);
                if !socket.may_send() {
                    return Err(syscall::EPIPE);
                }
                if !socket.can_send() {
                    return Err(syscall::EAGAIN);
                }
                match socket.send_slice(data) {
                    Ok(0) => Err(syscall::EAGAIN),
                    Ok(n) => Ok(n),
                    Err(_) => Err(syscall::EPIPE),
                }
            }
            SocketKind::Raw => {
                let socket = sockets.get_mut::<icmp::Socket>(handle);
                let icmp_ident = if data.len() >= 6 {
                    u16::from_be_bytes([data[4], data[5]])
                } else {
                    snapshot.local_port.max(1)
                };
                if !socket.is_open() {
                    socket
                        .bind(icmp::Endpoint::Ident(icmp_ident))
                        .map_err(|_| syscall::EINVAL)?;
                }
                socket
                    .send_slice(data, ip_addr(snapshot.remote_addr))
                    .map_err(|_| syscall::EAGAIN)?;
                Ok(data.len())
            }
        }
    }

    pub fn recv_socket_data(
        &mut self,
        snapshot: &SocketSnapshot,
        out: &mut [u8],
        peek: bool,
    ) -> Result<(usize, u32, u16), i64> {
        if out.is_empty() || snapshot.shutdown_rd {
            return Ok((0, snapshot.remote_addr, snapshot.remote_port));
        }
        let Some(slot) = self.socket_slot_by_exo_handle(snapshot.handle) else {
            return Err(syscall::EBADF);
        };
        let Some(handle) = self.socket_handles[slot] else {
            return Err(syscall::EBADF);
        };

        let mut sockets = socket_set();
        match snapshot.kind {
            SocketKind::Udp => {
                let socket = sockets.get_mut::<udp::Socket>(handle);
                if peek {
                    let (payload, meta) = socket.peek().map_err(|_| syscall::EAGAIN)?;
                    let n = payload.len().min(out.len());
                    out[..n].copy_from_slice(&payload[..n]);
                    let (addr, port) = endpoint_to_v4(meta.endpoint)
                        .unwrap_or((snapshot.remote_addr, snapshot.remote_port));
                    Ok((n, addr, port))
                } else {
                    let (n, meta) = socket.recv_slice(out).map_err(|_| syscall::EAGAIN)?;
                    let (addr, port) = endpoint_to_v4(meta.endpoint)
                        .unwrap_or((snapshot.remote_addr, snapshot.remote_port));
                    Ok((n, addr, port))
                }
            }
            SocketKind::Tcp => {
                let socket = sockets.get_mut::<tcp::Socket>(handle);
                if !socket.can_recv() {
                    if !socket.may_recv() {
                        // Peer closed / FIN received -> clean TCP EOF (0 bytes).
                        return Ok((0, snapshot.remote_addr, snapshot.remote_port));
                    }
                    return Err(syscall::EAGAIN);
                }
                let res = if peek {
                    socket.peek_slice(out)
                } else {
                    socket.recv_slice(out)
                };
                match res {
                    Ok(n) => Ok((n, snapshot.remote_addr, snapshot.remote_port)),
                    Err(tcp::RecvError::Finished) => {
                        Ok((0, snapshot.remote_addr, snapshot.remote_port))
                    }
                    Err(tcp::RecvError::InvalidState) => {
                        if matches!(
                            socket.state(),
                            tcp::State::Closed
                                | tcp::State::TimeWait
                                | tcp::State::CloseWait
                                | tcp::State::LastAck
                                | tcp::State::Closing
                        ) {
                            Ok((0, snapshot.remote_addr, snapshot.remote_port))
                        } else {
                            Err(syscall::ENOTCONN)
                        }
                    }
                }
            }
            SocketKind::Raw => {
                let socket = sockets.get_mut::<icmp::Socket>(handle);
                let (n, addr) = socket.recv_slice(out).map_err(|_| syscall::EAGAIN)?;
                Ok((n, ip_address_to_v4(addr).unwrap_or(snapshot.remote_addr), 0))
            }
        }
    }

    pub const fn ip(&self) -> u32 {
        self.ip
    }

    pub const fn mac(&self) -> [u8; 6] {
        self.mac
    }

    pub fn set_mac(&mut self, mac: [u8; 6]) {
        if mac == [0; 6] || mac == self.mac {
            return;
        }
        self.mac = mac;
        if let Some(iface) = self.iface.as_mut() {
            iface.set_hardware_addr(EthernetAddress(mac).into());
        }
    }

    pub const fn prefix_len(&self) -> u8 {
        self.prefix_len
    }

    pub fn tcp_connect_status(&self, snapshot: &SocketSnapshot) -> TcpConnectStatus {
        let Some(slot) = self.socket_slot_by_exo_handle(snapshot.handle) else {
            return TcpConnectStatus::Failed;
        };
        let Some(handle) = self.socket_handles[slot] else {
            return TcpConnectStatus::Failed;
        };
        let sockets = socket_set();
        let socket = sockets.get::<tcp::Socket>(handle);
        match socket.state() {
            tcp::State::Established | tcp::State::CloseWait => TcpConnectStatus::Established,
            tcp::State::Closed | tcp::State::TimeWait => TcpConnectStatus::Failed,
            _ => TcpConnectStatus::Pending,
        }
    }

    pub(super) fn ensure_lo_iface(&mut self, now: Instant) {
        if self.lo_iface.is_some() {
            return;
        }
        let mut lo_dev = LoopbackIpDevice;
        let mut config = Config::new(HardwareAddress::Ip);
        config.random_seed = 0x7f00_0001_4558_4f4e;
        let mut iface = Interface::new(config, &mut lo_dev, now);
        iface.update_ip_addrs(|addrs| {
            addrs.clear();
            let _ = addrs.push(IpCidr::new(IpAddress::v4(127, 0, 0, 1), 8));
        });
        self.lo_iface = Some(iface);
    }

    pub(super) fn ensure_iface(&mut self, device: &mut ExoNetDevice, pool: &NetBufPool, now: Instant) {
        if self.iface.is_some() {
            return;
        }

        let mut smol_device = ExoSmoltcpDevice::new(device, pool);
        let mut config = Config::new(EthernetAddress(self.mac).into());
        config.random_seed = (self.ip as u64) ^ 0x4558_4f4e_4554;
        let mut iface = Interface::new(config, &mut smol_device, now);
        let ip = self.ip_cidr();
        iface.update_ip_addrs(|addrs| {
            addrs.clear();
            let _ = addrs.push(ip);
        });
        let gw = if self.gateway != 0 {
            self.gateway
        } else {
            0x0a00_0202
        };
        let _ = iface
            .routes_mut()
            .add_default_ipv4_route(ipv4_from_u32(gw));
        self.iface = Some(iface);
    }

    pub(super) fn ip_cidr(&self) -> IpCidr {
        IpCidr::new(ip_addr(self.ip), self.prefix_len)
    }

    pub(super) fn socket_slot_by_exo_handle(&self, exo_handle: u32) -> Option<usize> {
        self.socket_exo_handles
            .iter()
            .position(|&stored| stored == exo_handle)
    }

    pub(super) fn apply_tcp_state(
        &mut self,
        handle: SocketHandle,
        sockets: &mut SocketSet<'static>,
        snapshot: &SocketSnapshot,
    ) {
        let socket = sockets.get_mut::<tcp::Socket>(handle);
        match snapshot.state {
            SocketState::Listening if snapshot.local_port != 0 => {
                if socket.state() == tcp::State::Closed {
                    let _ = socket.listen(IpListenEndpoint {
                        addr: ip_listen_addr(snapshot.local_addr),
                        port: snapshot.local_port,
                    });
                }
            }
            SocketState::Connecting | SocketState::Connected
                if snapshot.remote_port != 0 && snapshot.local_port != 0 =>
            {
                if socket.state() == tcp::State::Closed {
                    let use_loopback = is_loopback_ipv4(snapshot.remote_addr);
                    let target_iface = if use_loopback {
                        self.lo_iface.as_mut()
                    } else {
                        self.iface.as_mut()
                    };
                    if let Some(iface) = target_iface {
                        let local_ep = if use_loopback {
                            IpListenEndpoint {
                                addr: Some(IpAddress::v4(127, 0, 0, 1)),
                                port: snapshot.local_port,
                            }
                        } else {
                            IpListenEndpoint {
                                addr: ip_listen_addr(snapshot.local_addr),
                                port: snapshot.local_port,
                            }
                        };
                        let _ = socket.connect(
                            iface.context(),
                            IpEndpoint::new(ip_addr(snapshot.remote_addr), snapshot.remote_port),
                            local_ep,
                        );
                    }
                }
            }
            SocketState::Shutdown => socket.close(),
            SocketState::Closed => socket.abort(),
            _ => {}
        }
    }

    pub(super) fn apply_udp_state(
        &mut self,
        handle: SocketHandle,
        sockets: &mut SocketSet<'static>,
        snapshot: &SocketSnapshot,
    ) {
        if snapshot.local_port == 0 {
            return;
        }
        let socket = sockets.get_mut::<udp::Socket>(handle);
        if !socket.is_open() {
            let _ = socket.bind(IpListenEndpoint {
                addr: ip_listen_addr(snapshot.local_addr),
                port: snapshot.local_port,
            });
        }
    }

    pub(super) fn apply_icmp_state(
        &mut self,
        _handle: SocketHandle,
        _sockets: &mut SocketSet<'static>,
        _snapshot: &SocketSnapshot,
    ) {
        // Le bind ICMP est effectué dans send_socket_data avec l'identifiant exact de l'en-tête ICMP.
    }
}
