use crate::buf_pool::{NetBufPool, PAGE_SIZE};
use crate::virtio_device::{ExoNetDevice, NetBufRef};
use smoltcp::iface::{Config, Interface, PollIngressSingleResult, SocketSet, SocketStorage};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, IpAddress, IpCidr};

const SOCKET_STORAGE_LEN: usize = 8;
const ETHERNET_MTU_WITH_HEADER: usize = 1514;

pub struct SmoltcpIface {
    mac: [u8; 6],
    ip: u32,
    prefix_len: u8,
    ingress_ticks: u64,
    egress_ticks: u64,
    iface: Option<Interface>,
}

impl SmoltcpIface {
    pub const fn empty() -> Self {
        Self {
            mac: [0; 6],
            ip: 0,
            prefix_len: 0,
            ingress_ticks: 0,
            egress_ticks: 0,
            iface: None,
        }
    }

    pub fn init(mac: [u8; 6], ip: u32, prefix_len: u8) -> Self {
        Self {
            mac,
            ip,
            prefix_len,
            ingress_ticks: 0,
            egress_ticks: 0,
            iface: None,
        }
    }

    pub fn poll_one(&mut self, device: &mut ExoNetDevice, pool: &NetBufPool) -> bool {
        self.ingress_ticks = self.ingress_ticks.saturating_add(1);
        let now = Instant::from_millis(self.ingress_ticks as i64);
        self.ensure_iface(device, pool, now);

        let Some(iface) = self.iface.as_mut() else {
            return false;
        };
        let mut sockets = [const { SocketStorage::EMPTY }; SOCKET_STORAGE_LEN];
        let mut socket_set = SocketSet::new(&mut sockets[..]);
        let mut smol_device = ExoSmoltcpDevice::new(device, pool);
        matches!(
            iface.poll_ingress_single(now, &mut smol_device, &mut socket_set),
            PollIngressSingleResult::PacketProcessed | PollIngressSingleResult::SocketStateChanged
        )
    }

    pub fn poll_egress(&mut self, device: &mut ExoNetDevice, pool: &NetBufPool) {
        self.egress_ticks = self.egress_ticks.saturating_add(1);
        let now = Instant::from_millis(self.egress_ticks as i64);
        self.ensure_iface(device, pool, now);

        if let Some(iface) = self.iface.as_mut() {
            let mut sockets = [const { SocketStorage::EMPTY }; SOCKET_STORAGE_LEN];
            let mut socket_set = SocketSet::new(&mut sockets[..]);
            let mut smol_device = ExoSmoltcpDevice::new(device, pool);
            let _ = iface.poll_egress(now, &mut smol_device, &mut socket_set);
        }

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

    fn ensure_iface(&mut self, device: &mut ExoNetDevice, pool: &NetBufPool, now: Instant) {
        if self.iface.is_some() {
            return;
        }

        let mut smol_device = ExoSmoltcpDevice::new(device, pool);
        let mut config = Config::new(EthernetAddress(self.mac).into());
        config.random_seed = (self.ip as u64) ^ 0x4558_4f4e_4554;
        let mut iface = Interface::new(config, &mut smol_device, now);
        let ip = self.ip_cidr();
        iface.update_ip_addrs(|addrs| {
            addrs.clear();
            let _ = addrs.push(ip);
        });
        self.iface = Some(iface);
    }

    fn ip_cidr(&self) -> IpCidr {
        let a = ((self.ip >> 24) & 0xff) as u8;
        let b = ((self.ip >> 16) & 0xff) as u8;
        let c = ((self.ip >> 8) & 0xff) as u8;
        let d = (self.ip & 0xff) as u8;
        IpCidr::new(IpAddress::v4(a, b, c, d), self.prefix_len)
    }
}

struct ExoSmoltcpDevice<'a> {
    device: *mut ExoNetDevice,
    pool: &'a NetBufPool,
}

impl<'a> ExoSmoltcpDevice<'a> {
    fn new(device: &'a mut ExoNetDevice, pool: &'a NetBufPool) -> Self {
        Self { device, pool }
    }

    fn device_mut(&mut self) -> &mut ExoNetDevice {
        unsafe { &mut *self.device }
    }

    fn alloc_tx(&self) -> Option<u16> {
        if self.pool.ready() {
            self.pool.tx_alloc()
        } else {
            None
        }
    }
}

impl Device for ExoSmoltcpDevice<'_> {
    type RxToken<'a>
        = ExoRxToken<'a>
    where
        Self: 'a;
    type TxToken<'a>
        = ExoTxToken<'a>
    where
        Self: 'a;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        if !self.pool.ready() {
            return None;
        }

        let rx = self.device_mut().pop_rx_for_stack()?;
        let tx_idx = match self.alloc_tx() {
            Some(idx) => idx,
            None => {
                self.device_mut().release_rx(rx.pool_idx);
                self.device_mut().dropped_tx = self.device_mut().dropped_tx.saturating_add(1);
                return None;
            }
        };

        Some((
            ExoRxToken {
                device: self.device,
                pool: self.pool,
                rx,
            },
            ExoTxToken {
                device: self.device,
                pool: self.pool,
                pool_idx: tx_idx,
            },
        ))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        let pool_idx = self.alloc_tx()?;
        Some(ExoTxToken {
            device: self.device,
            pool: self.pool,
            pool_idx,
        })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit =
            ETHERNET_MTU_WITH_HEADER.min(PAGE_SIZE.saturating_sub(self.pool.hdr_size()));
        caps.max_burst_size = Some(16);
        caps
    }
}

struct ExoRxToken<'a> {
    device: *mut ExoNetDevice,
    pool: &'a NetBufPool,
    rx: NetBufRef,
}

impl RxToken for ExoRxToken<'_> {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        let len = (self.rx.len as usize).min(PAGE_SIZE.saturating_sub(self.pool.hdr_size()));
        let payload = unsafe {
            core::slice::from_raw_parts(
                self.pool.rx_payload_ptr_mut(self.rx.pool_idx as usize),
                len,
            )
        };
        let result = f(payload);
        unsafe {
            (&mut *self.device).release_rx(self.rx.pool_idx);
        }
        result
    }
}

struct ExoTxToken<'a> {
    device: *mut ExoNetDevice,
    pool: &'a NetBufPool,
    pool_idx: u16,
}

impl TxToken for ExoTxToken<'_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let max_len = PAGE_SIZE.saturating_sub(self.pool.hdr_size());
        let len = len.min(max_len);
        unsafe {
            core::ptr::write_bytes(
                self.pool.tx_header_ptr_mut(self.pool_idx as usize),
                0,
                self.pool.hdr_size(),
            );
        }
        let payload = unsafe {
            core::slice::from_raw_parts_mut(
                self.pool.tx_payload_ptr_mut(self.pool_idx as usize),
                len,
            )
        };
        let result = f(payload);
        let queued = unsafe { (&mut *self.device).queue_tx_idx(self.pool_idx, len) };
        if queued.is_err() {
            self.pool.tx_free(self.pool_idx);
        }
        result
    }
}
