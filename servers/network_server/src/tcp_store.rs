use crate::socket_table::MAX_SOCKETS;
use core::cell::UnsafeCell;

pub const TCP_SOCKET_STATE_SIZE: usize = 6176;
pub const STORE_SERIALIZED_SIZE: usize = MAX_SOCKETS * TCP_SOCKET_STATE_SIZE;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TcpSocketState {
    pub local_addr: u32,
    pub local_port: u16,
    pub _pad0: u16,
    pub remote_addr: u32,
    pub remote_port: u16,
    pub state: u8,
    pub _pad1: u8,
    pub rx_len: u16,
    pub tx_len: u16,
    pub _pad2: u16,
    pub seq_num: u32,
    pub ack_num: u32,
    pub rx_buf: [u8; 2048],
    pub tx_buf: [u8; 4096],
}

impl TcpSocketState {
    pub const fn empty() -> Self {
        Self {
            local_addr: 0,
            local_port: 0,
            _pad0: 0,
            remote_addr: 0,
            remote_port: 0,
            state: 0,
            _pad1: 0,
            rx_len: 0,
            tx_len: 0,
            _pad2: 0,
            seq_num: 0,
            ack_num: 0,
            rx_buf: [0; 2048],
            tx_buf: [0; 4096],
        }
    }
}

const _: () = assert!(core::mem::size_of::<TcpSocketState>() == TCP_SOCKET_STATE_SIZE);

struct TcpStateSlots(UnsafeCell<[TcpSocketState; MAX_SOCKETS]>);

// SAFETY: network_server manipule l'état Phoenix sous son mutex global.
unsafe impl Sync for TcpStateSlots {}

static TCP_STATE_SLOTS: TcpStateSlots =
    TcpStateSlots(UnsafeCell::new([TcpSocketState::empty(); MAX_SOCKETS]));

pub struct TcpStateStore {
    occupied: [bool; MAX_SOCKETS],
    count: usize,
}

impl TcpStateStore {
    pub const fn new_empty() -> Self {
        Self {
            occupied: [false; MAX_SOCKETS],
            count: 0,
        }
    }

    pub fn clear(&mut self) {
        self.occupied = [false; MAX_SOCKETS];
        self.count = 0;
    }

    pub fn count(&self) -> usize {
        self.count
    }

    pub fn save(&mut self, slot: usize, state: TcpSocketState) -> bool {
        if slot >= MAX_SOCKETS {
            return false;
        }
        // SAFETY: le serveur réseau sérialise l'accès au store pendant le drain.
        unsafe {
            (*TCP_STATE_SLOTS.0.get())[slot] = state;
        }
        if !self.occupied[slot] {
            self.occupied[slot] = true;
            self.count = self.count.saturating_add(1);
        }
        true
    }

    pub fn load(&self, slot: usize) -> Option<TcpSocketState> {
        if slot >= MAX_SOCKETS || !self.occupied[slot] {
            return None;
        }
        // SAFETY: la lecture est sérialisée par le même mutex du service.
        Some(unsafe { (*TCP_STATE_SLOTS.0.get())[slot] })
    }
}
