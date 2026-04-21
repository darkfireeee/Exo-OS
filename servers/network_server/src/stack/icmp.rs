use exo_syscall_abi as syscall;

const MAX_ECHOES: usize = 32;

#[derive(Clone, Copy)]
struct EchoRecord {
    active: bool,
    token: u64,
    target: u32,
    payload_len: u16,
    last_latency_ms: u32,
}

impl EchoRecord {
    const fn empty() -> Self {
        Self {
            active: false,
            token: 0,
            target: 0,
            payload_len: 0,
            last_latency_ms: 0,
        }
    }
}

#[derive(Clone, Copy)]
pub struct EchoSnapshot {
    pub token: u64,
    pub target: u32,
    pub payload_len: u16,
    pub sent_count: u32,
    pub completed_count: u32,
    pub last_latency_ms: u32,
}

pub struct IcmpTracker {
    echoes: [EchoRecord; MAX_ECHOES],
    next_token: u64,
    sent_count: u32,
    completed_count: u32,
    last_latency_ms: u32,
}

impl IcmpTracker {
    pub const fn new() -> Self {
        Self {
            echoes: [EchoRecord::empty(); MAX_ECHOES],
            next_token: 1,
            sent_count: 0,
            completed_count: 0,
            last_latency_ms: 0,
        }
    }

    pub fn issue_echo(&mut self, target: u32, payload_len: u16) -> Result<EchoSnapshot, i64> {
        let Some(idx) = self.echoes.iter().position(|entry| !entry.active) else {
            return Err(syscall::ENOBUFS);
        };
        let token = self.next_token;
        self.next_token = self.next_token.saturating_add(1);
        self.sent_count = self.sent_count.saturating_add(1);
        self.echoes[idx] = EchoRecord {
            active: true,
            token,
            target,
            payload_len,
            last_latency_ms: 0,
        };
        Ok(self.snapshot(token).unwrap_or(EchoSnapshot {
            token,
            target,
            payload_len,
            sent_count: self.sent_count,
            completed_count: self.completed_count,
            last_latency_ms: self.last_latency_ms,
        }))
    }

    pub fn complete(&mut self, token: u64, latency_ms: u32) -> bool {
        let Some(idx) = self.echoes.iter().position(|entry| entry.active && entry.token == token) else {
            return false;
        };
        self.echoes[idx].last_latency_ms = latency_ms;
        self.last_latency_ms = latency_ms;
        self.completed_count = self.completed_count.saturating_add(1);
        true
    }

    pub fn snapshot(&self, token: u64) -> Option<EchoSnapshot> {
        let entry = self.echoes.iter().find(|record| record.active && record.token == token)?;
        Some(EchoSnapshot {
            token: entry.token,
            target: entry.target,
            payload_len: entry.payload_len,
            sent_count: self.sent_count,
            completed_count: self.completed_count,
            last_latency_ms: entry.last_latency_ms,
        })
    }
}
