const MAX_POWER_POLICIES: usize = 64;

#[repr(u32)]
#[derive(Clone, Copy)]
pub enum PowerState {
    Active = 0,
    Suspended = 1,
    Quiesced = 2,
    RestartPending = 3,
}

impl PowerState {
    pub fn from_u32(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Active),
            1 => Some(Self::Suspended),
            2 => Some(Self::Quiesced),
            3 => Some(Self::RestartPending),
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
struct PowerPolicy {
    active: bool,
    pid: u32,
    state: PowerState,
    restart_backoff_ms: u64,
    last_signal: u32,
}

impl PowerPolicy {
    const fn empty() -> Self {
        Self {
            active: false,
            pid: 0,
            state: PowerState::Active,
            restart_backoff_ms: 0,
            last_signal: 0,
        }
    }
}

#[derive(Clone, Copy)]
pub struct PowerSnapshot {
    pub state: PowerState,
    pub restart_backoff_ms: u64,
    pub last_signal: u32,
}

pub struct PowerPolicyTable {
    entries: [PowerPolicy; MAX_POWER_POLICIES],
}

impl PowerPolicyTable {
    pub const fn new() -> Self {
        Self {
            entries: [PowerPolicy::empty(); MAX_POWER_POLICIES],
        }
    }

    fn slot_mut(&mut self, pid: u32) -> &mut PowerPolicy {
        if let Some(idx) = self
            .entries
            .iter()
            .position(|entry| entry.active && entry.pid == pid)
        {
            return &mut self.entries[idx];
        }

        let idx = self
            .entries
            .iter()
            .position(|entry| !entry.active)
            .unwrap_or(0);
        self.entries[idx] = PowerPolicy {
            active: true,
            pid,
            state: PowerState::Active,
            restart_backoff_ms: 0,
            last_signal: 0,
        };
        &mut self.entries[idx]
    }

    pub fn set_state(&mut self, pid: u32, state: PowerState) -> PowerSnapshot {
        let slot = self.slot_mut(pid);
        slot.state = state;
        if matches!(state, PowerState::RestartPending) {
            slot.restart_backoff_ms = slot.restart_backoff_ms.saturating_mul(2).max(100);
        }
        PowerSnapshot {
            state: slot.state,
            restart_backoff_ms: slot.restart_backoff_ms,
            last_signal: slot.last_signal,
        }
    }

    pub fn note_release(&mut self, pid: u32, signal: u32) {
        let slot = self.slot_mut(pid);
        slot.last_signal = signal;
        slot.state = PowerState::RestartPending;
        slot.restart_backoff_ms = slot.restart_backoff_ms.saturating_mul(2).max(100);
    }
}
