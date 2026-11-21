use crate::buf_pool::{NetBufPool, PAGE_SIZE};
use crate::socket_table::{SocketKind, SocketSnapshot, SocketState, MAX_SOCKETS};
use crate::virtio_device::{ExoNetDevice, NetBufRef};
use core::cell::{RefCell, UnsafeCell};
use core::mem::MaybeUninit;
use core::sync::atomic::AtomicBool;
use exo_syscall_abi as syscall;
use smoltcp::iface::{
    Config, Interface, PollIngressSingleResult, SocketHandle, SocketSet, SocketStorage,
};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::{icmp, tcp, udp};
use smoltcp::time::Instant;
use smoltcp::wire::{
    EthernetAddress, HardwareAddress, IpAddress, IpCidr, IpEndpoint, IpListenEndpoint, Ipv4Address,
};

const LISTEN_BACKLOG_SLOTS: usize = 4;
const EXTRA_SLOTS: usize = 33; // Up to 32 extra TCP listener backlog slots + 1 internal DHCP slot
const SOCKET_STORAGE_LEN: usize = MAX_SOCKETS + EXTRA_SLOTS;
const ETHERNET_MTU_WITH_HEADER: usize = 1514;
const LOOPBACK_MTU: usize = 2048;
const LOOPBACK_RING_LEN: usize = 64;
const CLOCK_MONOTONIC: u64 = 1;
const TCP_BUFFER_SIZE: usize = 16384;
const UDP_BUFFER_SIZE: usize = 16384;
const UDP_PACKET_METADATA_LEN: usize = 8;
const ICMP_BUFFER_SIZE: usize = 256;
const ICMP_PACKET_METADATA_LEN: usize = 4;

const EPOLLIN: u32 = 0x0001;
const EPOLLOUT: u32 = 0x0004;
const EPOLLERR: u32 = 0x0008;
const EPOLLHUP: u32 = 0x0010;
const EPOLLRDHUP: u32 = 0x2000;

static LOGGED_STACK_RX: AtomicBool = AtomicBool::new(false);
static LOGGED_STACK_TX: AtomicBool = AtomicBool::new(false);
static LAST_EGRESS_WAS_ARP: AtomicBool = AtomicBool::new(false);
static LAST_EGRESS_SENT_IPV4: AtomicBool = AtomicBool::new(false);

#[repr(C)]
#[derive(Default)]
struct Timespec {
    tv_sec: i64,
    tv_nsec: i64,
}

#[inline]
pub const fn is_loopback_ipv4(ip: u32) -> bool {
    (ip & 0xff00_0000) == 0x7f00_0000
}

struct PersistentSocketStorage {
    slots: UnsafeCell<[MaybeUninit<SocketStorage<'static>>; SOCKET_STORAGE_LEN]>,
    initialized: UnsafeCell<bool>,
}

// SAFETY: network_server sérialise l'accès à SmoltcpIface sous NETWORK_SERVICE.
unsafe impl Sync for PersistentSocketStorage {}

static SOCKET_STORAGE: PersistentSocketStorage = PersistentSocketStorage {
    slots: UnsafeCell::new([const { MaybeUninit::uninit() }; SOCKET_STORAGE_LEN]),
    initialized: UnsafeCell::new(false),
};

struct TcpBuffers {
    rx: UnsafeCell<[[u8; TCP_BUFFER_SIZE]; SOCKET_STORAGE_LEN]>,
    tx: UnsafeCell<[[u8; TCP_BUFFER_SIZE]; SOCKET_STORAGE_LEN]>,
}

struct UdpBuffers {
    rx_meta: UnsafeCell<
        [[MaybeUninit<udp::PacketMetadata>; UDP_PACKET_METADATA_LEN]; SOCKET_STORAGE_LEN],
    >,
    tx_meta: UnsafeCell<
        [[MaybeUninit<udp::PacketMetadata>; UDP_PACKET_METADATA_LEN]; SOCKET_STORAGE_LEN],
    >,
    rx_payload: UnsafeCell<[[u8; UDP_BUFFER_SIZE]; SOCKET_STORAGE_LEN]>,
    tx_payload: UnsafeCell<[[u8; UDP_BUFFER_SIZE]; SOCKET_STORAGE_LEN]>,
}

struct IcmpBuffers {
    rx_meta: UnsafeCell<[[icmp::PacketMetadata; ICMP_PACKET_METADATA_LEN]; SOCKET_STORAGE_LEN]>,
    tx_meta: UnsafeCell<[[icmp::PacketMetadata; ICMP_PACKET_METADATA_LEN]; SOCKET_STORAGE_LEN]>,
    rx_payload: UnsafeCell<[[u8; ICMP_BUFFER_SIZE]; SOCKET_STORAGE_LEN]>,
    tx_payload: UnsafeCell<[[u8; ICMP_BUFFER_SIZE]; SOCKET_STORAGE_LEN]>,
}

// SAFETY: les buffers sont uniquement empruntés sous le mutex du service réseau.
unsafe impl Sync for TcpBuffers {}
unsafe impl Sync for UdpBuffers {}
unsafe impl Sync for IcmpBuffers {}

static TCP_BUFFERS: TcpBuffers = TcpBuffers {
    rx: UnsafeCell::new([[0u8; TCP_BUFFER_SIZE]; SOCKET_STORAGE_LEN]),
    tx: UnsafeCell::new([[0u8; TCP_BUFFER_SIZE]; SOCKET_STORAGE_LEN]),
};

static UDP_BUFFERS: UdpBuffers = UdpBuffers {
    rx_meta: UnsafeCell::new(
        [[const { MaybeUninit::uninit() }; UDP_PACKET_METADATA_LEN]; SOCKET_STORAGE_LEN],
    ),
    tx_meta: UnsafeCell::new(
        [[const { MaybeUninit::uninit() }; UDP_PACKET_METADATA_LEN]; SOCKET_STORAGE_LEN],
    ),
    rx_payload: UnsafeCell::new([[0u8; UDP_BUFFER_SIZE]; SOCKET_STORAGE_LEN]),
    tx_payload: UnsafeCell::new([[0u8; UDP_BUFFER_SIZE]; SOCKET_STORAGE_LEN]),
};

static ICMP_BUFFERS: IcmpBuffers = IcmpBuffers {
    rx_meta: UnsafeCell::new(
        [[const { icmp::PacketMetadata::EMPTY }; ICMP_PACKET_METADATA_LEN]; SOCKET_STORAGE_LEN],
    ),
    tx_meta: UnsafeCell::new(
        [[const { icmp::PacketMetadata::EMPTY }; ICMP_PACKET_METADATA_LEN]; SOCKET_STORAGE_LEN],
    ),
    rx_payload: UnsafeCell::new([[0u8; ICMP_BUFFER_SIZE]; SOCKET_STORAGE_LEN]),
    tx_payload: UnsafeCell::new([[0u8; ICMP_BUFFER_SIZE]; SOCKET_STORAGE_LEN]),
};

// ─────────────────────────────────────────────────────────────────────────────
// In-process IP Loopback Device (127.0.0.1/8 — zero ARP, instant local delivery)
// ─────────────────────────────────────────────────────────────────────────────

struct LoopbackQueue {
    frames: [[u8; LOOPBACK_MTU]; LOOPBACK_RING_LEN],
    lens: [usize; LOOPBACK_RING_LEN],
    head: usize,
    tail: usize,
    count: usize,
}

impl LoopbackQueue {
    const fn new() -> Self {
        Self {
            frames: [[0u8; LOOPBACK_MTU]; LOOPBACK_RING_LEN],
            lens: [0; LOOPBACK_RING_LEN],
            head: 0,
            tail: 0,
            count: 0,
        }
    }

    fn push_frame(&mut self, len: usize) -> Option<&mut [u8]> {
        if self.count >= LOOPBACK_RING_LEN || len > LOOPBACK_MTU {
            return None;
        }
        let idx = self.tail & (LOOPBACK_RING_LEN - 1);
        self.lens[idx] = len;
        self.tail = self.tail.wrapping_add(1);
        self.count += 1;
        Some(&mut self.frames[idx][..len])
    }

    fn pop_frame(&mut self) -> Option<([u8; LOOPBACK_MTU], usize)> {
        if self.count == 0 {
            return None;
        }
        let idx = self.head & (LOOPBACK_RING_LEN - 1);
        let len = self.lens[idx];
        let mut copy = [0u8; LOOPBACK_MTU];
        copy[..len].copy_from_slice(&self.frames[idx][..len]);
        self.head = self.head.wrapping_add(1);
        self.count -= 1;
        Some((copy, len))
    }

    const fn is_empty(&self) -> bool {
        self.count == 0
    }
}

struct LoopbackStorage {
    queue: UnsafeCell<LoopbackQueue>,
}

// SAFETY: uniquement emprunté sous NETWORK_SERVICE.
unsafe impl Sync for LoopbackStorage {}

static LOOPBACK_QUEUE: LoopbackStorage = LoopbackStorage {
    queue: UnsafeCell::new(LoopbackQueue::new()),
};

struct LoopbackIpDevice;

struct LoopbackRxToken {
    buf: [u8; LOOPBACK_MTU],
    len: usize,
}

struct LoopbackTxToken;

impl RxToken for LoopbackRxToken {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.buf[..self.len])
    }
}

impl TxToken for LoopbackTxToken {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        // SAFETY: accès sérialisé par NETWORK_SERVICE.
        let q = unsafe { &mut *LOOPBACK_QUEUE.queue.get() };
        if let Some(slot) = q.push_frame(len) {
            f(slot)
        } else {
            let mut drop_buf = [0u8; LOOPBACK_MTU];
            let clamped = len.min(drop_buf.len());
            f(&mut drop_buf[..clamped])
        }
    }
}

impl Device for LoopbackIpDevice {
    type RxToken<'a> = LoopbackRxToken;
    type TxToken<'a> = LoopbackTxToken;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let q = unsafe { &mut *LOOPBACK_QUEUE.queue.get() };
        let (buf, len) = q.pop_frame()?;
        Some((LoopbackRxToken { buf, len }, LoopbackTxToken))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        let q = unsafe { &*LOOPBACK_QUEUE.queue.get() };
        if q.count < LOOPBACK_RING_LEN {
            Some(LoopbackTxToken)
        } else {
            None
        }
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ip;
        caps.max_transmission_unit = LOOPBACK_MTU;
        caps.max_burst_size = Some(16);
        caps
    }
}

fn socket_set() -> SocketSet<'static> {
    // SAFETY: un seul thread de service utilise ce stockage, sous le mutex global.
    unsafe {
        let slots = &mut *SOCKET_STORAGE.slots.get();
        if !*SOCKET_STORAGE.initialized.get() {
            let mut idx = 0usize;
            while idx < SOCKET_STORAGE_LEN {
                slots[idx].write(SocketStorage::EMPTY);
                idx += 1;
            }
            *SOCKET_STORAGE.initialized.get() = true;
        }
        let storage = core::slice::from_raw_parts_mut(
            slots.as_mut_ptr() as *mut SocketStorage<'static>,
            SOCKET_STORAGE_LEN,
        );
        SocketSet::new(storage)
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

#[derive(Clone, Copy)]
struct ListenerPool {
    active: bool,
    listener_exo_handle: u32,
    local_addr: u32,
    local_port: u16,
    slots: [Option<usize>; LISTEN_BACKLOG_SLOTS],
}

impl ListenerPool {
    const fn empty() -> Self {
        Self {
            active: false,
            listener_exo_handle: 0,
            local_addr: 0,
            local_port: 0,
            slots: [None; LISTEN_BACKLOG_SLOTS],
        }
    }
}

const MAX_LISTENERS: usize = 8;

pub struct SmoltcpIface {
    mac: [u8; 6],
    ip: u32,
    prefix_len: u8,
    gateway: u32,
    ingress_ticks: u64,
    egress_ticks: u64,
    iface: Option<Interface>,
    lo_iface: Option<Interface>,
    socket_handles: [Option<SocketHandle>; SOCKET_STORAGE_LEN],
    socket_exo_handles: [u32; SOCKET_STORAGE_LEN],
    listener_pools: [ListenerPool; MAX_LISTENERS],
    dhcp_slot: Option<usize>,
}

pub enum TcpConnectStatus {
    Pending,
    Established,
    Failed,
}

impl SmoltcpIface {
    pub const fn empty() -> Self {
        Self {
            mac: [0; 6],
            ip: 0,
            prefix_len: 0,
            gateway: 0x0a00_0202,
            ingress_ticks: 0,
            egress_ticks: 0,
            iface: None,
            lo_iface: None,
            socket_handles: [None; SOCKET_STORAGE_LEN],
            socket_exo_handles: [0; SOCKET_STORAGE_LEN],
            listener_pools: [ListenerPool::empty(); MAX_LISTENERS],
            dhcp_slot: None,
        }
    }

    pub fn init(mac: [u8; 6], ip: u32, prefix_len: u8) -> Self {
        Self {
            mac,
            ip,
            prefix_len,
            gateway: 0x0a00_0202,
            ingress_ticks: 0,
            egress_ticks: 0,
            iface: None,
            lo_iface: None,
            socket_handles: [None; SOCKET_STORAGE_LEN],
            socket_exo_handles: [0; SOCKET_STORAGE_LEN],
            listener_pools: [ListenerPool::empty(); MAX_LISTENERS],
            dhcp_slot: None,
        }
    }

    fn alloc_free_slot(&self) -> Option<usize> {
        self.socket_handles.iter().position(Option::is_none)
    }

    pub fn register_socket(&mut self, exo_handle: u32, kind: SocketKind) -> Result<(), i64> {
        if self.socket_slot_by_exo_handle(exo_handle).is_some() {
            return Ok(());
        }
        let slot = self.alloc_free_slot().ok_or(syscall::ENOBUFS)?;

        let mut sockets = socket_set();
        let handle = match kind {
            SocketKind::Tcp => sockets.add(make_tcp_socket(slot)),
            SocketKind::Udp => sockets.add(make_udp_socket(slot)),
            SocketKind::Raw => sockets.add(make_icmp_socket(slot)),
        };
        self.socket_handles[slot] = Some(handle);
        self.socket_exo_handles[slot] = exo_handle;
        Ok(())
    }

    pub fn unregister_socket(&mut self, exo_handle: u32) {
        // If this handle owns a listener pool, tear down all extra backlog slots too.
        for pool in &mut self.listener_pools {
            if pool.active && pool.listener_exo_handle == exo_handle {
                let mut sockets = socket_set();
                for s_opt in &mut pool.slots {
                    if let Some(slot) = s_opt.take() {
                        if let Some(h) = self.socket_handles[slot].take() {
                            let _ = sockets.remove(h);
                        }
                        self.socket_exo_handles[slot] = 0;
                    }
                }
                *pool = ListenerPool::empty();
                return;
            }
        }
        let Some(slot) = self.socket_slot_by_exo_handle(exo_handle) else {
            return;
        };
        if let Some(handle) = self.socket_handles[slot].take() {
            let mut sockets = socket_set();
            let _ = sockets.remove(handle);
        }
        self.socket_exo_handles[slot] = 0;
    }

    /// Gracefully closes a TCP socket (sends FIN) and pumps loopback/egress before removing.
    pub fn close_socket_graceful(
        &mut self,
        snapshot: &SocketSnapshot,
        device: &mut ExoNetDevice,
        pool: &NetBufPool,
    ) {
        if snapshot.kind == SocketKind::Tcp && snapshot.state != SocketState::Listening {
            if let Some(slot) = self.socket_slot_by_exo_handle(snapshot.handle) {
                if let Some(handle) = self.socket_handles[slot] {
                    {
                        let mut sockets = socket_set();
                        let sock = sockets.get_mut::<tcp::Socket>(handle);
                        sock.close();
                    }
                    let _ = self.poll_loopback();
                    let _ = self.poll_egress(device, pool);
                }
            }
        }
        self.unregister_socket(snapshot.handle);
    }

    pub fn ensure_interface(&mut self, device: &mut ExoNetDevice, pool: &NetBufPool) {
        let now = smoltcp_now_ms(self.egress_ticks.max(self.ingress_ticks));
        self.ensure_lo_iface(now);
        self.ensure_iface(device, pool, now);
    }

    pub fn ensure_loopback(&mut self) {
        let now = smoltcp_now_ms(self.egress_ticks.max(self.ingress_ticks));
        self.ensure_lo_iface(now);
    }

    pub fn reconfigure_ipv4(&mut self, ip: u32, prefix_len: u8, gateway: u32) {
        if ip == 0 || prefix_len == 0 || prefix_len > 32 {
            return;
        }
        self.ip = ip;
        self.prefix_len = prefix_len;
        if gateway != 0 {
            self.gateway = gateway;
        }
        let cidr = self.ip_cidr();
        let gw = self.gateway;
        if let Some(iface) = self.iface.as_mut() {
            iface.update_ip_addrs(|addrs| {
                addrs.clear();
                let _ = addrs.push(cidr);
            });
            if gw != 0 {
                let _ = iface
                    .routes_mut()
                    .add_default_ipv4_route(ipv4_from_u32(gw));
            }
        }
    }

    /// Polls the in-process `127.0.0.1/8` loopback interface until quiescent (bounded).
    pub fn poll_loopback(&mut self) -> bool {
        let now = smoltcp_now_ms(self.egress_ticks.max(self.ingress_ticks));
        self.ensure_lo_iface(now);
        let Some(lo_iface) = self.lo_iface.as_mut() else {
            return false;
        };
        let mut sockets = socket_set();
        let mut lo_dev = LoopbackIpDevice;
        let mut activity = false;
        for _ in 0..32 {
            let eg = lo_iface.poll_egress(now, &mut lo_dev, &mut sockets);
            let mut ing_any = false;
            for _ in 0..16 {
                match lo_iface.poll_ingress_single(now, &mut lo_dev, &mut sockets) {
                    PollIngressSingleResult::None => break,
                    PollIngressSingleResult::PacketProcessed
                    | PollIngressSingleResult::SocketStateChanged => {
                        ing_any = true;
                        activity = true;
                    }
                }
            }
            let eg2 = lo_iface.poll_egress(now, &mut lo_dev, &mut sockets);
            let q_empty = unsafe { (*LOOPBACK_QUEUE.queue.get()).is_empty() };
            let progressed =
                ing_any || matches!(eg, smoltcp::iface::PollResult::SocketStateChanged)
                    || matches!(eg2, smoltcp::iface::PollResult::SocketStateChanged);
            if progressed {
                activity = true;
            }
            if !progressed && q_empty {
                break;
            }
        }
        activity
    }

    pub fn apply_socket_state(&mut self, snapshot: &SocketSnapshot) {
        if snapshot.kind == SocketKind::Tcp && snapshot.state == SocketState::Listening {
            self.setup_tcp_listener_pool(snapshot);
            return;
        }
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
            SocketKind::Raw => self.apply_icmp_state(handle, &mut sockets, snapshot),
        }
    }

    pub fn apply_socket_options(&mut self, snapshot: &SocketSnapshot) {
        let Some(slot) = self.socket_slot_by_exo_handle(snapshot.handle) else {
            return;
        };
        let Some(handle) = self.socket_handles[slot] else {
            return;
        };
        let mut sockets = socket_set();
        match snapshot.kind {
            SocketKind::Tcp => {
                let sock = sockets.get_mut::<tcp::Socket>(handle);
                sock.set_nagle_enabled(!snapshot.nodelay);
                sock.set_hop_limit(Some(snapshot.ttl.max(1)));
            }
            SocketKind::Udp => {
                let sock = sockets.get_mut::<udp::Socket>(handle);
                sock.set_hop_limit(Some(snapshot.ttl.max(1)));
            }
            SocketKind::Raw => {
                let sock = sockets.get_mut::<icmp::Socket>(handle);
                sock.set_hop_limit(Some(snapshot.ttl.max(1)));
            }
        }
    }

    fn setup_tcp_listener_pool(&mut self, snapshot: &SocketSnapshot) {
        if snapshot.local_port == 0 {
            return;
        }
        let desired = (snapshot.backlog as usize).clamp(1, LISTEN_BACKLOG_SLOTS);
        // Check if pool already exists for this listener.
        let pool_idx = if let Some(idx) = self
            .listener_pools
            .iter()
            .position(|p| p.active && p.listener_exo_handle == snapshot.handle)
        {
            idx
        } else if let Some(idx) = self.listener_pools.iter().position(|p| !p.active) {
            self.listener_pools[idx] = ListenerPool {
                active: true,
                listener_exo_handle: snapshot.handle,
                local_addr: snapshot.local_addr,
                local_port: snapshot.local_port,
                slots: [None; LISTEN_BACKLOG_SLOTS],
            };
            // Adopt the primary slot already registered for this listener handle.
            if let Some(primary_slot) = self.socket_slot_by_exo_handle(snapshot.handle) {
                self.listener_pools[idx].slots[0] = Some(primary_slot);
            }
            idx
        } else {
            return;
        };

        let listen_ep = IpListenEndpoint {
            addr: ip_listen_addr(snapshot.local_addr),
            port: snapshot.local_port,
        };
        let mut sockets = socket_set();
        for k in 0..desired {
            let slot = if let Some(existing) = self.listener_pools[pool_idx].slots[k] {
                existing
            } else {
                let Some(free) = self.alloc_free_slot() else {
                    break;
                };
                let h = sockets.add(make_tcp_socket(free));
                self.socket_handles[free] = Some(h);
                self.socket_exo_handles[free] = snapshot.handle;
                self.listener_pools[pool_idx].slots[k] = Some(free);
                free
            };
            if let Some(handle) = self.socket_handles[slot] {
                let sock = sockets.get_mut::<tcp::Socket>(handle);
                if sock.state() == tcp::State::Closed {
                    let _ = sock.listen(listen_ep);
                }
            }
        }
    }

    pub fn has_acceptable_connection(&self, listener_exo_handle: u32) -> bool {
        let Some(pool) = self
            .listener_pools
            .iter()
            .find(|p| p.active && p.listener_exo_handle == listener_exo_handle)
        else {
            return false;
        };
        let sockets = socket_set();
        for slot_opt in pool.slots {
            let Some(slot) = slot_opt else {
                continue;
            };
            let Some(handle) = self.socket_handles[slot] else {
                continue;
            };
            let state = sockets.get::<tcp::Socket>(handle).state();
            if !matches!(
                state,
                tcp::State::Listen | tcp::State::Closed | tcp::State::SynReceived
            ) {
                return true;
            }
        }
        false
    }

    /// Detaches one ready accepted TCP connection from `listener_exo_handle` and
    /// binds it to `child_exo_handle`, immediately replenishing a fresh listening slot.
    pub fn accept_ready_connection(
        &mut self,
        listener_exo_handle: u32,
        child_exo_handle: u32,
    ) -> Result<(u32, u16, u32, u16), i64> {
        let Some(pool_idx) = self
            .listener_pools
            .iter()
            .position(|p| p.active && p.listener_exo_handle == listener_exo_handle)
        else {
            return Err(syscall::EINVAL);
        };

        let mut ready_k: Option<usize> = None;
        let mut endpoints: (u32, u16, u32, u16) = (0, 0, 0, 0);
        {
            let sockets = socket_set();
            for (k, slot_opt) in self.listener_pools[pool_idx].slots.iter().enumerate() {
                let Some(slot) = *slot_opt else {
                    continue;
                };
                let Some(handle) = self.socket_handles[slot] else {
                    continue;
                };
                let sock = sockets.get::<tcp::Socket>(handle);
                if !matches!(
                    sock.state(),
                    tcp::State::Listen | tcp::State::Closed | tcp::State::SynReceived
                ) {
                    let (la, lp) = sock
                        .local_endpoint()
                        .and_then(endpoint_to_v4)
                        .unwrap_or((
                            self.listener_pools[pool_idx].local_addr,
                            self.listener_pools[pool_idx].local_port,
                        ));
                    let (ra, rp) = sock
                        .remote_endpoint()
                        .and_then(endpoint_to_v4)
                        .unwrap_or((0, 0));
                    endpoints = (la, lp, ra, rp);
                    ready_k = Some(k);
                    break;
                }
            }
        }

        let Some(k) = ready_k else {
            return Err(syscall::EAGAIN);
        };
        let accepted_slot = self.listener_pools[pool_idx].slots[k].take().unwrap();
        // Reassign the established smoltcp slot directly to `child_exo_handle`.
        self.socket_exo_handles[accepted_slot] = child_exo_handle;

        // Immediately replenish the listener slot with a new listening TCP socket.
        if let Some(new_slot) = self.alloc_free_slot() {
            let listen_ep = IpListenEndpoint {
                addr: ip_listen_addr(self.listener_pools[pool_idx].local_addr),
                port: self.listener_pools[pool_idx].local_port,
            };
            let mut sockets = socket_set();
            let mut sock = make_tcp_socket(new_slot);
            let _ = sock.listen(listen_ep);
            let h = sockets.add(sock);
            self.socket_handles[new_slot] = Some(h);
            self.socket_exo_handles[new_slot] = listener_exo_handle;
            self.listener_pools[pool_idx].slots[k] = Some(new_slot);
        }

        Ok(endpoints)
    }

    /// Computes real Linux `poll`/`epoll` readiness mask + available RX bytes for `snapshot`.
    pub fn poll_socket(&self, snapshot: &SocketSnapshot) -> (u32, u32) {
        if snapshot.kind == SocketKind::Tcp && snapshot.state == SocketState::Listening {
            return if self.has_acceptable_connection(snapshot.handle) {
                (EPOLLIN, 1)
            } else {
                (0, 0)
            };
        }

        let Some(slot) = self.socket_slot_by_exo_handle(snapshot.handle) else {
            let mut m = EPOLLHUP;
            if snapshot.so_error != 0 {
                m |= EPOLLERR | EPOLLOUT;
            }
            return (m, 0);
        };
        let Some(handle) = self.socket_handles[slot] else {
            return (EPOLLHUP, 0);
        };

        let sockets = socket_set();
        let mut mask = 0u32;
        if snapshot.so_error != 0 {
            mask |= EPOLLERR;
        }
        match snapshot.kind {
            SocketKind::Tcp => {
                let sock = sockets.get::<tcp::Socket>(handle);
                if snapshot.state == SocketState::Connecting {
                    match sock.state() {
                        tcp::State::Established | tcp::State::CloseWait => {
                            mask |= EPOLLOUT;
                            if sock.can_recv() || !sock.may_recv() {
                                mask |= EPOLLIN;
                            }
                        }
                        tcp::State::Closed | tcp::State::TimeWait => {
                            mask |= EPOLLOUT | EPOLLERR | EPOLLHUP;
                        }
                        _ => {}
                    }
                    return (mask, sock.recv_queue() as u32);
                }
                if snapshot.state == SocketState::Closed {
                    mask |= EPOLLHUP | EPOLLIN | EPOLLOUT;
                    return (mask, 0);
                }
                let avail = sock.recv_queue() as u32;
                if sock.can_recv() || !sock.may_recv() || snapshot.shutdown_rd {
                    mask |= EPOLLIN;
                }
                if !sock.may_recv() {
                    mask |= EPOLLRDHUP;
                }
                if !snapshot.shutdown_wr && sock.can_send() {
                    mask |= EPOLLOUT;
                }
                if !sock.may_recv() && !sock.may_send() {
                    mask |= EPOLLHUP;
                }
                (mask, avail)
            }
            SocketKind::Udp => {
                let sock = sockets.get::<udp::Socket>(handle);
                let mut avail = 0u32;
                if sock.can_recv() || snapshot.shutdown_rd {
                    mask |= EPOLLIN;
                    if sock.can_recv() {
                        avail = 1;
                    }
                }
                if !snapshot.shutdown_wr && sock.can_send() {
                    mask |= EPOLLOUT;
                }
                (mask, avail)
            }
            SocketKind::Raw => {
                let sock = sockets.get::<icmp::Socket>(handle);
                let avail = if sock.can_recv() { 1 } else { 0 };
                if sock.can_recv() {
                    mask |= EPOLLIN;
                }
                if sock.can_send() {
                    mask |= EPOLLOUT;
                }
                (mask, avail)
            }
        }
    }

    pub fn shutdown_tcp(
        &mut self,
        snapshot: &SocketSnapshot,
        how: u32,
        device: &mut ExoNetDevice,
        pool: &NetBufPool,
    ) {
        if snapshot.kind != SocketKind::Tcp {
            return;
        }
        if how == 1 || how == 2 {
            if let Some(slot) = self.socket_slot_by_exo_handle(snapshot.handle) {
                if let Some(handle) = self.socket_handles[slot] {
                    {
                        let mut sockets = socket_set();
                        let sock = sockets.get_mut::<tcp::Socket>(handle);
                        sock.close();
                    }
                    let _ = self.poll_loopback();
                    let _ = self.poll_egress(device, pool);
                }
            }
        }
    }

    pub fn poll_one(&mut self, device: &mut ExoNetDevice, pool: &NetBufPool) -> bool {
        self.ingress_ticks = self.ingress_ticks.saturating_add(1);
        let now = smoltcp_now_ms(self.ingress_ticks);
        self.ensure_lo_iface(now);
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

    pub fn poll_egress(&mut self, device: &mut ExoNetDevice, pool: &NetBufPool) -> bool {
        use core::sync::atomic::Ordering;
        LAST_EGRESS_WAS_ARP.store(false, Ordering::Relaxed);
        LAST_EGRESS_SENT_IPV4.store(false, Ordering::Relaxed);

        self.egress_ticks = self.egress_ticks.saturating_add(1);
        let now = smoltcp_now_ms(self.egress_ticks);
        self.ensure_lo_iface(now);
        self.ensure_iface(device, pool, now);

        if let Some(iface) = self.iface.as_mut() {
            let mut socket_set = socket_set();
            let mut smol_device = ExoSmoltcpDevice::new(device, pool);
            let _ = iface.poll_egress(now, &mut smol_device, &mut socket_set);
        }

        LAST_EGRESS_WAS_ARP.load(Ordering::Relaxed) && !LAST_EGRESS_SENT_IPV4.load(Ordering::Relaxed)
    }

    // ── Internal DHCP UDP socket helpers ─────────────────────────────────────

    pub fn ensure_dhcp_socket(&mut self) -> bool {
        if self.dhcp_slot.is_some() {
            return true;
        }
        let Some(slot) = self.alloc_free_slot() else {
            return false;
        };
        let mut sockets = socket_set();
        let mut sock = make_udp_socket(slot);
        let _ = sock.bind(IpListenEndpoint {
            addr: None,
            port: crate::dhcp::DHCP_CLIENT_PORT,
        });
        let h = sockets.add(sock);
        self.socket_handles[slot] = Some(h);
        self.socket_exo_handles[slot] = 0xFFFF_FFFE; // reserved internal DHCP marker
        self.dhcp_slot = Some(slot);
        true
    }

    pub fn dhcp_send(&mut self, packet: &[u8]) -> bool {
        if !self.ensure_dhcp_socket() {
            return false;
        }
        let Some(slot) = self.dhcp_slot else {
            return false;
        };
        let Some(handle) = self.socket_handles[slot] else {
            return false;
        };
        let mut sockets = socket_set();
        let sock = sockets.get_mut::<udp::Socket>(handle);
        if !sock.can_send() {
            return false;
        }
        sock.send_slice(
            packet,
            IpEndpoint::new(IpAddress::v4(255, 255, 255, 255), crate::dhcp::DHCP_SERVER_PORT),
        )
        .is_ok()
    }

    pub fn dhcp_recv(&mut self, out: &mut [u8]) -> Option<usize> {
        let slot = self.dhcp_slot?;
        let handle = self.socket_handles[slot]?;
        let mut sockets = socket_set();
        let sock = sockets.get_mut::<udp::Socket>(handle);
        match sock.recv_slice(out) {
            Ok((n, _)) => Some(n),
            Err(_) => None,
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

    pub fn send_socket_data(
        &mut self,
        snapshot: &SocketSnapshot,
        data: &[u8],
    ) -> Result<usize, i64> {
        if data.is_empty() {
            return Ok(0);
        }
        let Some(slot) = self.socket_slot_by_exo_handle(snapshot.handle) else {
            return Err(syscall::EBADF);
        };
        let Some(handle) = self.socket_handles[slot] else {
            return Err(syscall::EBADF);
        };
        if snapshot.remote_addr == 0
            || (snapshot.remote_port == 0 && snapshot.kind != SocketKind::Raw)
        {
            return Err(syscall::ENOTCONN);
        }

        let mut sockets = socket_set();
        match snapshot.kind {
            SocketKind::Udp => {
                if data.len() > UDP_BUFFER_SIZE {
                    return Err(syscall::EMSGSIZE);
                }
                let socket = sockets.get_mut::<udp::Socket>(handle);
                if !socket.is_open() && snapshot.local_port != 0 {
                    let _ = socket.bind(IpListenEndpoint {
                        addr: ip_listen_addr(snapshot.local_addr),
                        port: snapshot.local_port,
                    });
                }
                if !socket.can_send() {
                    return Err(syscall::EAGAIN);
                }
                socket
                    .send_slice(
                        data,
                        IpEndpoint::new(ip_addr(snapshot.remote_addr), snapshot.remote_port),
                    )
                    .map_err(|_| syscall::EAGAIN)?;
                Ok(data.len())
            }
            SocketKind::Tcp => {
                let socket = sockets.get_mut::<tcp::Socket>(handle);
                if !socket.may_send() {
                    return Err(syscall::EPIPE);
                }
                if !socket.can_send() {
                    return Err(syscall::EAGAIN);
                }
                match socket.send_slice(data) {
                    Ok(0) => Err(syscall::EAGAIN),
                    Ok(n) => Ok(n),
                    Err(_) => Err(syscall::EPIPE),
                }
            }
            SocketKind::Raw => {
                let socket = sockets.get_mut::<icmp::Socket>(handle);
                let icmp_ident = if data.len() >= 6 {
                    u16::from_be_bytes([data[4], data[5]])
                } else {
                    snapshot.local_port.max(1)
                };
                if !socket.is_open() {
                    socket
                        .bind(icmp::Endpoint::Ident(icmp_ident))
                        .map_err(|_| syscall::EINVAL)?;
                }
                socket
                    .send_slice(data, ip_addr(snapshot.remote_addr))
                    .map_err(|_| syscall::EAGAIN)?;
                Ok(data.len())
            }
        }
    }

    pub fn recv_socket_data(
        &mut self,
        snapshot: &SocketSnapshot,
        out: &mut [u8],
        peek: bool,
    ) -> Result<(usize, u32, u16), i64> {
        if out.is_empty() || snapshot.shutdown_rd {
            return Ok((0, snapshot.remote_addr, snapshot.remote_port));
        }
        let Some(slot) = self.socket_slot_by_exo_handle(snapshot.handle) else {
            return Err(syscall::EBADF);
        };
        let Some(handle) = self.socket_handles[slot] else {
            return Err(syscall::EBADF);
        };

        let mut sockets = socket_set();
        match snapshot.kind {
            SocketKind::Udp => {
                let socket = sockets.get_mut::<udp::Socket>(handle);
                if peek {
                    let (payload, meta) = socket.peek().map_err(|_| syscall::EAGAIN)?;
                    let n = payload.len().min(out.len());
                    out[..n].copy_from_slice(&payload[..n]);
                    let (addr, port) = endpoint_to_v4(meta.endpoint)
                        .unwrap_or((snapshot.remote_addr, snapshot.remote_port));
                    Ok((n, addr, port))
                } else {
                    let (n, meta) = socket.recv_slice(out).map_err(|_| syscall::EAGAIN)?;
                    let (addr, port) = endpoint_to_v4(meta.endpoint)
                        .unwrap_or((snapshot.remote_addr, snapshot.remote_port));
                    Ok((n, addr, port))
                }
            }
            SocketKind::Tcp => {
                let socket = sockets.get_mut::<tcp::Socket>(handle);
                if !socket.can_recv() {
                    if !socket.may_recv() {
                        // Peer closed / FIN received -> clean TCP EOF (0 bytes).
                        return Ok((0, snapshot.remote_addr, snapshot.remote_port));
                    }
                    return Err(syscall::EAGAIN);
                }
                let res = if peek {
                    socket.peek_slice(out)
                } else {
                    socket.recv_slice(out)
                };
                match res {
                    Ok(n) => Ok((n, snapshot.remote_addr, snapshot.remote_port)),
                    Err(tcp::RecvError::Finished) => {
                        Ok((0, snapshot.remote_addr, snapshot.remote_port))
                    }
                    Err(tcp::RecvError::InvalidState) => {
                        if matches!(
                            socket.state(),
                            tcp::State::Closed
                                | tcp::State::TimeWait
                                | tcp::State::CloseWait
                                | tcp::State::LastAck
                                | tcp::State::Closing
                        ) {
                            Ok((0, snapshot.remote_addr, snapshot.remote_port))
                        } else {
                            Err(syscall::ENOTCONN)
                        }
                    }
                }
            }
            SocketKind::Raw => {
                let socket = sockets.get_mut::<icmp::Socket>(handle);
                let (n, addr) = socket.recv_slice(out).map_err(|_| syscall::EAGAIN)?;
                Ok((n, ip_address_to_v4(addr).unwrap_or(snapshot.remote_addr), 0))
            }
        }
    }

    pub const fn ip(&self) -> u32 {
        self.ip
    }

    pub const fn mac(&self) -> [u8; 6] {
        self.mac
    }

    pub fn set_mac(&mut self, mac: [u8; 6]) {
        if mac == [0; 6] || mac == self.mac {
            return;
        }
        self.mac = mac;
        if let Some(iface) = self.iface.as_mut() {
            iface.set_hardware_addr(EthernetAddress(mac).into());
        }
    }

    pub const fn prefix_len(&self) -> u8 {
        self.prefix_len
    }

    pub fn tcp_connect_status(&self, snapshot: &SocketSnapshot) -> TcpConnectStatus {
        let Some(slot) = self.socket_slot_by_exo_handle(snapshot.handle) else {
            return TcpConnectStatus::Failed;
        };
        let Some(handle) = self.socket_handles[slot] else {
            return TcpConnectStatus::Failed;
        };
        let sockets = socket_set();
        let socket = sockets.get::<tcp::Socket>(handle);
        match socket.state() {
            tcp::State::Established | tcp::State::CloseWait => TcpConnectStatus::Established,
            tcp::State::Closed | tcp::State::TimeWait => TcpConnectStatus::Failed,
            _ => TcpConnectStatus::Pending,
        }
    }

    fn ensure_lo_iface(&mut self, now: Instant) {
        if self.lo_iface.is_some() {
            return;
        }
        let mut lo_dev = LoopbackIpDevice;
        let mut config = Config::new(HardwareAddress::Ip);
        config.random_seed = 0x7f00_0001_4558_4f4e;
        let mut iface = Interface::new(config, &mut lo_dev, now);
        iface.update_ip_addrs(|addrs| {
            addrs.clear();
            let _ = addrs.push(IpCidr::new(IpAddress::v4(127, 0, 0, 1), 8));
        });
        self.lo_iface = Some(iface);
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
        let gw = if self.gateway != 0 {
            self.gateway
        } else {
            0x0a00_0202
        };
        let _ = iface
            .routes_mut()
            .add_default_ipv4_route(ipv4_from_u32(gw));
        self.iface = Some(iface);
    }

    fn ip_cidr(&self) -> IpCidr {
        IpCidr::new(ip_addr(self.ip), self.prefix_len)
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
                if socket.state() == tcp::State::Closed {
                    let _ = socket.listen(IpListenEndpoint {
                        addr: ip_listen_addr(snapshot.local_addr),
                        port: snapshot.local_port,
                    });
                }
            }
            SocketState::Connecting | SocketState::Connected
                if snapshot.remote_port != 0 && snapshot.local_port != 0 =>
            {
                if socket.state() == tcp::State::Closed {
                    let use_loopback = is_loopback_ipv4(snapshot.remote_addr);
                    let target_iface = if use_loopback {
                        self.lo_iface.as_mut()
                    } else {
                        self.iface.as_mut()
                    };
                    if let Some(iface) = target_iface {
                        let local_ep = if use_loopback {
                            IpListenEndpoint {
                                addr: Some(IpAddress::v4(127, 0, 0, 1)),
                                port: snapshot.local_port,
                            }
                        } else {
                            IpListenEndpoint {
                                addr: ip_listen_addr(snapshot.local_addr),
                                port: snapshot.local_port,
                            }
                        };
                        let _ = socket.connect(
                            iface.context(),
                            IpEndpoint::new(ip_addr(snapshot.remote_addr), snapshot.remote_port),
                            local_ep,
                        );
                    }
                }
            }
            SocketState::Shutdown => socket.close(),
            SocketState::Closed => socket.abort(),
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

    fn apply_icmp_state(
        &mut self,
        _handle: SocketHandle,
        _sockets: &mut SocketSet<'static>,
        _snapshot: &SocketSnapshot,
    ) {
        // Le bind ICMP est effectué dans send_socket_data avec l'identifiant exact de l'en-tête ICMP.
    }
}

fn ipv4_from_u32(ip: u32) -> Ipv4Address {
    Ipv4Address::new(
        ((ip >> 24) & 0xff) as u8,
        ((ip >> 16) & 0xff) as u8,
        ((ip >> 8) & 0xff) as u8,
        (ip & 0xff) as u8,
    )
}

fn ip_addr(ip: u32) -> IpAddress {
    IpAddress::Ipv4(ipv4_from_u32(ip))
}

fn ip_listen_addr(ip: u32) -> Option<IpAddress> {
    (ip != 0).then_some(ip_addr(ip))
}

fn endpoint_to_v4(endpoint: IpEndpoint) -> Option<(u32, u16)> {
    ip_address_to_v4(endpoint.addr).map(|addr| (addr, endpoint.port))
}

fn ip_address_to_v4(addr: IpAddress) -> Option<u32> {
    match addr {
        IpAddress::Ipv4(addr) => {
            let octets = addr.octets();
            Some(
                ((octets[0] as u32) << 24)
                    | ((octets[1] as u32) << 16)
                    | ((octets[2] as u32) << 8)
                    | octets[3] as u32,
            )
        }
        #[allow(unreachable_patterns)]
        _ => None,
    }
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
        let rx_meta_slots = &mut (*UDP_BUFFERS.rx_meta.get())[slot];
        let tx_meta_slots = &mut (*UDP_BUFFERS.tx_meta.get())[slot];
        let mut idx = 0usize;
        while idx < UDP_PACKET_METADATA_LEN {
            rx_meta_slots[idx].write(udp::PacketMetadata::EMPTY);
            tx_meta_slots[idx].write(udp::PacketMetadata::EMPTY);
            idx += 1;
        }
        let rx_meta: &'static mut [udp::PacketMetadata; UDP_PACKET_METADATA_LEN] =
            &mut *(rx_meta_slots.as_mut_ptr()
                as *mut [udp::PacketMetadata; UDP_PACKET_METADATA_LEN]);
        let tx_meta: &'static mut [udp::PacketMetadata; UDP_PACKET_METADATA_LEN] =
            &mut *(tx_meta_slots.as_mut_ptr()
                as *mut [udp::PacketMetadata; UDP_PACKET_METADATA_LEN]);
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

fn make_icmp_socket(slot: usize) -> icmp::Socket<'static> {
    // SAFETY: chaque slot est alloué une seule fois tant que le socket est actif.
    unsafe {
        let rx_meta: &'static mut [icmp::PacketMetadata; ICMP_PACKET_METADATA_LEN] =
            &mut (*ICMP_BUFFERS.rx_meta.get())[slot];
        let tx_meta: &'static mut [icmp::PacketMetadata; ICMP_PACKET_METADATA_LEN] =
            &mut (*ICMP_BUFFERS.tx_meta.get())[slot];
        let rx_payload: &'static mut [u8; ICMP_BUFFER_SIZE] =
            &mut (*ICMP_BUFFERS.rx_payload.get())[slot];
        let tx_payload: &'static mut [u8; ICMP_BUFFER_SIZE] =
            &mut (*ICMP_BUFFERS.tx_payload.get())[slot];
        icmp::Socket::new(
            icmp::PacketBuffer::new(&mut rx_meta[..], &mut rx_payload[..]),
            icmp::PacketBuffer::new(&mut tx_meta[..], &mut tx_payload[..]),
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
        log_first_frame(
            &LOGGED_STACK_RX,
            b"network_server: first stack rx ",
            payload,
        );
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
            if payload.len() >= 14 {
                let ethertype = u16::from_be_bytes([payload[12], payload[13]]);
                if ethertype == 0x0806 {
                    LAST_EGRESS_WAS_ARP.store(true, core::sync::atomic::Ordering::Relaxed);
                } else if ethertype == 0x0800 {
                    LAST_EGRESS_SENT_IPV4.store(true, core::sync::atomic::Ordering::Relaxed);
                }
            }
            log_first_frame(
                &LOGGED_STACK_TX,
                b"network_server: first stack tx ",
                payload,
            );
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

fn log_first_frame(flag: &AtomicBool, prefix: &[u8], frame: &[u8]) {
    let _ = (flag, prefix, frame);
}

#[allow(dead_code)]
fn debug_write(bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    unsafe {
        let _ = syscall::syscall3(
            syscall::SYS_EXO_LOG,
            bytes.as_ptr() as u64,
            bytes.len() as u64,
            1,
        );
    }
}
