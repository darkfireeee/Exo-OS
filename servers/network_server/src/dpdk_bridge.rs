#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BackendMode {
    Virtio,
    Dpdk,
    Xdp,
}

impl BackendMode {
    pub const fn from_u32(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::Virtio),
            1 => Some(Self::Dpdk),
            2 => Some(Self::Xdp),
            _ => None,
        }
    }

    pub const fn as_u32(self) -> u32 {
        match self {
            Self::Virtio => 0,
            Self::Dpdk => 1,
            Self::Xdp => 2,
        }
    }
}

#[derive(Clone, Copy)]
pub struct BackendSnapshot {
    pub mode: BackendMode,
    pub queue_pairs: u16,
    pub lcore_mask: u64,
    pub attached: bool,
}

pub struct DpdkBridge {
    mode: BackendMode,
    queue_pairs: u16,
    lcore_mask: u64,
    attached: bool,
}

impl DpdkBridge {
    pub const fn new() -> Self {
        Self {
            mode: BackendMode::Virtio,
            queue_pairs: 1,
            lcore_mask: 1,
            attached: false,
        }
    }

    pub fn configure(
        &mut self,
        mode: BackendMode,
        queue_pairs: u16,
        lcore_mask: u64,
    ) -> BackendSnapshot {
        self.mode = mode;
        self.queue_pairs = queue_pairs.max(1);
        self.lcore_mask = lcore_mask.max(1);
        self.attached = true;
        self.snapshot()
    }

    pub fn snapshot(&self) -> BackendSnapshot {
        BackendSnapshot {
            mode: self.mode,
            queue_pairs: self.queue_pairs,
            lcore_mask: self.lcore_mask,
            attached: self.attached,
        }
    }
}
