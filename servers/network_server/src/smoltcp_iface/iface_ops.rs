use super::loopback_dev::{LoopbackIpDevice, LOOPBACK_QUEUE};
use super::phy_dev::{ExoSmoltcpDevice, LAST_EGRESS_SENT_IPV4, LAST_EGRESS_WAS_ARP};
use super::storage::{
    endpoint_to_v4, ip_listen_addr, ipv4_from_u32, make_icmp_socket, make_tcp_socket,
    make_udp_socket, smoltcp_now_ms, socket_set,
};
use super::{
    ListenerPool, SmoltcpIface, EPOLLERR, EPOLLHUP, EPOLLIN, EPOLLOUT, EPOLLRDHUP,
    LISTEN_BACKLOG_SLOTS, MAX_LISTENERS, SOCKET_STORAGE_LEN,
};
use crate::buf_pool::NetBufPool;
use crate::socket_table::{SocketKind, SocketSnapshot, SocketState};
use crate::virtio_device::ExoNetDevice;
use exo_syscall_abi as syscall;
use smoltcp::iface::PollIngressSingleResult;
use smoltcp::socket::{icmp, tcp, udp};
use smoltcp::wire::{IpAddress, IpEndpoint, IpListenEndpoint};

impl SmoltcpIface {
    pub const fn empty() -> Self {
        Self {
            mac: [0; 6],
            ip: 0,
            prefix_len: 0,
            gateway: 0x0a00_0202,
            ingress_ticks: 0,
            egress_ticks: 0,
            iface: None,
            lo_iface: None,
            socket_handles: [None; SOCKET_STORAGE_LEN],
            socket_exo_handles: [0; SOCKET_STORAGE_LEN],
            listener_pools: [ListenerPool::empty(); MAX_LISTENERS],
            dhcp_slot: None,
        }
    }

    pub fn init(mac: [u8; 6], ip: u32, prefix_len: u8) -> Self {
        Self {
            mac,
            ip,
            prefix_len,
            gateway: 0x0a00_0202,
            ingress_ticks: 0,
            egress_ticks: 0,
            iface: None,
            lo_iface: None,
            socket_handles: [None; SOCKET_STORAGE_LEN],
            socket_exo_handles: [0; SOCKET_STORAGE_LEN],
            listener_pools: [ListenerPool::empty(); MAX_LISTENERS],
            dhcp_slot: None,
        }
    }

    fn alloc_free_slot(&self) -> Option<usize> {
        self.socket_handles.iter().position(Option::is_none)
    }

    pub fn register_socket(&mut self, exo_handle: u32, kind: SocketKind) -> Result<(), i64> {
        if self.socket_slot_by_exo_handle(exo_handle).is_some() {
            return Ok(());
        }
        let slot = self.alloc_free_slot().ok_or(syscall::ENOBUFS)?;

        let mut sockets = socket_set();
        let handle = match kind {
            SocketKind::Tcp => sockets.add(make_tcp_socket(slot)),
            SocketKind::Udp => sockets.add(make_udp_socket(slot)),
            SocketKind::Raw => sockets.add(make_icmp_socket(slot)),
        };
        self.socket_handles[slot] = Some(handle);
        self.socket_exo_handles[slot] = exo_handle;
        Ok(())
    }

    pub fn unregister_socket(&mut self, exo_handle: u32) {
        // If this handle owns a listener pool, tear down all extra backlog slots too.
        for pool in &mut self.listener_pools {
            if pool.active && pool.listener_exo_handle == exo_handle {
                let mut sockets = socket_set();
                for s_opt in &mut pool.slots {
                    if let Some(slot) = s_opt.take() {
                        if let Some(h) = self.socket_handles[slot].take() {
                            let _ = sockets.remove(h);
                        }
                        self.socket_exo_handles[slot] = 0;
                    }
                }
                *pool = ListenerPool::empty();
                return;
            }
        }
        let Some(slot) = self.socket_slot_by_exo_handle(exo_handle) else {
            return;
        };
        if let Some(handle) = self.socket_handles[slot].take() {
            let mut sockets = socket_set();
            let _ = sockets.remove(handle);
        }
        self.socket_exo_handles[slot] = 0;
    }

    /// Gracefully closes a TCP socket (sends FIN) and pumps loopback/egress before removing.
    pub fn close_socket_graceful(
        &mut self,
        snapshot: &SocketSnapshot,
        device: &mut ExoNetDevice,
        pool: &NetBufPool,
    ) {
        if snapshot.kind == SocketKind::Tcp && snapshot.state != SocketState::Listening {
            if let Some(slot) = self.socket_slot_by_exo_handle(snapshot.handle) {
                if let Some(handle) = self.socket_handles[slot] {
                    {
                        let mut sockets = socket_set();
                        let sock = sockets.get_mut::<tcp::Socket>(handle);
                        sock.close();
                    }
                    let _ = self.poll_loopback();
                    let _ = self.poll_egress(device, pool);
                }
            }
        }
        self.unregister_socket(snapshot.handle);
    }

    pub fn ensure_interface(&mut self, device: &mut ExoNetDevice, pool: &NetBufPool) {
        let now = smoltcp_now_ms(self.egress_ticks.max(self.ingress_ticks));
        self.ensure_lo_iface(now);
        self.ensure_iface(device, pool, now);
    }

    pub fn ensure_loopback(&mut self) {
        let now = smoltcp_now_ms(self.egress_ticks.max(self.ingress_ticks));
        self.ensure_lo_iface(now);
    }

    pub fn reconfigure_ipv4(&mut self, ip: u32, prefix_len: u8, gateway: u32) {
        if ip == 0 || prefix_len == 0 || prefix_len > 32 {
            return;
        }
        self.ip = ip;
        self.prefix_len = prefix_len;
        if gateway != 0 {
            self.gateway = gateway;
        }
        let cidr = self.ip_cidr();
        let gw = self.gateway;
        if let Some(iface) = self.iface.as_mut() {
            iface.update_ip_addrs(|addrs| {
                addrs.clear();
                let _ = addrs.push(cidr);
            });
            if gw != 0 {
                let _ = iface
                    .routes_mut()
                    .add_default_ipv4_route(ipv4_from_u32(gw));
            }
        }
    }

    /// Polls the in-process `127.0.0.1/8` loopback interface until quiescent (bounded).
    pub fn poll_loopback(&mut self) -> bool {
        let now = smoltcp_now_ms(self.egress_ticks.max(self.ingress_ticks));
        self.ensure_lo_iface(now);
        let Some(lo_iface) = self.lo_iface.as_mut() else {
            return false;
        };
        let mut sockets = socket_set();
        let mut lo_dev = LoopbackIpDevice;
        let mut activity = false;
        for _ in 0..32 {
            let eg = lo_iface.poll_egress(now, &mut lo_dev, &mut sockets);
            let mut ing_any = false;
            for _ in 0..16 {
                match lo_iface.poll_ingress_single(now, &mut lo_dev, &mut sockets) {
                    PollIngressSingleResult::None => break,
                    PollIngressSingleResult::PacketProcessed
                    | PollIngressSingleResult::SocketStateChanged => {
                        ing_any = true;
                        activity = true;
                    }
                }
            }
            let eg2 = lo_iface.poll_egress(now, &mut lo_dev, &mut sockets);
            let q_empty = unsafe { (*LOOPBACK_QUEUE.queue.get()).is_empty() };
            let progressed =
                ing_any || matches!(eg, smoltcp::iface::PollResult::SocketStateChanged)
                    || matches!(eg2, smoltcp::iface::PollResult::SocketStateChanged);
            if progressed {
                activity = true;
            }
            if !progressed && q_empty {
                break;
            }
        }
        activity
    }

    pub fn apply_socket_state(&mut self, snapshot: &SocketSnapshot) {
        if snapshot.kind == SocketKind::Tcp && snapshot.state == SocketState::Listening {
            self.setup_tcp_listener_pool(snapshot);
            return;
        }
        let Some(slot) = self.socket_slot_by_exo_handle(snapshot.handle) else {
            return;
        };
        let Some(handle) = self.socket_handles[slot] else {
            return;
        };
        let mut sockets = socket_set();
        match snapshot.kind {
            SocketKind::Tcp => self.apply_tcp_state(handle, &mut sockets, snapshot),
            SocketKind::Udp => self.apply_udp_state(handle, &mut sockets, snapshot),
            SocketKind::Raw => self.apply_icmp_state(handle, &mut sockets, snapshot),
        }
    }

    pub fn apply_socket_options(&mut self, snapshot: &SocketSnapshot) {
        let Some(slot) = self.socket_slot_by_exo_handle(snapshot.handle) else {
            return;
        };
        let Some(handle) = self.socket_handles[slot] else {
            return;
        };
        let mut sockets = socket_set();
        match snapshot.kind {
            SocketKind::Tcp => {
                let sock = sockets.get_mut::<tcp::Socket>(handle);
                sock.set_nagle_enabled(!snapshot.nodelay);
                sock.set_hop_limit(Some(snapshot.ttl.max(1)));
            }
            SocketKind::Udp => {
                let sock = sockets.get_mut::<udp::Socket>(handle);
                sock.set_hop_limit(Some(snapshot.ttl.max(1)));
            }
            SocketKind::Raw => {
                let sock = sockets.get_mut::<icmp::Socket>(handle);
                sock.set_hop_limit(Some(snapshot.ttl.max(1)));
            }
        }
    }

    fn setup_tcp_listener_pool(&mut self, snapshot: &SocketSnapshot) {
        if snapshot.local_port == 0 {
            return;
        }
        let desired = (snapshot.backlog as usize).clamp(1, LISTEN_BACKLOG_SLOTS);
        // Check if pool already exists for this listener.
        let pool_idx = if let Some(idx) = self
            .listener_pools
            .iter()
            .position(|p| p.active && p.listener_exo_handle == snapshot.handle)
        {
            idx
        } else if let Some(idx) = self.listener_pools.iter().position(|p| !p.active) {
            self.listener_pools[idx] = ListenerPool {
                active: true,
                listener_exo_handle: snapshot.handle,
                local_addr: snapshot.local_addr,
                local_port: snapshot.local_port,
                slots: [None; LISTEN_BACKLOG_SLOTS],
            };
            // Adopt the primary slot already registered for this listener handle.
            if let Some(primary_slot) = self.socket_slot_by_exo_handle(snapshot.handle) {
                self.listener_pools[idx].slots[0] = Some(primary_slot);
            }
            idx
        } else {
            return;
        };

        let listen_ep = IpListenEndpoint {
            addr: ip_listen_addr(snapshot.local_addr),
            port: snapshot.local_port,
        };
        let mut sockets = socket_set();
        for k in 0..desired {
            let slot = if let Some(existing) = self.listener_pools[pool_idx].slots[k] {
                existing
            } else {
                let Some(free) = self.alloc_free_slot() else {
                    break;
                };
                let h = sockets.add(make_tcp_socket(free));
                self.socket_handles[free] = Some(h);
                self.socket_exo_handles[free] = snapshot.handle;
                self.listener_pools[pool_idx].slots[k] = Some(free);
                free
            };
            if let Some(handle) = self.socket_handles[slot] {
                let sock = sockets.get_mut::<tcp::Socket>(handle);
                if sock.state() == tcp::State::Closed {
                    let _ = sock.listen(listen_ep);
                }
            }
        }
    }

    pub fn has_acceptable_connection(&self, listener_exo_handle: u32) -> bool {
        let Some(pool) = self
            .listener_pools
            .iter()
            .find(|p| p.active && p.listener_exo_handle == listener_exo_handle)
        else {
            return false;
        };
        let sockets = socket_set();
        for slot_opt in pool.slots {
            let Some(slot) = slot_opt else {
                continue;
            };
            let Some(handle) = self.socket_handles[slot] else {
                continue;
            };
            let state = sockets.get::<tcp::Socket>(handle).state();
            if !matches!(
                state,
                tcp::State::Listen | tcp::State::Closed | tcp::State::SynReceived
            ) {
                return true;
            }
        }
        false
    }

    /// Detaches one ready accepted TCP connection from `listener_exo_handle` and
    /// binds it to `child_exo_handle`, immediately replenishing a fresh listening slot.
    pub fn accept_ready_connection(
        &mut self,
        listener_exo_handle: u32,
        child_exo_handle: u32,
    ) -> Result<(u32, u16, u32, u16), i64> {
        let Some(pool_idx) = self
            .listener_pools
            .iter()
            .position(|p| p.active && p.listener_exo_handle == listener_exo_handle)
        else {
            return Err(syscall::EINVAL);
        };

        let mut ready_k: Option<usize> = None;
        let mut endpoints: (u32, u16, u32, u16) = (0, 0, 0, 0);
        {
            let sockets = socket_set();
            for (k, slot_opt) in self.listener_pools[pool_idx].slots.iter().enumerate() {
                let Some(slot) = *slot_opt else {
                    continue;
                };
                let Some(handle) = self.socket_handles[slot] else {
                    continue;
                };
                let sock = sockets.get::<tcp::Socket>(handle);
                if !matches!(
                    sock.state(),
                    tcp::State::Listen | tcp::State::Closed | tcp::State::SynReceived
                ) {
                    let (la, lp) = sock
                        .local_endpoint()
                        .and_then(endpoint_to_v4)
                        .unwrap_or((
                            self.listener_pools[pool_idx].local_addr,
                            self.listener_pools[pool_idx].local_port,
                        ));
                    let (ra, rp) = sock
                        .remote_endpoint()
                        .and_then(endpoint_to_v4)
                        .unwrap_or((0, 0));
                    endpoints = (la, lp, ra, rp);
                    ready_k = Some(k);
                    break;
                }
            }
        }

        let Some(k) = ready_k else {
            return Err(syscall::EAGAIN);
        };
        let accepted_slot = self.listener_pools[pool_idx].slots[k].take().unwrap();
        // Reassign the established smoltcp slot directly to `child_exo_handle`.
        self.socket_exo_handles[accepted_slot] = child_exo_handle;

        // Immediately replenish the listener slot with a new listening TCP socket.
        if let Some(new_slot) = self.alloc_free_slot() {
            let listen_ep = IpListenEndpoint {
                addr: ip_listen_addr(self.listener_pools[pool_idx].local_addr),
                port: self.listener_pools[pool_idx].local_port,
            };
            let mut sockets = socket_set();
            let mut sock = make_tcp_socket(new_slot);
            let _ = sock.listen(listen_ep);
            let h = sockets.add(sock);
            self.socket_handles[new_slot] = Some(h);
            self.socket_exo_handles[new_slot] = listener_exo_handle;
            self.listener_pools[pool_idx].slots[k] = Some(new_slot);
        }

        Ok(endpoints)
    }

    /// Computes real Linux `poll`/`epoll` readiness mask + available RX bytes for `snapshot`.
    pub fn poll_socket(&self, snapshot: &SocketSnapshot) -> (u32, u32) {
        if snapshot.kind == SocketKind::Tcp && snapshot.state == SocketState::Listening {
            return if self.has_acceptable_connection(snapshot.handle) {
                (EPOLLIN, 1)
            } else {
                (0, 0)
            };
        }

        let Some(slot) = self.socket_slot_by_exo_handle(snapshot.handle) else {
            let mut m = EPOLLHUP;
            if snapshot.so_error != 0 {
                m |= EPOLLERR | EPOLLOUT;
            }
            return (m, 0);
        };
        let Some(handle) = self.socket_handles[slot] else {
            return (EPOLLHUP, 0);
        };

        let sockets = socket_set();
        let mut mask = 0u32;
        if snapshot.so_error != 0 {
            mask |= EPOLLERR;
        }
        match snapshot.kind {
            SocketKind::Tcp => {
                let sock = sockets.get::<tcp::Socket>(handle);
                if snapshot.state == SocketState::Connecting {
                    match sock.state() {
                        tcp::State::Established | tcp::State::CloseWait => {
                            mask |= EPOLLOUT;
                            if sock.can_recv() || !sock.may_recv() {
                                mask |= EPOLLIN;
                            }
                        }
                        tcp::State::Closed | tcp::State::TimeWait => {
                            mask |= EPOLLOUT | EPOLLERR | EPOLLHUP;
                        }
                        _ => {}
                    }
                    return (mask, sock.recv_queue() as u32);
                }
                if snapshot.state == SocketState::Closed {
                    mask |= EPOLLHUP | EPOLLIN | EPOLLOUT;
                    return (mask, 0);
                }
                let avail = sock.recv_queue() as u32;
                if sock.can_recv() || !sock.may_recv() || snapshot.shutdown_rd {
                    mask |= EPOLLIN;
                }
                if !sock.may_recv() {
                    mask |= EPOLLRDHUP;
                }
                if !snapshot.shutdown_wr && sock.can_send() {
                    mask |= EPOLLOUT;
                }
                if !sock.may_recv() && !sock.may_send() {
                    mask |= EPOLLHUP;
                }
                (mask, avail)
            }
            SocketKind::Udp => {
                let sock = sockets.get::<udp::Socket>(handle);
                let mut avail = 0u32;
                if sock.can_recv() || snapshot.shutdown_rd {
                    mask |= EPOLLIN;
                    if sock.can_recv() {
                        avail = 1;
                    }
                }
                if !snapshot.shutdown_wr && sock.can_send() {
                    mask |= EPOLLOUT;
                }
                (mask, avail)
            }
            SocketKind::Raw => {
                let sock = sockets.get::<icmp::Socket>(handle);
                let avail = if sock.can_recv() { 1 } else { 0 };
                if sock.can_recv() {
                    mask |= EPOLLIN;
                }
                if sock.can_send() {
                    mask |= EPOLLOUT;
                }
                (mask, avail)
            }
        }
    }

    pub fn shutdown_tcp(
        &mut self,
        snapshot: &SocketSnapshot,
        how: u32,
        device: &mut ExoNetDevice,
        pool: &NetBufPool,
    ) {
        if snapshot.kind != SocketKind::Tcp {
            return;
        }
        if how == 1 || how == 2 {
            if let Some(slot) = self.socket_slot_by_exo_handle(snapshot.handle) {
                if let Some(handle) = self.socket_handles[slot] {
                    {
                        let mut sockets = socket_set();
                        let sock = sockets.get_mut::<tcp::Socket>(handle);
                        sock.close();
                    }
                    let _ = self.poll_loopback();
                    let _ = self.poll_egress(device, pool);
                }
            }
        }
    }

    pub fn poll_one(&mut self, device: &mut ExoNetDevice, pool: &NetBufPool) -> bool {
        self.ingress_ticks = self.ingress_ticks.saturating_add(1);
        let now = smoltcp_now_ms(self.ingress_ticks);
        self.ensure_lo_iface(now);
        self.ensure_iface(device, pool, now);

        let Some(iface) = self.iface.as_mut() else {
            return false;
        };
        let mut socket_set = socket_set();
        let mut smol_device = ExoSmoltcpDevice::new(device, pool);
        matches!(
            iface.poll_ingress_single(now, &mut smol_device, &mut socket_set),
            PollIngressSingleResult::PacketProcessed | PollIngressSingleResult::SocketStateChanged
        )
    }

    pub fn poll_egress(&mut self, device: &mut ExoNetDevice, pool: &NetBufPool) -> bool {
        use core::sync::atomic::Ordering;
        LAST_EGRESS_WAS_ARP.store(false, Ordering::Relaxed);
        LAST_EGRESS_SENT_IPV4.store(false, Ordering::Relaxed);

        self.egress_ticks = self.egress_ticks.saturating_add(1);
        let now = smoltcp_now_ms(self.egress_ticks);
        self.ensure_lo_iface(now);
        self.ensure_iface(device, pool, now);

        if let Some(iface) = self.iface.as_mut() {
            let mut socket_set = socket_set();
            let mut smol_device = ExoSmoltcpDevice::new(device, pool);
            let _ = iface.poll_egress(now, &mut smol_device, &mut socket_set);
        }

        LAST_EGRESS_WAS_ARP.load(Ordering::Relaxed) && !LAST_EGRESS_SENT_IPV4.load(Ordering::Relaxed)
    }

    // ── Internal DHCP UDP socket helpers ─────────────────────────────────────

    pub fn ensure_dhcp_socket(&mut self) -> bool {
        if self.dhcp_slot.is_some() {
            return true;
        }
        let Some(slot) = self.alloc_free_slot() else {
            return false;
        };
        let mut sockets = socket_set();
        let mut sock = make_udp_socket(slot);
        let _ = sock.bind(IpListenEndpoint {
            addr: None,
            port: crate::dhcp::DHCP_CLIENT_PORT,
        });
        let h = sockets.add(sock);
        self.socket_handles[slot] = Some(h);
        self.socket_exo_handles[slot] = 0xFFFF_FFFE; // reserved internal DHCP marker
        self.dhcp_slot = Some(slot);
        true
    }

    pub fn dhcp_send(&mut self, packet: &[u8]) -> bool {
        if !self.ensure_dhcp_socket() {
            return false;
        }
        let Some(slot) = self.dhcp_slot else {
            return false;
        };
        let Some(handle) = self.socket_handles[slot] else {
            return false;
        };
        let mut sockets = socket_set();
        let sock = sockets.get_mut::<udp::Socket>(handle);
        if !sock.can_send() {
            return false;
        }
        sock.send_slice(
            packet,
            IpEndpoint::new(IpAddress::v4(255, 255, 255, 255), crate::dhcp::DHCP_SERVER_PORT),
        )
        .is_ok()
    }

    pub fn dhcp_recv(&mut self, out: &mut [u8]) -> Option<usize> {
        let slot = self.dhcp_slot?;
        let handle = self.socket_handles[slot]?;
        let mut sockets = socket_set();
        let sock = sockets.get_mut::<udp::Socket>(handle);
        match sock.recv_slice(out) {
            Ok((n, _)) => Some(n),
            Err(_) => None,
        }
    }

    pub fn drain_bounded(
        &mut self,
        device: &mut ExoNetDevice,
        pool: &NetBufPool,
        max_ingress_polls: usize,
    ) -> bool {
        let mut polls = 0usize;
        while self.poll_one(device, pool) {
            polls = polls.saturating_add(1);
            if polls >= max_ingress_polls {
                return false;
            }
        }
        self.poll_egress(device, pool);
        true
    }

    pub fn drain_all(&mut self, device: &mut ExoNetDevice, pool: &NetBufPool) {
        let _ = self.drain_bounded(device, pool, usize::MAX);
    }

}
