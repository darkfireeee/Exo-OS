use crate::buf_pool::NetBufPool;
use crate::driver_link::DriverLink;
use crate::smoltcp_iface::SmoltcpIface;
use crate::tcp_store::TcpStateStore;
use crate::virtio_device::ExoNetDevice;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PhoenixPhase {
    Normal,
    Draining,
    Serialized,
}

pub struct IsolationState {
    phase: PhoenixPhase,
}

impl IsolationState {
    pub const fn new() -> Self {
        Self {
            phase: PhoenixPhase::Normal,
        }
    }

    pub const fn phase(&self) -> PhoenixPhase {
        self.phase
    }

    pub fn prepare(
        &mut self,
        iface: &mut SmoltcpIface,
        device: &mut ExoNetDevice,
        pool: &NetBufPool,
        driver: &DriverLink,
        store: &mut TcpStateStore,
    ) {
        self.phase = PhoenixPhase::Draining;
        iface.drain_all(device, pool);
        driver.flush_released(device);
        store.clear();
        self.phase = PhoenixPhase::Serialized;
    }

    pub fn restore(&mut self) {
        self.phase = PhoenixPhase::Normal;
    }
}
