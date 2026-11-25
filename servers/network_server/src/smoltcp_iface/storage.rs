use super::SOCKET_STORAGE_LEN;
use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use exo_syscall_abi as syscall;
use smoltcp::iface::{SocketSet, SocketStorage};
use smoltcp::socket::{icmp, tcp, udp};
use smoltcp::time::Instant;
use smoltcp::wire::{IpAddress, IpEndpoint, Ipv4Address};

const CLOCK_MONOTONIC: u64 = 1;
const TCP_BUFFER_SIZE: usize = 16384;
pub(super) const UDP_BUFFER_SIZE: usize = 16384;
const UDP_PACKET_METADATA_LEN: usize = 8;
const ICMP_BUFFER_SIZE: usize = 256;
const ICMP_PACKET_METADATA_LEN: usize = 4;

#[repr(C)]
#[derive(Default)]
struct Timespec {
    tv_sec: i64,
    tv_nsec: i64,
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


pub(super) fn socket_set() -> SocketSet<'static> {
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

pub(super) fn smoltcp_now_ms(fallback: u64) -> Instant {
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

pub(super) fn ipv4_from_u32(ip: u32) -> Ipv4Address {
    Ipv4Address::new(
        ((ip >> 24) & 0xff) as u8,
        ((ip >> 16) & 0xff) as u8,
        ((ip >> 8) & 0xff) as u8,
        (ip & 0xff) as u8,
    )
}

pub(super) fn ip_addr(ip: u32) -> IpAddress {
    IpAddress::Ipv4(ipv4_from_u32(ip))
}

pub(super) fn ip_listen_addr(ip: u32) -> Option<IpAddress> {
    (ip != 0).then_some(ip_addr(ip))
}

pub(super) fn endpoint_to_v4(endpoint: IpEndpoint) -> Option<(u32, u16)> {
    ip_address_to_v4(endpoint.addr).map(|addr| (addr, endpoint.port))
}

pub(super) fn ip_address_to_v4(addr: IpAddress) -> Option<u32> {
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

pub(super) fn make_tcp_socket(slot: usize) -> tcp::Socket<'static> {
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

pub(super) fn make_udp_socket(slot: usize) -> udp::Socket<'static> {
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

pub(super) fn make_icmp_socket(slot: usize) -> icmp::Socket<'static> {
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

