use exo_syscall_abi as syscall;

pub const MAX_SOCKETS: usize = 64;
const HANDLE_TAG: u32 = 0x4000_0000;
const HANDLE_TAG_MASK: u32 = 0xf000_0000;
const HANDLE_GENERATION_MASK: u32 = 0x0fff;
const HANDLE_INDEX_MASK: u32 = 0xffff;
const AF_INET: u32 = 2;
const SOCK_STREAM: u32 = 1;
const SOCK_DGRAM: u32 = 2;
const SOCK_RAW: u32 = 3;
const SOCK_TYPE_MASK: u32 = 0x0f;

/// FIX-11: Strict kernel-attested authorization for `SOCK_RAW`.
/// Removes both the hardcoded static PID list and the insecure `|| sender_pid >= 18` bypass.
/// - `InitServer` / `NetworkServer` may open `SOCK_RAW` (protocol 0 or ICMP=1).
/// - `ExoCompositor` / `Exosh` may open ICMP-only (`protocol == 1`) for built-in ping diagnostics.
/// - Any unprivileged Ring 3 process (`SVC_UNKNOWN`) is strictly denied with `EPERM`.
#[inline]
pub const fn service_allows_raw_socket(sender_pid: u32, service_class: u8, protocol: u32) -> bool {
    if sender_pid == 1
        || service_class == syscall::SVC_INIT_SERVER
        || service_class == syscall::SVC_NETWORK_SERVER
    {
        return protocol == 0 || protocol == 1;
    }
    if protocol == 1
        && (service_class == syscall::SVC_EXO_COMPOSITOR || service_class == syscall::SVC_EXOSH)
    {
        return true;
    }
    false
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketKind {
    Tcp,
    Udp,
    Raw,
}

impl SocketKind {
    pub fn from_domain_type_for_service(
        domain: u32,
        ty: u32,
        protocol: u32,
        sender_pid: u32,
        service_class: u8,
    ) -> Result<Self, i64> {
        if domain != AF_INET {
            return Err(syscall::EAFNOSUPPORT);
        }
        match ty & SOCK_TYPE_MASK {
            SOCK_STREAM => Ok(Self::Tcp),
            SOCK_DGRAM => Ok(Self::Udp),
            SOCK_RAW if protocol == 0 || protocol == 1 => {
                if service_allows_raw_socket(sender_pid, service_class, protocol) {
                    Ok(Self::Raw)
                } else {
                    Err(syscall::EPERM)
                }
            }
            SOCK_RAW => Err(syscall::EOPNOTSUPP),
            _ => Err(syscall::EINVAL),
        }
    }

    pub fn from_domain_type_privileged(
        domain: u32,
        ty: u32,
        protocol: u32,
        sender_pid: u32,
    ) -> Result<Self, i64> {
        let class = if sender_pid == 1 {
            syscall::SVC_INIT_SERVER
        } else {
            syscall::SVC_UNKNOWN
        };
        Self::from_domain_type_for_service(domain, ty, protocol, sender_pid, class)
    }

    pub fn from_domain_type(domain: u32, ty: u32, protocol: u32) -> Result<Self, i64> {
        Self::from_domain_type_privileged(domain, ty, protocol, 1)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SocketState {
    Closed,
    Open,
    Bound,
    Listening,
    Connecting,
    Connected,
    Shutdown,
}

#[derive(Clone, Copy)]
pub struct SocketSnapshot {
    pub handle: u32,
    pub owner_pid: u32,
    pub kind: SocketKind,
    pub state: SocketState,
    pub local_addr: u32,
    pub local_port: u16,
    pub remote_addr: u32,
    pub remote_port: u16,
    pub pending_rx: u32,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub shutdown_rd: bool,
    pub shutdown_wr: bool,
    pub so_error: i32,
    pub backlog: u32,
    pub nodelay: bool,
    pub ttl: u8,
    pub reuseaddr: bool,
    pub broadcast: bool,
    pub multicast_loop: bool,
    pub multicast_ttl: u8,
    pub linger_onoff: i32,
    pub linger_secs: i32,
}

impl SocketSnapshot {
    pub const fn empty() -> Self {
        Self {
            handle: 0,
            owner_pid: 0,
            kind: SocketKind::Udp,
            state: SocketState::Closed,
            local_addr: 0,
            local_port: 0,
            remote_addr: 0,
            remote_port: 0,
            pending_rx: 0,
            tx_bytes: 0,
            rx_bytes: 0,
            shutdown_rd: false,
            shutdown_wr: false,
            so_error: 0,
            backlog: 0,
            nodelay: false,
            ttl: 64,
            reuseaddr: false,
            broadcast: false,
            multicast_loop: true,
            multicast_ttl: 1,
            linger_onoff: 0,
            linger_secs: 0,
        }
    }
}

#[derive(Clone, Copy)]
struct SocketRecord {
    active: bool,
    generation: u16,
    owner_pid: u32,
    kind: SocketKind,
    state: SocketState,
    local_addr: u32,
    local_port: u16,
    remote_addr: u32,
    remote_port: u16,
    pending_rx: u32,
    tx_bytes: u64,
    rx_bytes: u64,
    shutdown_rd: bool,
    shutdown_wr: bool,
    so_error: i32,
    backlog: u32,
    nodelay: bool,
    ttl: u8,
    reuseaddr: bool,
    broadcast: bool,
    multicast_loop: bool,
    multicast_ttl: u8,
    linger_onoff: i32,
    linger_secs: i32,
    connect_started_tick: u64,
}

impl SocketRecord {
    const fn empty() -> Self {
        Self {
            active: false,
            generation: 0,
            owner_pid: 0,
            kind: SocketKind::Udp,
            state: SocketState::Closed,
            local_addr: 0,
            local_port: 0,
            remote_addr: 0,
            remote_port: 0,
            pending_rx: 0,
            tx_bytes: 0,
            rx_bytes: 0,
            shutdown_rd: false,
            shutdown_wr: false,
            so_error: 0,
            backlog: 0,
            nodelay: false,
            ttl: 64,
            reuseaddr: false,
            broadcast: false,
            multicast_loop: true,
            multicast_ttl: 1,
            linger_onoff: 0,
            linger_secs: 0,
            connect_started_tick: 0,
        }
    }
}

pub struct SocketTable {
    sockets: [SocketRecord; MAX_SOCKETS],
    next_ephemeral: u16,
}

impl SocketTable {
    pub const fn new() -> Self {
        Self {
            sockets: [SocketRecord::empty(); MAX_SOCKETS],
            next_ephemeral: 49152,
        }
    }

    pub fn open(&mut self, owner_pid: u32, kind: SocketKind) -> Result<SocketSnapshot, i64> {
        let Some(idx) = self.sockets.iter().position(|entry| !entry.active) else {
            return Err(syscall::EMFILE);
        };
        let generation = self.sockets[idx].generation.wrapping_add(1).max(1);
        self.sockets[idx] = SocketRecord {
            active: true,
            generation,
            owner_pid,
            kind,
            state: SocketState::Open,
            ..SocketRecord::empty()
        };
        Ok(self.snapshot(idx))
    }

    pub fn open_accepted(
        &mut self,
        owner_pid: u32,
        local_addr: u32,
        local_port: u16,
        remote_addr: u32,
        remote_port: u16,
    ) -> Result<SocketSnapshot, i64> {
        let Some(idx) = self.sockets.iter().position(|entry| !entry.active) else {
            return Err(syscall::EMFILE);
        };
        let generation = self.sockets[idx].generation.wrapping_add(1).max(1);
        self.sockets[idx] = SocketRecord {
            active: true,
            generation,
            owner_pid,
            kind: SocketKind::Tcp,
            state: SocketState::Connected,
            local_addr,
            local_port,
            remote_addr,
            remote_port,
            ..SocketRecord::empty()
        };
        Ok(self.snapshot(idx))
    }

    pub fn update_endpoints(
        &mut self,
        owner_pid: u32,
        handle: u32,
        local_addr: u32,
        local_port: u16,
        remote_addr: u32,
        remote_port: u16,
    ) -> Result<SocketSnapshot, i64> {
        let idx = self.lookup_owned(owner_pid, handle)?;
        self.sockets[idx].local_addr = local_addr;
        self.sockets[idx].local_port = local_port;
        self.sockets[idx].remote_addr = remote_addr;
        self.sockets[idx].remote_port = remote_port;
        Ok(self.snapshot(idx))
    }

    pub fn bind(
        &mut self,
        owner_pid: u32,
        handle: u32,
        local_addr: u32,
        local_port: u16,
    ) -> Result<SocketSnapshot, i64> {
        let idx = self.lookup_owned(owner_pid, handle)?;
        let port = if local_port == 0 {
            self.alloc_ephemeral_unused(self.sockets[idx].kind)
        } else {
            local_port
        };
        if self.port_in_use(
            idx,
            local_addr,
            port,
            self.sockets[idx].kind,
            self.sockets[idx].reuseaddr,
        ) {
            return Err(syscall::EADDRINUSE);
        }
        let socket = &mut self.sockets[idx];
        socket.local_addr = local_addr;
        socket.local_port = port;
        socket.state = SocketState::Bound;
        Ok(self.snapshot(idx))
    }

    pub fn listen(
        &mut self,
        owner_pid: u32,
        handle: u32,
        backlog: u32,
    ) -> Result<SocketSnapshot, i64> {
        let idx = self.lookup_owned(owner_pid, handle)?;
        if self.sockets[idx].kind != SocketKind::Tcp {
            return Err(syscall::EOPNOTSUPP);
        }
        if self.sockets[idx].local_port == 0 {
            return Err(syscall::EINVAL);
        }
        self.sockets[idx].state = SocketState::Listening;
        self.sockets[idx].backlog = backlog.clamp(1, 128);
        Ok(self.snapshot(idx))
    }

    pub fn connect(
        &mut self,
        owner_pid: u32,
        handle: u32,
        remote_addr: u32,
        remote_port: u16,
    ) -> Result<SocketSnapshot, i64> {
        self.connect_at_tick(owner_pid, handle, remote_addr, remote_port, 0)
    }

    pub fn connect_at_tick(
        &mut self,
        owner_pid: u32,
        handle: u32,
        remote_addr: u32,
        remote_port: u16,
        now_tick: u64,
    ) -> Result<SocketSnapshot, i64> {
        let idx = self.lookup_owned(owner_pid, handle)?;
        let raw = self.sockets[idx].kind == SocketKind::Raw;
        if remote_addr == 0 || (remote_port == 0 && !raw) {
            return Err(syscall::EINVAL);
        }
        if self.sockets[idx].kind == SocketKind::Tcp
            && matches!(
                self.sockets[idx].state,
                SocketState::Connecting | SocketState::Connected
            )
            && (self.sockets[idx].remote_addr != remote_addr
                || self.sockets[idx].remote_port != remote_port)
        {
            return Err(syscall::EINVAL);
        }
        if self.sockets[idx].local_port == 0 {
            self.sockets[idx].local_addr = if (remote_addr & 0xff00_0000) == 0x7f00_0000 {
                0x7f00_0001
            } else {
                0
            };
            self.sockets[idx].local_port = self.alloc_ephemeral_unused(self.sockets[idx].kind);
        }
        let already_connecting = self.sockets[idx].state == SocketState::Connecting;
        self.sockets[idx].remote_addr = remote_addr;
        self.sockets[idx].remote_port = remote_port;
        if !already_connecting {
            self.sockets[idx].so_error = 0;
            self.sockets[idx].connect_started_tick = now_tick;
        }
        self.sockets[idx].state = if self.sockets[idx].kind == SocketKind::Tcp {
            if self.sockets[idx].state == SocketState::Connected {
                SocketState::Connected
            } else {
                SocketState::Connecting
            }
        } else {
            SocketState::Connected
        };
        Ok(self.snapshot(idx))
    }

    pub fn complete_tcp_connect(
        &mut self,
        owner_pid: u32,
        handle: u32,
    ) -> Result<SocketSnapshot, i64> {
        let idx = self.lookup_owned(owner_pid, handle)?;
        if self.sockets[idx].kind != SocketKind::Tcp
            || !matches!(
                self.sockets[idx].state,
                SocketState::Connecting | SocketState::Connected
            )
        {
            return Err(syscall::EINVAL);
        }
        self.sockets[idx].state = SocketState::Connected;
        self.sockets[idx].so_error = 0;
        Ok(self.snapshot(idx))
    }

    pub fn complete_tcp_connect_by_handle(&mut self, handle: u32) {
        if let Some(idx) = self.lookup_any(handle) {
            if self.sockets[idx].kind == SocketKind::Tcp
                && self.sockets[idx].state == SocketState::Connecting
            {
                self.sockets[idx].state = SocketState::Connected;
                self.sockets[idx].so_error = 0;
            }
        }
    }

    pub fn fail_tcp_connect_by_handle(&mut self, handle: u32, err: i32) {
        if let Some(idx) = self.lookup_any(handle) {
            if self.sockets[idx].kind == SocketKind::Tcp
                && self.sockets[idx].state == SocketState::Connecting
            {
                self.sockets[idx].state = SocketState::Closed;
                self.sockets[idx].so_error = err;
            }
        }
    }

    pub fn connecting_sockets(&self, out: &mut [(SocketSnapshot, u64)]) -> usize {
        let mut count = 0usize;
        for idx in 0..MAX_SOCKETS {
            if count >= out.len() {
                break;
            }
            let s = self.sockets[idx];
            if s.active && s.kind == SocketKind::Tcp && s.state == SocketState::Connecting {
                out[count] = (self.snapshot(idx), s.connect_started_tick);
                count += 1;
            }
        }
        count
    }

    pub fn send_to(
        &mut self,
        owner_pid: u32,
        handle: u32,
        len: u32,
        addr: u32,
        port: u16,
    ) -> Result<SocketSnapshot, i64> {
        let idx = self.lookup_owned(owner_pid, handle)?;
        if self.sockets[idx].shutdown_wr {
            return Err(syscall::EPIPE);
        }
        if matches!(
            self.sockets[idx].state,
            SocketState::Connecting | SocketState::Shutdown | SocketState::Closed
        ) {
            return Err(if self.sockets[idx].state == SocketState::Connecting {
                syscall::EAGAIN
            } else {
                syscall::EPIPE
            });
        }
        if addr != 0 && (port != 0 || self.sockets[idx].kind == SocketKind::Raw) {
            if self.sockets[idx].kind == SocketKind::Tcp {
                const EISCONN: i64 = -106;
                return Err(EISCONN);
            }
            if self.sockets[idx].local_port == 0 {
                self.sockets[idx].local_addr = if (addr & 0xff00_0000) == 0x7f00_0000 {
                    0x7f00_0001
                } else {
                    0
                };
                self.sockets[idx].local_port = self.alloc_ephemeral_unused(self.sockets[idx].kind);
            }
            let mut snap = self.snapshot(idx);
            snap.remote_addr = addr;
            snap.remote_port = port;
            self.sockets[idx].tx_bytes = self.sockets[idx].tx_bytes.saturating_add(len as u64);
            snap.tx_bytes = self.sockets[idx].tx_bytes;
            return Ok(snap);
        }
        if self.sockets[idx].state != SocketState::Connected {
            return Err(syscall::ENOTCONN);
        }
        self.sockets[idx].tx_bytes = self.sockets[idx].tx_bytes.saturating_add(len as u64);
        Ok(self.snapshot(idx))
    }

    pub fn record_recv(
        &mut self,
        owner_pid: u32,
        handle: u32,
        delivered: u32,
    ) -> Result<SocketSnapshot, i64> {
        let idx = self.lookup_owned(owner_pid, handle)?;
        self.sockets[idx].pending_rx = self.sockets[idx].pending_rx.saturating_sub(delivered);
        self.sockets[idx].rx_bytes = self.sockets[idx].rx_bytes.saturating_add(delivered as u64);
        Ok(self.snapshot(idx))
    }

    pub fn shutdown(&mut self, owner_pid: u32, handle: u32) -> Result<SocketSnapshot, i64> {
        self.shutdown_how(owner_pid, handle, 2)
    }

    pub fn shutdown_how(
        &mut self,
        owner_pid: u32,
        handle: u32,
        how: u32,
    ) -> Result<SocketSnapshot, i64> {
        if how > 2 {
            return Err(syscall::EINVAL);
        }
        let idx = self.lookup_owned(owner_pid, handle)?;
        if how == 0 || how == 2 {
            self.sockets[idx].shutdown_rd = true;
        }
        if how == 1 || how == 2 {
            self.sockets[idx].shutdown_wr = true;
        }
        if self.sockets[idx].shutdown_rd && self.sockets[idx].shutdown_wr {
            self.sockets[idx].state = SocketState::Shutdown;
        }
        Ok(self.snapshot(idx))
    }

    pub fn set_option(
        &mut self,
        owner_pid: u32,
        handle: u32,
        level: i32,
        optname: i32,
        val: &[u8],
    ) -> Result<SocketSnapshot, i64> {
        const SOL_SOCKET: i32 = 1;
        const IPPROTO_IP: i32 = 0;
        const IPPROTO_TCP: i32 = 6;
        const SO_REUSEADDR: i32 = 2;
        const SO_BROADCAST: i32 = 6;
        const SO_SNDBUF: i32 = 7;
        const SO_RCVBUF: i32 = 8;
        const SO_LINGER: i32 = 13;
        const SO_RCVTIMEO: i32 = 20;
        const SO_SNDTIMEO: i32 = 21;
        const TCP_NODELAY: i32 = 1;
        const IP_TTL: i32 = 2;
        const IP_MULTICAST_TTL: i32 = 33;
        const IP_MULTICAST_LOOP: i32 = 34;
        const ENOPROTOOPT: i64 = -92;

        let idx = self.lookup_owned(owner_pid, handle)?;
        let read_i32 = || -> Result<i32, i64> {
            if val.len() < 4 {
                return Err(syscall::EINVAL);
            }
            Ok(i32::from_ne_bytes([val[0], val[1], val[2], val[3]]))
        };

        match (level, optname) {
            (SOL_SOCKET, SO_REUSEADDR) => {
                self.sockets[idx].reuseaddr = read_i32()? != 0;
            }
            (SOL_SOCKET, SO_BROADCAST) => {
                self.sockets[idx].broadcast = read_i32()? != 0;
            }
            (SOL_SOCKET, SO_SNDBUF) | (SOL_SOCKET, SO_RCVBUF) => {
                let _ = read_i32()?;
            }
            (SOL_SOCKET, SO_LINGER) => {
                if val.len() < 8 {
                    return Err(syscall::EINVAL);
                }
                let onoff = i32::from_ne_bytes([val[0], val[1], val[2], val[3]]);
                let secs = i32::from_ne_bytes([val[4], val[5], val[6], val[7]]);
                self.sockets[idx].linger_onoff = onoff;
                self.sockets[idx].linger_secs = secs;
            }
            (SOL_SOCKET, SO_RCVTIMEO) | (SOL_SOCKET, SO_SNDTIMEO) => {
                // Timeouts are enforced by net_bridge; accept cleanly here as well.
            }
            (IPPROTO_TCP, TCP_NODELAY) => {
                if self.sockets[idx].kind != SocketKind::Tcp {
                    return Err(ENOPROTOOPT);
                }
                self.sockets[idx].nodelay = read_i32()? != 0;
            }
            (IPPROTO_IP, IP_TTL) => {
                let v = read_i32()?;
                if !(1..=255).contains(&v) {
                    return Err(syscall::EINVAL);
                }
                self.sockets[idx].ttl = v as u8;
            }
            (IPPROTO_IP, IP_MULTICAST_TTL) => {
                let v = read_i32()?;
                if !(0..=255).contains(&v) {
                    return Err(syscall::EINVAL);
                }
                self.sockets[idx].multicast_ttl = v as u8;
            }
            (IPPROTO_IP, IP_MULTICAST_LOOP) => {
                self.sockets[idx].multicast_loop = read_i32()? != 0;
            }
            _ => return Err(ENOPROTOOPT),
        }
        Ok(self.snapshot(idx))
    }

    pub fn get_option(
        &mut self,
        owner_pid: u32,
        handle: u32,
        level: i32,
        optname: i32,
    ) -> Result<(SocketSnapshot, [u8; 8], usize), i64> {
        const SOL_SOCKET: i32 = 1;
        const IPPROTO_IP: i32 = 0;
        const IPPROTO_TCP: i32 = 6;
        const SO_REUSEADDR: i32 = 2;
        const SO_TYPE: i32 = 3;
        const SO_ERROR: i32 = 4;
        const SO_BROADCAST: i32 = 6;
        const SO_SNDBUF: i32 = 7;
        const SO_RCVBUF: i32 = 8;
        const SO_LINGER: i32 = 13;
        const TCP_NODELAY: i32 = 1;
        const IP_TTL: i32 = 2;
        const IP_MULTICAST_TTL: i32 = 33;
        const IP_MULTICAST_LOOP: i32 = 34;
        const ENOPROTOOPT: i64 = -92;

        let idx = self.lookup_owned(owner_pid, handle)?;
        let mut out = [0u8; 8];
        let len = match (level, optname) {
            (SOL_SOCKET, SO_ERROR) => {
                let err = self.sockets[idx].so_error;
                self.sockets[idx].so_error = 0;
                out[0..4].copy_from_slice(&err.to_ne_bytes());
                4
            }
            (SOL_SOCKET, SO_TYPE) => {
                let ty: i32 = match self.sockets[idx].kind {
                    SocketKind::Tcp => 1,
                    SocketKind::Udp => 2,
                    SocketKind::Raw => 3,
                };
                out[0..4].copy_from_slice(&ty.to_ne_bytes());
                4
            }
            (SOL_SOCKET, SO_REUSEADDR) => {
                let v = i32::from(self.sockets[idx].reuseaddr);
                out[0..4].copy_from_slice(&v.to_ne_bytes());
                4
            }
            (SOL_SOCKET, SO_BROADCAST) => {
                let v = i32::from(self.sockets[idx].broadcast);
                out[0..4].copy_from_slice(&v.to_ne_bytes());
                4
            }
            (SOL_SOCKET, SO_SNDBUF) | (SOL_SOCKET, SO_RCVBUF) => {
                let v: i32 = 16384;
                out[0..4].copy_from_slice(&v.to_ne_bytes());
                4
            }
            (SOL_SOCKET, SO_LINGER) => {
                out[0..4].copy_from_slice(&self.sockets[idx].linger_onoff.to_ne_bytes());
                out[4..8].copy_from_slice(&self.sockets[idx].linger_secs.to_ne_bytes());
                8
            }
            (IPPROTO_TCP, TCP_NODELAY) => {
                if self.sockets[idx].kind != SocketKind::Tcp {
                    return Err(ENOPROTOOPT);
                }
                let v = i32::from(self.sockets[idx].nodelay);
                out[0..4].copy_from_slice(&v.to_ne_bytes());
                4
            }
            (IPPROTO_IP, IP_TTL) => {
                let v = self.sockets[idx].ttl as i32;
                out[0..4].copy_from_slice(&v.to_ne_bytes());
                4
            }
            (IPPROTO_IP, IP_MULTICAST_TTL) => {
                let v = self.sockets[idx].multicast_ttl as i32;
                out[0..4].copy_from_slice(&v.to_ne_bytes());
                4
            }
            (IPPROTO_IP, IP_MULTICAST_LOOP) => {
                let v = i32::from(self.sockets[idx].multicast_loop);
                out[0..4].copy_from_slice(&v.to_ne_bytes());
                4
            }
            _ => return Err(ENOPROTOOPT),
        };
        Ok((self.snapshot(idx), out, len))
    }

    pub fn close(&mut self, owner_pid: u32, handle: u32) -> Result<SocketSnapshot, i64> {
        let idx = self.lookup_owned(owner_pid, handle)?;
        let snapshot = self.snapshot(idx);
        let generation = self.sockets[idx].generation;
        self.sockets[idx] = SocketRecord {
            generation,
            ..SocketRecord::empty()
        };
        Ok(snapshot)
    }

    pub fn snapshot_owned(&self, owner_pid: u32, handle: u32) -> Result<SocketSnapshot, i64> {
        let idx = self.lookup_owned(owner_pid, handle)?;
        Ok(self.snapshot(idx))
    }

    pub fn accept(&mut self, owner_pid: u32, handle: u32) -> Result<SocketSnapshot, i64> {
        let idx = self.lookup_owned(owner_pid, handle)?;
        if self.sockets[idx].state != SocketState::Listening {
            return Err(syscall::EINVAL);
        }
        Err(syscall::EAGAIN)
    }

    fn snapshot(&self, idx: usize) -> SocketSnapshot {
        let s = self.sockets[idx];
        SocketSnapshot {
            handle: handle_for(idx, s.generation),
            owner_pid: s.owner_pid,
            kind: s.kind,
            state: s.state,
            local_addr: s.local_addr,
            local_port: s.local_port,
            remote_addr: s.remote_addr,
            remote_port: s.remote_port,
            pending_rx: s.pending_rx,
            tx_bytes: s.tx_bytes,
            rx_bytes: s.rx_bytes,
            shutdown_rd: s.shutdown_rd,
            shutdown_wr: s.shutdown_wr,
            so_error: s.so_error,
            backlog: s.backlog,
            nodelay: s.nodelay,
            ttl: s.ttl,
            reuseaddr: s.reuseaddr,
            broadcast: s.broadcast,
            multicast_loop: s.multicast_loop,
            multicast_ttl: s.multicast_ttl,
            linger_onoff: s.linger_onoff,
            linger_secs: s.linger_secs,
        }
    }

    fn lookup_any(&self, handle: u32) -> Option<usize> {
        if handle & HANDLE_TAG_MASK != HANDLE_TAG {
            return None;
        }
        let idx = (handle & HANDLE_INDEX_MASK) as usize;
        if idx >= MAX_SOCKETS {
            return None;
        }
        let generation = ((handle >> 16) & HANDLE_GENERATION_MASK) as u16;
        let s = self.sockets[idx];
        if !s.active || s.generation != generation {
            return None;
        }
        Some(idx)
    }

    fn lookup_owned(&self, owner_pid: u32, handle: u32) -> Result<usize, i64> {
        if handle & HANDLE_TAG_MASK != HANDLE_TAG {
            return Err(syscall::EBADF);
        }
        let idx = (handle & HANDLE_INDEX_MASK) as usize;
        if idx >= MAX_SOCKETS {
            return Err(syscall::EBADF);
        }
        let generation = ((handle >> 16) & HANDLE_GENERATION_MASK) as u16;
        let s = self.sockets[idx];
        if !s.active || s.generation != generation {
            return Err(syscall::EBADF);
        }
        if s.owner_pid != owner_pid {
            return Err(syscall::EPERM);
        }
        Ok(idx)
    }

    fn port_in_use(
        &self,
        skip_idx: usize,
        local_addr: u32,
        port: u16,
        kind: SocketKind,
        reuseaddr: bool,
    ) -> bool {
        self.sockets.iter().enumerate().any(|(idx, entry)| {
            if idx == skip_idx || !entry.active || entry.kind != kind || entry.local_port != port {
                return false;
            }
            if matches!(entry.state, SocketState::Closed) {
                return false;
            }
            // Different specific local IPs do not conflict.
            if local_addr != 0 && entry.local_addr != 0 && local_addr != entry.local_addr {
                return false;
            }
            if reuseaddr
                && (entry.state == SocketState::Shutdown
                    || (entry.reuseaddr && entry.state != SocketState::Listening))
            {
                return false;
            }
            true
        })
    }

    fn alloc_ephemeral_unused(&mut self, kind: SocketKind) -> u16 {
        let mut attempts = 0usize;
        while attempts < 16384 {
            let port = self.next_ephemeral;
            self.next_ephemeral = if self.next_ephemeral == 65535 {
                49152
            } else {
                self.next_ephemeral + 1
            };
            if !self.port_in_use(usize::MAX, 0, port, kind, false) {
                return port;
            }
            attempts += 1;
        }
        self.next_ephemeral
    }
}

const fn handle_for(idx: usize, generation: u16) -> u32 {
    HANDLE_TAG | (((generation as u32) & HANDLE_GENERATION_MASK) << 16) | (idx as u32)
}
