const MAX_TCP_FLOWS: usize = 64;

#[derive(Clone, Copy)]
struct FlowRecord {
    active: bool,
    handle: u64,
    cwnd_bytes: u32,
    rtt_ms: u32,
    sent_bytes: u64,
    recv_bytes: u64,
}

impl FlowRecord {
    const fn empty() -> Self {
        Self {
            active: false,
            handle: 0,
            cwnd_bytes: 16 * 1024,
            rtt_ms: 3,
            sent_bytes: 0,
            recv_bytes: 0,
        }
    }
}

#[derive(Clone, Copy)]
pub struct TcpSnapshot {
    pub handle: u64,
    pub cwnd_bytes: u32,
    pub rtt_ms: u32,
    pub sent_bytes: u64,
    pub recv_bytes: u64,
}

pub struct TcpControlPlane {
    flows: [FlowRecord; MAX_TCP_FLOWS],
}

impl TcpControlPlane {
    pub const fn new() -> Self {
        Self {
            flows: [FlowRecord::empty(); MAX_TCP_FLOWS],
        }
    }

    pub fn activate(&mut self, handle: u64) {
        if let Some(idx) = self
            .flows
            .iter()
            .position(|entry| entry.active && entry.handle == handle)
        {
            self.flows[idx].rtt_ms = 3;
            return;
        }

        if let Some(idx) = self.flows.iter().position(|entry| !entry.active) {
            self.flows[idx] = FlowRecord {
                active: true,
                handle,
                cwnd_bytes: 16 * 1024,
                rtt_ms: 3,
                sent_bytes: 0,
                recv_bytes: 0,
            };
        }
    }

    pub fn note_send(&mut self, handle: u64, len: u32) {
        if let Some(idx) = self
            .flows
            .iter()
            .position(|entry| entry.active && entry.handle == handle)
        {
            let flow = &mut self.flows[idx];
            flow.sent_bytes = flow.sent_bytes.saturating_add(len as u64);
            flow.cwnd_bytes = flow.cwnd_bytes.saturating_add((len / 4).max(256));
        }
    }

    pub fn note_recv(&mut self, handle: u64, len: u32) {
        if let Some(idx) = self
            .flows
            .iter()
            .position(|entry| entry.active && entry.handle == handle)
        {
            let flow = &mut self.flows[idx];
            flow.recv_bytes = flow.recv_bytes.saturating_add(len as u64);
            flow.rtt_ms = flow.rtt_ms.saturating_add(1).min(250);
        }
    }

    pub fn close(&mut self, handle: u64) {
        if let Some(idx) = self
            .flows
            .iter()
            .position(|entry| entry.active && entry.handle == handle)
        {
            self.flows[idx] = FlowRecord::empty();
        }
    }

    pub fn snapshot(&self, handle: u64) -> Option<TcpSnapshot> {
        let flow = self
            .flows
            .iter()
            .find(|entry| entry.active && entry.handle == handle)?;
        Some(TcpSnapshot {
            handle: flow.handle,
            cwnd_bytes: flow.cwnd_bytes,
            rtt_ms: flow.rtt_ms,
            sent_bytes: flow.sent_bytes,
            recv_bytes: flow.recv_bytes,
        })
    }
}
