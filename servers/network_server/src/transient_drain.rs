use crate::buf_pool::NetBufPool;
use crate::driver_link::DriverLink;
use crate::smoltcp_iface::SmoltcpIface;
use crate::tcp_store::TcpStateStore;
use crate::virtio_device::ExoNetDevice;
use exo_syscall_abi as syscall;

const PHOENIX_DRAIN_MAX_POLLS: usize = 1024;

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

    fn sync_kernel_phase(phase: PhoenixPhase) {
        let state = match phase {
            PhoenixPhase::Normal => syscall::ExoPhoenixStateWire::Normal.as_syscall_arg(),
            PhoenixPhase::Draining => {
                syscall::ExoPhoenixStateWire::NetworkDraining.as_syscall_arg()
            }
            PhoenixPhase::Serialized => {
                syscall::ExoPhoenixStateWire::NetworkSerialized.as_syscall_arg()
            }
        };
        // SAFETY: best-effort kernel state synchronization; failures keep the
        // network server local phase coherent and are observed by later health checks.
        let _ = unsafe { syscall::syscall1(syscall::SYS_EXO_PHOENIX_STATE_SET, state) };
    }

    pub fn prepare(
        &mut self,
        iface: &mut SmoltcpIface,
        device: &mut ExoNetDevice,
        pool: &NetBufPool,
        driver: &DriverLink,
        store: &mut TcpStateStore,
    ) -> bool {
        self.phase = PhoenixPhase::Draining;
        Self::sync_kernel_phase(self.phase);
        let drained = iface.drain_bounded(device, pool, PHOENIX_DRAIN_MAX_POLLS);
        driver.flush_released(device);
        if !drained {
            self.phase = PhoenixPhase::Normal;
            Self::sync_kernel_phase(self.phase);
            return false;
        }
        store.clear();
        self.phase = PhoenixPhase::Serialized;
        Self::sync_kernel_phase(self.phase);
        true
    }

    pub fn restore(&mut self) {
        self.phase = PhoenixPhase::Normal;
        Self::sync_kernel_phase(self.phase);
    }
}
