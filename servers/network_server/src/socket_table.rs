use exo_syscall_abi as syscall;

pub const MAX_SOCKETS: usize = 64;
const HANDLE_BASE: u32 = 0x4E00_0000;
const AF_INET: u32 = 2;
const SOCK_STREAM: u32 = 1;
const SOCK_DGRAM: u32 = 2;
const SOCK_RAW: u32 = 3;
const SOCK_TYPE_MASK: u32 = 0x0f;
const LOOPBACK: u32 = 0x7f00_0001;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SocketKind {
    Tcp,
    Udp,
    Raw,
}

impl SocketKind {
    pub fn from_domain_type(domain: u32, ty: u32, _protocol: u32) -> Result<Self, i64> {
        if domain != AF_INET {
            return Err(syscall::EAFNOSUPPORT);
        }
        match ty & SOCK_TYPE_MASK {
            SOCK_STREAM => Ok(Self::Tcp),
            SOCK_DGRAM => Ok(Self::Udp),
            SOCK_RAW => Ok(Self::Raw),
            _ => Err(syscall::EINVAL),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SocketState {
    Closed,
    Open,
    Bound,
    Listening,
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
            local_addr: 0,
            local_port: 0,
            remote_addr: 0,
            remote_port: 0,
            pending_rx: 0,
            tx_bytes: 0,
            rx_bytes: 0,
        };
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
            self.alloc_ephemeral()
        } else {
            local_port
        };
        if self.port_in_use(owner_pid, idx, port, self.sockets[idx].kind) {
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
        _backlog: u32,
    ) -> Result<SocketSnapshot, i64> {
        let idx = self.lookup_owned(owner_pid, handle)?;
        if self.sockets[idx].kind != SocketKind::Tcp {
            return Err(syscall::ENOTCONN);
        }
        if self.sockets[idx].local_port == 0 {
            return Err(syscall::EINVAL);
        }
        self.sockets[idx].state = SocketState::Listening;
        Ok(self.snapshot(idx))
    }

    pub fn connect(
        &mut self,
        owner_pid: u32,
        handle: u32,
        remote_addr: u32,
        remote_port: u16,
    ) -> Result<SocketSnapshot, i64> {
        if remote_addr == 0 || remote_port == 0 {
            return Err(syscall::EINVAL);
        }
        let idx = self.lookup_owned(owner_pid, handle)?;
        if self.sockets[idx].local_port == 0 {
            self.sockets[idx].local_addr = LOOPBACK;
            self.sockets[idx].local_port = self.alloc_ephemeral();
        }
        self.sockets[idx].remote_addr = remote_addr;
        self.sockets[idx].remote_port = remote_port;
        self.sockets[idx].state = SocketState::Connected;
        Ok(self.snapshot(idx))
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
        if matches!(
            self.sockets[idx].state,
            SocketState::Shutdown | SocketState::Closed
        ) {
            return Err(syscall::ENOTCONN);
        }
        if addr != 0 && port != 0 {
            self.sockets[idx].remote_addr = addr;
            self.sockets[idx].remote_port = port;
            if self.sockets[idx].local_port == 0 {
                self.sockets[idx].local_addr = LOOPBACK;
                self.sockets[idx].local_port = self.alloc_ephemeral();
            }
            self.sockets[idx].state = SocketState::Connected;
        }
        if self.sockets[idx].state != SocketState::Connected {
            return Err(syscall::ENOTCONN);
        }
        self.sockets[idx].tx_bytes = self.sockets[idx].tx_bytes.saturating_add(len as u64);
        self.sockets[idx].pending_rx = self.sockets[idx].pending_rx.saturating_add(len);
        Ok(self.snapshot(idx))
    }

    pub fn recv_from(
        &mut self,
        owner_pid: u32,
        handle: u32,
        budget: u32,
    ) -> Result<SocketSnapshot, i64> {
        let idx = self.lookup_owned(owner_pid, handle)?;
        if self.sockets[idx].pending_rx == 0 {
            return Err(syscall::EAGAIN);
        }
        let delivered = self.sockets[idx].pending_rx.min(budget.max(1));
        self.sockets[idx].pending_rx -= delivered;
        self.sockets[idx].rx_bytes = self.sockets[idx].rx_bytes.saturating_add(delivered as u64);
        Ok(self.snapshot(idx))
    }

    pub fn shutdown(&mut self, owner_pid: u32, handle: u32) -> Result<SocketSnapshot, i64> {
        let idx = self.lookup_owned(owner_pid, handle)?;
        self.sockets[idx].state = SocketState::Shutdown;
        Ok(self.snapshot(idx))
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
        }
    }

    fn lookup_owned(&self, owner_pid: u32, handle: u32) -> Result<usize, i64> {
        let idx = (handle & 0xffff) as usize;
        if idx >= MAX_SOCKETS {
            return Err(syscall::EBADF);
        }
        let generation = ((handle >> 16) & 0x0fff) as u16;
        let s = self.sockets[idx];
        if !s.active || s.generation != generation {
            return Err(syscall::EBADF);
        }
        if s.owner_pid != owner_pid {
            return Err(syscall::EPERM);
        }
        Ok(idx)
    }

    fn port_in_use(&self, owner_pid: u32, skip_idx: usize, port: u16, kind: SocketKind) -> bool {
        self.sockets.iter().enumerate().any(|(idx, entry)| {
            idx != skip_idx
                && entry.active
                && entry.owner_pid == owner_pid
                && entry.local_port == port
                && entry.kind == kind
        })
    }

    fn alloc_ephemeral(&mut self) -> u16 {
        let port = self.next_ephemeral;
        self.next_ephemeral = if self.next_ephemeral == 65535 {
            49152
        } else {
            self.next_ephemeral + 1
        };
        port
    }
}

const fn handle_for(idx: usize, generation: u16) -> u32 {
    HANDLE_BASE | (((generation as u32) & 0x0fff) << 16) | (idx as u32)
}
