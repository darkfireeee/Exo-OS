use crate::buf_pool::{NetBufPool, PAGE_SIZE};
use crate::socket_table::{SocketKind, SocketSnapshot, SocketState};
use crate::virtio_device::{ExoNetDevice, NetBufRef};
use core::cell::RefCell;
use core::cell::UnsafeCell;
use exo_syscall_abi as syscall;
use smoltcp::iface::{
    Config, Interface, PollIngressSingleResult, SocketHandle, SocketSet, SocketStorage,
};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::{tcp, udp};
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, IpAddress, IpCidr, IpEndpoint, IpListenEndpoint};

const SOCKET_STORAGE_LEN: usize = 8;
const ETHERNET_MTU_WITH_HEADER: usize = 1514;
const CLOCK_MONOTONIC: u64 = 1;
const TCP_BUFFER_SIZE: usize = 2048;
const UDP_BUFFER_SIZE: usize = 2048;
const UDP_PACKET_METADATA_LEN: usize = 4;

#[repr(C)]
#[derive(Default)]
struct Timespec {
    tv_sec: i64,
    tv_nsec: i64,
}

struct PersistentSocketStorage(UnsafeCell<[SocketStorage<'static>; SOCKET_STORAGE_LEN]>);

// SAFETY: network_server sérialise l'accès à SmoltcpIface sous NETWORK_SERVICE.
unsafe impl Sync for PersistentSocketStorage {}

static SOCKET_STORAGE: PersistentSocketStorage = PersistentSocketStorage(UnsafeCell::new(
    [const { SocketStorage::EMPTY }; SOCKET_STORAGE_LEN],
));

struct TcpBuffers {
    rx: UnsafeCell<[[u8; TCP_BUFFER_SIZE]; SOCKET_STORAGE_LEN]>,
    tx: UnsafeCell<[[u8; TCP_BUFFER_SIZE]; SOCKET_STORAGE_LEN]>,
}

struct UdpBuffers {
    rx_meta: UnsafeCell<[[udp::PacketMetadata; UDP_PACKET_METADATA_LEN]; SOCKET_STORAGE_LEN]>,
    tx_meta: UnsafeCell<[[udp::PacketMetadata; UDP_PACKET_METADATA_LEN]; SOCKET_STORAGE_LEN]>,
    rx_payload: UnsafeCell<[[u8; UDP_BUFFER_SIZE]; SOCKET_STORAGE_LEN]>,
    tx_payload: UnsafeCell<[[u8; UDP_BUFFER_SIZE]; SOCKET_STORAGE_LEN]>,
}

// SAFETY: les buffers sont uniquement empruntés sous le mutex du service réseau.
unsafe impl Sync for TcpBuffers {}
unsafe impl Sync for UdpBuffers {}

static TCP_BUFFERS: TcpBuffers = TcpBuffers {
    rx: UnsafeCell::new([[0u8; TCP_BUFFER_SIZE]; SOCKET_STORAGE_LEN]),
    tx: UnsafeCell::new([[0u8; TCP_BUFFER_SIZE]; SOCKET_STORAGE_LEN]),
};

static UDP_BUFFERS: UdpBuffers = UdpBuffers {
    rx_meta: UnsafeCell::new(
        [[const { udp::PacketMetadata::EMPTY }; UDP_PACKET_METADATA_LEN]; SOCKET_STORAGE_LEN],
    ),
    tx_meta: UnsafeCell::new(
        [[const { udp::PacketMetadata::EMPTY }; UDP_PACKET_METADATA_LEN]; SOCKET_STORAGE_LEN],
    ),
    rx_payload: UnsafeCell::new([[0u8; UDP_BUFFER_SIZE]; SOCKET_STORAGE_LEN]),
    tx_payload: UnsafeCell::new([[0u8; UDP_BUFFER_SIZE]; SOCKET_STORAGE_LEN]),
};

fn socket_set() -> SocketSet<'static> {
    // SAFETY: un seul thread de service utilise ce stockage, sous le mutex global.
    unsafe {
        let storage: &'static mut [SocketStorage<'static>; SOCKET_STORAGE_LEN] =
            &mut *SOCKET_STORAGE.0.get();
        SocketSet::new(&mut storage[..])
    }
}

fn smoltcp_now_ms(fallback: u64) -> Instant {
    let mut ts = Timespec::default();
    let rc = unsafe {
        syscall::syscall2(
            syscall::SYS_CLOCK_GETTIME,
            CLOCK_MONOTONIC,
            &mut ts as *mut Timespec as u64,
        )
    };
    if rc == 0 && ts.tv_sec >= 0 && ts.tv_nsec >= 0 {
        let ms = (ts.tv_sec as u64)
            .saturating_mul(1_000)
            .saturating_add((ts.tv_nsec as u64) / 1_000_000);
        Instant::from_millis(ms.min(i64::MAX as u64) as i64)
    } else {
        Instant::from_millis(fallback.min(i64::MAX as u64) as i64)
    }
}

pub struct SmoltcpIface {
    mac: [u8; 6],
    ip: u32,
    prefix_len: u8,
    ingress_ticks: u64,
    egress_ticks: u64,
    iface: Option<Interface>,
    socket_handles: [Option<SocketHandle>; SOCKET_STORAGE_LEN],
    socket_exo_handles: [u32; SOCKET_STORAGE_LEN],
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
            socket_handles: [None; SOCKET_STORAGE_LEN],
            socket_exo_handles: [0; SOCKET_STORAGE_LEN],
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
            socket_handles: [None; SOCKET_STORAGE_LEN],
            socket_exo_handles: [0; SOCKET_STORAGE_LEN],
        }
    }

    pub fn register_socket(&mut self, exo_handle: u32, kind: SocketKind) -> Result<(), i64> {
        if matches!(kind, SocketKind::Raw) {
            return Ok(());
        }
        let slot = socket_slot(exo_handle).ok_or(syscall::ENOBUFS)?;
        if self.socket_handles[slot].is_some() {
            return Ok(());
        }
        if self.socket_handles.iter().all(Option::is_some) {
            return Err(syscall::ENOBUFS);
        }

        let mut sockets = socket_set();
        let handle = match kind {
            SocketKind::Tcp => sockets.add(make_tcp_socket(slot)),
            SocketKind::Udp => sockets.add(make_udp_socket(slot)),
            SocketKind::Raw => return Ok(()),
        };
        self.socket_handles[slot] = Some(handle);
        self.socket_exo_handles[slot] = exo_handle;
        Ok(())
    }

    pub fn unregister_socket(&mut self, exo_handle: u32) {
        let Some(slot) = self.socket_slot_by_exo_handle(exo_handle) else {
            return;
        };
        if let Some(handle) = self.socket_handles[slot].take() {
            let mut sockets = socket_set();
            let _ = sockets.remove(handle);
        }
        self.socket_exo_handles[slot] = 0;
    }

    pub fn apply_socket_state(&mut self, snapshot: &SocketSnapshot) {
        let Some(slot) = self.socket_slot_by_exo_handle(snapshot.handle) else {
            return;
        };
        let Some(handle) = self.socket_handles[slot] else {
            return;
        };
        let mut sockets = socket_set();
        match snapshot.kind {
            SocketKind::Tcp => self.apply_tcp_state(handle, &mut sockets, snapshot),
            SocketKind::Udp => self.apply_udp_state(handle, &mut sockets, snapshot),
            SocketKind::Raw => {}
        }
    }

    pub fn poll_one(&mut self, device: &mut ExoNetDevice, pool: &NetBufPool) -> bool {
        self.ingress_ticks = self.ingress_ticks.saturating_add(1);
        let now = smoltcp_now_ms(self.ingress_ticks);
        self.ensure_iface(device, pool, now);

        let Some(iface) = self.iface.as_mut() else {
            return false;
        };
        let mut socket_set = socket_set();
        let mut smol_device = ExoSmoltcpDevice::new(device, pool);
        matches!(
            iface.poll_ingress_single(now, &mut smol_device, &mut socket_set),
            PollIngressSingleResult::PacketProcessed | PollIngressSingleResult::SocketStateChanged
        )
    }

    pub fn poll_egress(&mut self, device: &mut ExoNetDevice, pool: &NetBufPool) {
        self.egress_ticks = self.egress_ticks.saturating_add(1);
        let now = smoltcp_now_ms(self.egress_ticks);
        self.ensure_iface(device, pool, now);

        if let Some(iface) = self.iface.as_mut() {
            let mut socket_set = socket_set();
            let mut smol_device = ExoSmoltcpDevice::new(device, pool);
            let _ = iface.poll_egress(now, &mut smol_device, &mut socket_set);
        }

        while let Some(tx) = device.pop_tx_for_driver() {
            pool.tx_free(tx.pool_idx);
        }
    }

    pub fn drain_bounded(
        &mut self,
        device: &mut ExoNetDevice,
        pool: &NetBufPool,
        max_ingress_polls: usize,
    ) -> bool {
        let mut polls = 0usize;
        while self.poll_one(device, pool) {
            polls = polls.saturating_add(1);
            if polls >= max_ingress_polls {
                return false;
            }
        }
        self.poll_egress(device, pool);
        true
    }

    pub fn drain_all(&mut self, device: &mut ExoNetDevice, pool: &NetBufPool) {
        let _ = self.drain_bounded(device, pool, usize::MAX);
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

    fn socket_slot_by_exo_handle(&self, exo_handle: u32) -> Option<usize> {
        self.socket_exo_handles
            .iter()
            .position(|&stored| stored == exo_handle)
    }

    fn apply_tcp_state(
        &mut self,
        handle: SocketHandle,
        sockets: &mut SocketSet<'static>,
        snapshot: &SocketSnapshot,
    ) {
        let socket = sockets.get_mut::<tcp::Socket>(handle);
        match snapshot.state {
            SocketState::Listening if snapshot.local_port != 0 => {
                let _ = socket.listen(IpListenEndpoint {
                    addr: ip_listen_addr(snapshot.local_addr),
                    port: snapshot.local_port,
                });
            }
            SocketState::Connected if snapshot.remote_port != 0 && snapshot.local_port != 0 => {
                if let Some(iface) = self.iface.as_mut() {
                    let _ = socket.connect(
                        iface.context(),
                        IpEndpoint::new(ip_addr(snapshot.remote_addr), snapshot.remote_port),
                        IpListenEndpoint {
                            addr: ip_listen_addr(snapshot.local_addr),
                            port: snapshot.local_port,
                        },
                    );
                }
            }
            SocketState::Shutdown | SocketState::Closed => socket.abort(),
            _ => {}
        }
    }

    fn apply_udp_state(
        &mut self,
        handle: SocketHandle,
        sockets: &mut SocketSet<'static>,
        snapshot: &SocketSnapshot,
    ) {
        if snapshot.local_port == 0 {
            return;
        }
        let socket = sockets.get_mut::<udp::Socket>(handle);
        if !socket.is_open() {
            let _ = socket.bind(IpListenEndpoint {
                addr: ip_listen_addr(snapshot.local_addr),
                port: snapshot.local_port,
            });
        }
    }
}

fn socket_slot(exo_handle: u32) -> Option<usize> {
    let slot = (exo_handle & 0xffff) as usize;
    (slot < SOCKET_STORAGE_LEN).then_some(slot)
}

fn ip_addr(ip: u32) -> IpAddress {
    IpAddress::v4(
        ((ip >> 24) & 0xff) as u8,
        ((ip >> 16) & 0xff) as u8,
        ((ip >> 8) & 0xff) as u8,
        (ip & 0xff) as u8,
    )
}

fn ip_listen_addr(ip: u32) -> Option<IpAddress> {
    (ip != 0).then_some(ip_addr(ip))
}

fn make_tcp_socket(slot: usize) -> tcp::Socket<'static> {
    // SAFETY: chaque slot est alloué une seule fois tant que le socket est actif.
    unsafe {
        let rx: &'static mut [u8; TCP_BUFFER_SIZE] = &mut (*TCP_BUFFERS.rx.get())[slot];
        let tx: &'static mut [u8; TCP_BUFFER_SIZE] = &mut (*TCP_BUFFERS.tx.get())[slot];
        tcp::Socket::new(
            tcp::SocketBuffer::new(&mut rx[..]),
            tcp::SocketBuffer::new(&mut tx[..]),
        )
    }
}

fn make_udp_socket(slot: usize) -> udp::Socket<'static> {
    // SAFETY: chaque slot est alloué une seule fois tant que le socket est actif.
    unsafe {
        let rx_meta: &'static mut [udp::PacketMetadata; UDP_PACKET_METADATA_LEN] =
            &mut (*UDP_BUFFERS.rx_meta.get())[slot];
        let tx_meta: &'static mut [udp::PacketMetadata; UDP_PACKET_METADATA_LEN] =
            &mut (*UDP_BUFFERS.tx_meta.get())[slot];
        let rx_payload: &'static mut [u8; UDP_BUFFER_SIZE] =
            &mut (*UDP_BUFFERS.rx_payload.get())[slot];
        let tx_payload: &'static mut [u8; UDP_BUFFER_SIZE] =
            &mut (*UDP_BUFFERS.tx_payload.get())[slot];
        udp::Socket::new(
            udp::PacketBuffer::new(&mut rx_meta[..], &mut rx_payload[..]),
            udp::PacketBuffer::new(&mut tx_meta[..], &mut tx_payload[..]),
        )
    }
}

struct ExoSmoltcpDevice<'a> {
    device: RefCell<&'a mut ExoNetDevice>,
    pool: &'a NetBufPool,
}

impl<'a> ExoSmoltcpDevice<'a> {
    fn new(device: &'a mut ExoNetDevice, pool: &'a NetBufPool) -> Self {
        Self {
            device: RefCell::new(device),
            pool,
        }
    }

    fn alloc_tx(&self) -> Option<u16> {
        if self.pool.ready() {
            self.pool.tx_alloc()
        } else {
            None
        }
    }
}

impl<'dev> Device for ExoSmoltcpDevice<'dev> {
    type RxToken<'a>
        = ExoRxToken<'a, 'dev>
    where
        Self: 'a;
    type TxToken<'a>
        = ExoTxToken<'a, 'dev>
    where
        Self: 'a;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        if !self.pool.ready() {
            return None;
        }

        let rx = self.device.borrow_mut().pop_rx_for_stack()?;
        let tx_idx = self.alloc_tx();

        Some((
            ExoRxToken {
                device: &self.device,
                pool: self.pool,
                rx,
            },
            ExoTxToken {
                device: &self.device,
                pool: self.pool,
                pool_idx: tx_idx,
            },
        ))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        let pool_idx = self.alloc_tx()?;
        Some(ExoTxToken {
            device: &self.device,
            pool: self.pool,
            pool_idx: Some(pool_idx),
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

struct ExoRxToken<'a, 'dev> {
    device: &'a RefCell<&'dev mut ExoNetDevice>,
    pool: &'dev NetBufPool,
    rx: NetBufRef,
}

impl RxToken for ExoRxToken<'_, '_> {
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
        self.device.borrow_mut().release_rx(self.rx.pool_idx);
        result
    }
}

struct ExoTxToken<'a, 'dev> {
    device: &'a RefCell<&'dev mut ExoNetDevice>,
    pool: &'dev NetBufPool,
    pool_idx: Option<u16>,
}

impl TxToken for ExoTxToken<'_, '_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let max_len = PAGE_SIZE.saturating_sub(self.pool.hdr_size());
        let len = len.min(max_len);
        if let Some(pool_idx) = self.pool_idx {
            unsafe {
                core::ptr::write_bytes(
                    self.pool.tx_header_ptr_mut(pool_idx as usize),
                    0,
                    self.pool.hdr_size(),
                );
            }
            let payload = unsafe {
                core::slice::from_raw_parts_mut(
                    self.pool.tx_payload_ptr_mut(pool_idx as usize),
                    len,
                )
            };
            let result = f(payload);
            let queued = self.device.borrow_mut().queue_tx_idx(pool_idx, len);
            if queued.is_err() {
                self.pool.tx_free(pool_idx);
            }
            return result;
        }

        let mut drop_buf = [0u8; ETHERNET_MTU_WITH_HEADER];
        let scratch_len = len.min(drop_buf.len());
        let result = f(&mut drop_buf[..scratch_len]);
        let mut device = self.device.borrow_mut();
        device.dropped_rx_tx_token = device.dropped_rx_tx_token.saturating_add(1);
        result
    }
}
