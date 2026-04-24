#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    Down,
    Up,
}

impl LinkState {
    pub const fn as_u32(self) -> u32 {
        match self {
            Self::Down => 0,
            Self::Up => 1,
        }
    }
}

#[derive(Clone, Copy)]
pub struct EthernetSnapshot {
    pub driver_pid: u32,
    pub link_state: LinkState,
    pub mtu: u32,
    pub mac: [u8; 6],
    pub queue_pairs: u16,
    pub tx_frames: u64,
    pub rx_frames: u64,
    pub drop_frames: u64,
}

pub struct EthernetPort {
    driver_pid: u32,
    link_state: LinkState,
    mtu: u32,
    mac: [u8; 6],
    queue_pairs: u16,
    tx_frames: u64,
    rx_frames: u64,
    drop_frames: u64,
}

impl EthernetPort {
    pub const fn new() -> Self {
        Self {
            driver_pid: 0,
            link_state: LinkState::Down,
            mtu: 1500,
            mac: [0; 6],
            queue_pairs: 1,
            tx_frames: 0,
            rx_frames: 0,
            drop_frames: 0,
        }
    }

    pub fn attach_driver(
        &mut self,
        driver_pid: u32,
        mtu: u32,
        mac: [u8; 6],
        queue_pairs: u16,
    ) -> EthernetSnapshot {
        self.driver_pid = driver_pid;
        self.mtu = mtu.max(576);
        self.mac = mac;
        self.queue_pairs = queue_pairs.max(1);
        self.snapshot()
    }

    pub fn set_link_state(&mut self, up: bool) -> EthernetSnapshot {
        self.link_state = if up { LinkState::Up } else { LinkState::Down };
        self.snapshot()
    }

    pub fn record_tx(&mut self, frames: u32) {
        self.tx_frames = self.tx_frames.saturating_add(frames as u64);
    }

    pub fn record_rx(&mut self, frames: u32) {
        self.rx_frames = self.rx_frames.saturating_add(frames as u64);
    }

    pub fn record_drop(&mut self, frames: u32) {
        self.drop_frames = self.drop_frames.saturating_add(frames as u64);
    }

    pub fn snapshot(&self) -> EthernetSnapshot {
        EthernetSnapshot {
            driver_pid: self.driver_pid,
            link_state: self.link_state,
            mtu: self.mtu,
            mac: self.mac,
            queue_pairs: self.queue_pairs,
            tx_frames: self.tx_frames,
            rx_frames: self.rx_frames,
            drop_frames: self.drop_frames,
        }
    }
}
