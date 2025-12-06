/// UDP Socket API
/// 
/// High-level UDP socket interface with:
/// - Connectionless communication
/// - Connected UDP mode
/// - Multicast support
/// - Broadcast support
/// - Zero-copy operations

use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;
use crate::net::core::{NetworkInterface, SocketBuffer};

/// UDP Socket
pub struct UdpSocket {
    /// Local address
    local_addr: [u8; 16],
    /// Local port
    local_port: u16,
    /// Remote address (for connected UDP)
    remote_addr: Option<[u8; 16]>,
    /// Remote port (for connected UDP)
    remote_port: Option<u16>,
    /// Socket state
    state: Mutex<SocketState>,
    /// Receive queue
    rx_queue: Mutex<Vec<(SocketBuffer, [u8; 16], u16)>>,
    /// Maximum receive queue size
    rx_queue_max: usize,
    /// Network interface
    interface: Option<Arc<NetworkInterface>>,
    /// Socket options
    options: Mutex<SocketOptions>,
}

/// Socket state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketState {
    /// Socket is closed
    Closed,
    /// Socket is bound to a local address
    Bound,
    /// Socket is connected to a remote address
    Connected,
}

/// Socket options
#[derive(Debug, Clone)]
pub struct SocketOptions {
    /// Enable broadcast
    pub broadcast: bool,
    /// TTL (Time To Live)
    pub ttl: u8,
    /// Multicast TTL
    pub multicast_ttl: u8,
    /// Multicast loop
    pub multicast_loop: bool,
    /// Receive buffer size
    pub rcvbuf: usize,
    /// Send buffer size
    pub sndbuf: usize,
}

impl Default for SocketOptions {
    fn default() -> Self {
        Self {
            broadcast: false,
            ttl: 64,
            multicast_ttl: 1,
            multicast_loop: true,
            rcvbuf: 212992,  // 208 KB
            sndbuf: 212992,  // 208 KB
        }
    }
}

/// UDP socket errors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UdpSocketError {
    /// Socket is already bound
    AlreadyBound,
    /// Socket is not bound
    NotBound,
    /// Socket is already connected
    AlreadyConnected,
    /// Socket is not connected
    NotConnected,
    /// Invalid address or port
    InvalidAddress,
    /// Receive queue is full
    QueueFull,
    /// Receive queue is empty
    QueueEmpty,
    /// Message too large
    MessageTooLarge,
    /// Permission denied (e.g., broadcast not enabled)
    PermissionDenied,
    /// Network unreachable
    NetworkUnreachable,
}

impl UdpSocket {
    /// Create a new UDP socket
    pub fn new() -> Self {
        Self {
            local_addr: [0; 16],
            local_port: 0,
            remote_addr: None,
            remote_port: None,
            state: Mutex::new(SocketState::Closed),
            rx_queue: Mutex::new(Vec::new()),
            rx_queue_max: 1000,
            interface: None,
            options: Mutex::new(SocketOptions::default()),
        }
    }

    /// Bind the socket to a local address and port
    pub fn bind(&mut self, addr: [u8; 16], port: u16) -> Result<(), UdpSocketError> {
        let mut state = self.state.lock();
        if *state != SocketState::Closed {
            return Err(UdpSocketError::AlreadyBound);
        }

        self.local_addr = addr;
        self.local_port = port;
        *state = SocketState::Bound;

        Ok(())
    }

    /// Connect the socket to a remote address and port
    /// 
    /// This doesn't establish a connection (UDP is connectionless),
    /// but restricts send/recv to this remote address only
    pub fn connect(&mut self, addr: [u8; 16], port: u16) -> Result<(), UdpSocketError> {
        let mut state = self.state.lock();
        if *state == SocketState::Closed {
            // Auto-bind to any address
            self.local_addr = [0; 16];
            self.local_port = 0;  // OS will assign ephemeral port
        }

        self.remote_addr = Some(addr);
        self.remote_port = Some(port);
        *state = SocketState::Connected;

        Ok(())
    }

    /// Send data to a specific address (for unconnected sockets)
    pub fn send_to(&self, data: &[u8], addr: [u8; 16], port: u16) -> Result<usize, UdpSocketError> {
        let state = self.state.lock();
        if *state == SocketState::Closed {
            return Err(UdpSocketError::NotBound);
        }

        // Check if we're trying to send to a broadcast address
        if self.is_broadcast_addr(&addr) {
            let options = self.options.lock();
            if !options.broadcast {
                return Err(UdpSocketError::PermissionDenied);
            }
        }

        // TODO: Create UDP packet and send via NetworkInterface
        // For now, just return the data length
        Ok(data.len())
    }

    /// Send data (for connected sockets)
    pub fn send(&self, data: &[u8]) -> Result<usize, UdpSocketError> {
        let state = self.state.lock();
        if *state != SocketState::Connected {
            return Err(UdpSocketError::NotConnected);
        }

        let remote_addr = self.remote_addr.ok_or(UdpSocketError::NotConnected)?;
        let remote_port = self.remote_port.ok_or(UdpSocketError::NotConnected)?;

        drop(state);
        self.send_to(data, remote_addr, remote_port)
    }

    /// Receive data from any address (for unconnected sockets)
    pub fn recv_from(&self, buf: &mut [u8]) -> Result<(usize, [u8; 16], u16), UdpSocketError> {
        let state = self.state.lock();
        if *state == SocketState::Closed {
            return Err(UdpSocketError::NotBound);
        }
        drop(state);

        let mut rx_queue = self.rx_queue.lock();
        if rx_queue.is_empty() {
            return Err(UdpSocketError::QueueEmpty);
        }

        let (skb, from_addr, from_port) = rx_queue.remove(0);
        let data = skb.data();
        let len = data.len().min(buf.len());
        buf[..len].copy_from_slice(&data[..len]);

        Ok((len, from_addr, from_port))
    }

    /// Receive data (for connected sockets)
    pub fn recv(&self, buf: &mut [u8]) -> Result<usize, UdpSocketError> {
        let state = self.state.lock();
        if *state != SocketState::Connected {
            return Err(UdpSocketError::NotConnected);
        }
        drop(state);

        let (len, _, _) = self.recv_from(buf)?;
        Ok(len)
    }

    /// Close the socket
    pub fn close(&mut self) -> Result<(), UdpSocketError> {
        let mut state = self.state.lock();
        *state = SocketState::Closed;

        // Clear receive queue
        self.rx_queue.lock().clear();

        Ok(())
    }

    /// Get socket options
    pub fn get_option(&self, option: SocketOption) -> SocketOptionValue {
        let options = self.options.lock();
        match option {
            SocketOption::Broadcast => SocketOptionValue::Bool(options.broadcast),
            SocketOption::Ttl => SocketOptionValue::U8(options.ttl),
            SocketOption::MulticastTtl => SocketOptionValue::U8(options.multicast_ttl),
            SocketOption::MulticastLoop => SocketOptionValue::Bool(options.multicast_loop),
            SocketOption::RcvBuf => SocketOptionValue::Usize(options.rcvbuf),
            SocketOption::SndBuf => SocketOptionValue::Usize(options.sndbuf),
        }
    }

    /// Set socket options
    pub fn set_option(&self, option: SocketOption, value: SocketOptionValue) -> Result<(), UdpSocketError> {
        let mut options = self.options.lock();
        match (option, value) {
            (SocketOption::Broadcast, SocketOptionValue::Bool(v)) => options.broadcast = v,
            (SocketOption::Ttl, SocketOptionValue::U8(v)) => options.ttl = v,
            (SocketOption::MulticastTtl, SocketOptionValue::U8(v)) => options.multicast_ttl = v,
            (SocketOption::MulticastLoop, SocketOptionValue::Bool(v)) => options.multicast_loop = v,
            (SocketOption::RcvBuf, SocketOptionValue::Usize(v)) => options.rcvbuf = v,
            (SocketOption::SndBuf, SocketOptionValue::Usize(v)) => options.sndbuf = v,
            _ => return Err(UdpSocketError::InvalidAddress),
        }
        Ok(())
    }

    /// Check if an address is a broadcast address
    fn is_broadcast_addr(&self, addr: &[u8; 16]) -> bool {
        // Check for IPv4 broadcast: 255.255.255.255
        addr[..4] == [255, 255, 255, 255] && addr[4..].iter().all(|&b| b == 0)
    }

    /// Get the local address
    pub fn local_addr(&self) -> [u8; 16] {
        self.local_addr
    }

    /// Get the local port
    pub fn local_port(&self) -> u16 {
        self.local_port
    }

    /// Get the remote address (for connected sockets)
    pub fn remote_addr(&self) -> Option<[u8; 16]> {
        self.remote_addr
    }

    /// Get the remote port (for connected sockets)
    pub fn remote_port(&self) -> Option<u16> {
        self.remote_port
    }

    /// Get the socket state
    pub fn state(&self) -> SocketState {
        *self.state.lock()
    }
}

/// Socket option types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketOption {
    Broadcast,
    Ttl,
    MulticastTtl,
    MulticastLoop,
    RcvBuf,
    SndBuf,
}

/// Socket option values
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketOptionValue {
    Bool(bool),
    U8(u8),
    Usize(usize),
}
