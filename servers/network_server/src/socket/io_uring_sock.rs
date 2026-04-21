use exo_syscall_abi as syscall;

const MAX_INFLIGHT: usize = 128;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OperationKind {
    Send,
    Recv,
    Echo,
}

impl OperationKind {
    pub const fn as_u32(self) -> u32 {
        match self {
            Self::Send => 1,
            Self::Recv => 2,
            Self::Echo => 3,
        }
    }
}

#[derive(Clone, Copy)]
struct Submission {
    active: bool,
    handle: u64,
    op: OperationKind,
    len: u32,
    cookie: u64,
}

impl Submission {
    const fn empty() -> Self {
        Self {
            active: false,
            handle: 0,
            op: OperationKind::Send,
            len: 0,
            cookie: 0,
        }
    }
}

#[derive(Clone, Copy)]
pub struct Completion {
    pub op: OperationKind,
    pub len: u32,
    pub cookie: u64,
}

pub struct InflightQueue {
    slots: [Submission; MAX_INFLIGHT],
    next_cookie: u64,
}

impl InflightQueue {
    pub const fn new() -> Self {
        Self {
            slots: [Submission::empty(); MAX_INFLIGHT],
            next_cookie: 1,
        }
    }

    pub fn submit_send(&mut self, handle: u64, len: u32) -> Result<u64, i64> {
        self.submit(handle, OperationKind::Send, len)
    }

    pub fn submit_recv(&mut self, handle: u64, len: u32) -> Result<u64, i64> {
        self.submit(handle, OperationKind::Recv, len)
    }

    pub fn submit_echo(&mut self, handle: u64, len: u32) -> Result<u64, i64> {
        self.submit(handle, OperationKind::Echo, len)
    }

    pub fn complete_next_for(&mut self, handle: u64) -> Option<Completion> {
        let idx = self
            .slots
            .iter()
            .position(|slot| slot.active && slot.handle == handle)?;
        let slot = self.slots[idx];
        self.slots[idx] = Submission::empty();
        Some(Completion {
            op: slot.op,
            len: slot.len,
            cookie: slot.cookie,
        })
    }

    pub fn depth(&self) -> u32 {
        self.slots.iter().filter(|slot| slot.active).count() as u32
    }
    pub fn cancel_handle(&mut self, handle: u64) -> u32 {
        let mut removed = 0u32;
        let mut idx = 0usize;
        while idx < self.slots.len() {
            if self.slots[idx].active && self.slots[idx].handle == handle {
                self.slots[idx] = Submission::empty();
                removed = removed.saturating_add(1);
            }
            idx += 1;
        }
        removed
    }

    fn submit(&mut self, handle: u64, op: OperationKind, len: u32) -> Result<u64, i64> {
        let Some(idx) = self.slots.iter().position(|slot| !slot.active) else {
            return Err(syscall::ENOBUFS);
        };
        let cookie = self.next_cookie;
        self.next_cookie = self.next_cookie.saturating_add(1);
        self.slots[idx] = Submission {
            active: true,
            handle,
            op,
            len,
            cookie,
        };
        Ok(cookie)
    }
}
