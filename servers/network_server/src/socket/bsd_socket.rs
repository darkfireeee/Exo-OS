use exo_syscall_abi as syscall;

pub const MAX_SOCKETS: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SocketKind {
    Tcp,
    Udp,
    Raw,
}

impl SocketKind {
    pub const fn from_u32(raw: u32) -> Option<Self> {
        match raw {
            1 => Some(Self::Tcp),
            2 => Some(Self::Udp),
            3 => Some(Self::Raw),
            _ => None,
        }
    }

    pub const fn as_u32(self) -> u32 {
        match self {
            Self::Tcp => 1,
            Self::Udp => 2,
            Self::Raw => 3,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SocketState {
    Closed,
    Open,
    Bound,
    Connected,
}

impl SocketState {
    pub const fn as_u32(self) -> u32 {
        match self {
            Self::Closed => 0,
            Self::Open => 1,
            Self::Bound => 2,
            Self::Connected => 3,
        }
    }
}

#[derive(Clone, Copy)]
pub struct SocketSnapshot {
    pub handle: u64,
    pub owner_pid: u32,
    pub kind: SocketKind,
    pub state: SocketState,
    pub local_addr: u32,
    pub local_port: u16,
    pub remote_addr: u32,
    pub remote_port: u16,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub pending_rx: u32,
    pub pending_tx: u32,
    pub flags: u32,
}

#[derive(Clone, Copy)]
struct SocketRecord {
    active: bool,
    handle: u64,
    owner_pid: u32,
    kind: SocketKind,
    state: SocketState,
    local_addr: u32,
    local_port: u16,
    remote_addr: u32,
    remote_port: u16,
    tx_bytes: u64,
    rx_bytes: u64,
    pending_rx: u32,
    pending_tx: u32,
    flags: u32,
}

impl SocketRecord {
    const fn empty() -> Self {
        Self {
            active: false,
            handle: 0,
            owner_pid: 0,
            kind: SocketKind::Udp,
            state: SocketState::Closed,
            local_addr: 0,
            local_port: 0,
            remote_addr: 0,
            remote_port: 0,
            tx_bytes: 0,
            rx_bytes: 0,
            pending_rx: 0,
            pending_tx: 0,
            flags: 0,
        }
    }
}

pub struct SocketTable {
    sockets: [SocketRecord; MAX_SOCKETS],
    next_handle: u64,
}

impl SocketTable {
    pub const fn new() -> Self {
        Self {
            sockets: [SocketRecord::empty(); MAX_SOCKETS],
            next_handle: 1,
        }
    }

    pub fn open(
        &mut self,
        owner_pid: u32,
        kind: SocketKind,
        flags: u32,
    ) -> Result<SocketSnapshot, i64> {
        let Some(idx) = self.sockets.iter().position(|entry| !entry.active) else {
            return Err(syscall::EMFILE);
        };

        let handle = self.next_handle;
        self.next_handle = self.next_handle.saturating_add(1);
        self.sockets[idx] = SocketRecord {
            active: true,
            handle,
            owner_pid,
            kind,
            state: SocketState::Open,
            local_addr: 0,
            local_port: 0,
            remote_addr: 0,
            remote_port: 0,
            tx_bytes: 0,
            rx_bytes: 0,
            pending_rx: 0,
            pending_tx: 0,
            flags,
        };
        Ok(self.snapshot(idx))
    }

    pub fn bind(
        &mut self,
        owner_pid: u32,
        handle: u64,
        local_addr: u32,
        local_port: u16,
    ) -> Result<SocketSnapshot, i64> {
        if local_port == 0 {
            return Err(syscall::EINVAL);
        }

        if self.sockets.iter().any(|entry| {
            entry.active
                && entry.owner_pid == owner_pid
                && entry.handle != handle
                && entry.local_port == local_port
                && entry.kind == self.kind_of(handle).unwrap_or(SocketKind::Udp)
        }) {
            return Err(syscall::EADDRINUSE);
        }

        let idx = self.lookup_owned(owner_pid, handle)?;
        let socket = &mut self.sockets[idx];
        socket.local_addr = local_addr;
        socket.local_port = local_port;
        socket.state = SocketState::Bound;
        Ok(self.snapshot(idx))
    }

    pub fn connect(
        &mut self,
        owner_pid: u32,
        handle: u64,
        remote_addr: u32,
        remote_port: u16,
    ) -> Result<SocketSnapshot, i64> {
        if remote_addr == 0 || remote_port == 0 {
            return Err(syscall::EINVAL);
        }

        let idx = self.lookup_owned(owner_pid, handle)?;
        let socket = &mut self.sockets[idx];
        socket.remote_addr = remote_addr;
        socket.remote_port = remote_port;
        if socket.local_port == 0 {
            socket.state = SocketState::Open;
        }
        socket.state = SocketState::Connected;
        Ok(self.snapshot(idx))
    }

    pub fn assign_ephemeral_port(
        &mut self,
        owner_pid: u32,
        handle: u64,
        local_addr: u32,
        local_port: u16,
    ) -> Result<SocketSnapshot, i64> {
        let idx = self.lookup_owned(owner_pid, handle)?;
        let socket = &mut self.sockets[idx];
        if socket.local_port == 0 {
            socket.local_addr = local_addr;
            socket.local_port = local_port;
            if socket.state == SocketState::Open {
                socket.state = SocketState::Bound;
            }
        }
        Ok(self.snapshot(idx))
    }

    pub fn note_send(
        &mut self,
        owner_pid: u32,
        handle: u64,
        len: u32,
    ) -> Result<SocketSnapshot, i64> {
        let idx = self.lookup_owned(owner_pid, handle)?;
        let socket = &mut self.sockets[idx];
        if socket.state != SocketState::Connected {
            return Err(syscall::ENOTCONN);
        }
        socket.tx_bytes = socket.tx_bytes.saturating_add(len as u64);
        socket.pending_tx = socket.pending_tx.saturating_add(1);
        Ok(self.snapshot(idx))
    }

    pub fn inject_rx(&mut self, handle: u64, len: u32) -> Result<(), i64> {
        let idx = self.lookup(handle)?;
        let socket = &mut self.sockets[idx];
        socket.pending_rx = socket.pending_rx.saturating_add(len);
        Ok(())
    }

    pub fn take_rx(
        &mut self,
        owner_pid: u32,
        handle: u64,
        budget: u32,
    ) -> Result<SocketSnapshot, i64> {
        let idx = self.lookup_owned(owner_pid, handle)?;
        let socket = &mut self.sockets[idx];
        if socket.pending_rx == 0 {
            return Err(syscall::EAGAIN);
        }
        let delivered = socket.pending_rx.min(budget.max(1));
        socket.pending_rx -= delivered;
        socket.rx_bytes = socket.rx_bytes.saturating_add(delivered as u64);
        if socket.pending_tx > 0 {
            socket.pending_tx -= 1;
        }
        Ok(self.snapshot(idx))
    }

    pub fn queue_recv(&mut self, owner_pid: u32, handle: u64) -> Result<(), i64> {
        let idx = self.lookup_owned(owner_pid, handle)?;
        self.sockets[idx].pending_rx = self.sockets[idx].pending_rx.saturating_add(0);
        Ok(())
    }

    pub fn close(&mut self, owner_pid: u32, handle: u64) -> Result<SocketSnapshot, i64> {
        let idx = self.lookup_owned(owner_pid, handle)?;
        let snapshot = self.snapshot(idx);
        self.sockets[idx] = SocketRecord::empty();
        Ok(snapshot)
    }

    pub fn snapshot_owned(&self, owner_pid: u32, handle: u64) -> Result<SocketSnapshot, i64> {
        let idx = self.lookup_owned(owner_pid, handle)?;
        Ok(self.snapshot(idx))
    }

    pub fn snapshot(&self, idx: usize) -> SocketSnapshot {
        let socket = self.sockets[idx];
        SocketSnapshot {
            handle: socket.handle,
            owner_pid: socket.owner_pid,
            kind: socket.kind,
            state: socket.state,
            local_addr: socket.local_addr,
            local_port: socket.local_port,
            remote_addr: socket.remote_addr,
            remote_port: socket.remote_port,
            tx_bytes: socket.tx_bytes,
            rx_bytes: socket.rx_bytes,
            pending_rx: socket.pending_rx,
            pending_tx: socket.pending_tx,
            flags: socket.flags,
        }
    }

    pub fn count_by_owner(&self, owner_pid: u32) -> u32 {
        self.sockets
            .iter()
            .filter(|entry| entry.active && entry.owner_pid == owner_pid)
            .count() as u32
    }

    pub fn active_count(&self) -> u32 {
        self.sockets.iter().filter(|entry| entry.active).count() as u32
    }

    pub fn kind_of(&self, handle: u64) -> Option<SocketKind> {
        let idx = self.lookup(handle).ok()?;
        Some(self.sockets[idx].kind)
    }

    fn lookup(&self, handle: u64) -> Result<usize, i64> {
        self.sockets
            .iter()
            .position(|entry| entry.active && entry.handle == handle)
            .ok_or(syscall::ENOENT)
    }

    fn lookup_owned(&self, owner_pid: u32, handle: u64) -> Result<usize, i64> {
        let idx = self.lookup(handle)?;
        if self.sockets[idx].owner_pid != owner_pid {
            return Err(syscall::EPERM);
        }
        Ok(idx)
    }
}

impl SocketSnapshot {
    pub const fn nice_queue_hint(&self) -> u16 {
        self.pending_rx.saturating_add(self.pending_tx) as u16
    }
}
