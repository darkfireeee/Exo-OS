pub struct LoopbackState {
    rx_released: u64,
    tx_echoed: u64,
}

impl LoopbackState {
    pub const fn new() -> Self {
        Self {
            rx_released: 0,
            tx_echoed: 0,
        }
    }

    pub fn note_release(&mut self, count: u32) {
        self.rx_released = self.rx_released.saturating_add(count as u64);
    }

    pub fn note_echo(&mut self) {
        self.tx_echoed = self.tx_echoed.saturating_add(1);
    }

    pub const fn rx_released(&self) -> u64 {
        self.rx_released
    }

    pub const fn tx_echoed(&self) -> u64 {
        self.tx_echoed
    }
}
