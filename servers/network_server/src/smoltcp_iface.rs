use crate::buf_pool::NetBufPool;
use crate::virtio_device::ExoNetDevice;
use smoltcp::time::Instant;

pub struct SmoltcpIface {
    mac: [u8; 6],
    ip: u32,
    prefix_len: u8,
    ingress_ticks: u64,
    egress_ticks: u64,
}

impl SmoltcpIface {
    pub const fn empty() -> Self {
        Self {
            mac: [0; 6],
            ip: 0,
            prefix_len: 0,
            ingress_ticks: 0,
            egress_ticks: 0,
        }
    }

    pub fn init(mac: [u8; 6], ip: u32, prefix_len: u8) -> Self {
        Self {
            mac,
            ip,
            prefix_len,
            ingress_ticks: 0,
            egress_ticks: 0,
        }
    }

    pub fn poll_one(&mut self, device: &mut ExoNetDevice, pool: &NetBufPool) -> bool {
        self.ingress_ticks = self.ingress_ticks.saturating_add(1);
        let _now = Instant::from_millis(self.ingress_ticks as i64);
        device.poll_ingress_single(pool)
    }

    pub fn poll_egress(&mut self, device: &mut ExoNetDevice, pool: &NetBufPool) {
        self.egress_ticks = self.egress_ticks.saturating_add(1);
        while let Some(tx) = device.pop_tx_for_driver() {
            pool.tx_free(tx.pool_idx);
        }
    }

    pub fn drain_all(&mut self, device: &mut ExoNetDevice, pool: &NetBufPool) {
        while self.poll_one(device, pool) {}
        self.poll_egress(device, pool);
    }

    pub const fn ip(&self) -> u32 {
        self.ip
    }

    pub const fn mac(&self) -> [u8; 6] {
        self.mac
    }

    pub const fn prefix_len(&self) -> u8 {
        self.prefix_len
    }
}
