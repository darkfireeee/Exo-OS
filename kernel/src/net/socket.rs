//! Network Core - Socket abstraction and buffer management
//!
//! Phase 2 - Network Stack Core:
//! - Socket abstraction (BSD-like API)
//! - Packet buffers (sk_buff equivalent)
//! - Network device interface

use alloc::vec::Vec;
use alloc::boxed::Box;
use core::sync::atomic::{AtomicU32, Ordering};
use spin::Mutex;

/// Maximum packet size (MTU)
pub const MAX_PACKET_SIZE: usize = 1500;

/// Socket types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketType {
    /// Stream socket (TCP)
    Stream,
    /// Datagram socket (UDP)
    Datagram,
    /// Raw socket (IP)
    Raw,
}

/// Socket domain
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketDomain {
    /// IPv4
    Inet,
    /// IPv6
    Inet6,
    /// Unix domain
    Unix,
}

/// Socket state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketState {
    Closed,
    Listening,
    Connecting,
    Connected,
    Disconnecting,
}

/// Socket abstraction
pub struct Socket {
    /// Socket ID
    id: u32,
    
    /// Socket type
    socket_type: SocketType,
    
    /// Socket domain
    domain: SocketDomain,
    
    /// Current state
    state: SocketState,
    
    /// Local address
    local_addr: Option<SocketAddr>,
    
    /// Remote address
    remote_addr: Option<SocketAddr>,
    
    /// Receive buffer
    recv_buffer: Mutex<Vec<u8>>,
    
    /// Send buffer
    send_buffer: Mutex<Vec<u8>>,
    
    /// Socket options
    options: SocketOptions,
}

/// Socket address
#[derive(Debug, Clone, Copy)]
pub struct SocketAddr {
    pub ip: IpAddr,
    pub port: u16,
}

/// IP address
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpAddr {
    V4(Ipv4Addr),
    V6(Ipv6Addr),
}

/// IPv4 address
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ipv4Addr(pub [u8; 4]);

impl Ipv4Addr {
    pub const fn new(a: u8, b: u8, c: u8, d: u8) -> Self {
        Self([a, b, c, d])
    }
    
    pub const fn localhost() -> Self {
        Self([127, 0, 0, 1])
    }
    
    pub const fn any() -> Self {
        Self([0, 0, 0, 0])
    }
    
    pub fn to_u32(&self) -> u32 {
        u32::from_be_bytes(self.0)
    }
    
    pub fn from_u32(val: u32) -> Self {
        Self(val.to_be_bytes())
    }
}

/// IPv6 address
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ipv6Addr(pub [u8; 16]);

/// Socket options
#[derive(Debug, Clone, Copy)]
pub struct SocketOptions {
    /// Reuse address
    pub reuse_addr: bool,
    
    /// Keep alive
    pub keep_alive: bool,
    
    /// Receive timeout (milliseconds)
    pub recv_timeout: Option<u32>,
    
    /// Send timeout (milliseconds)
    pub send_timeout: Option<u32>,
    
    /// Receive buffer size
    pub recv_buffer_size: usize,
    
    /// Send buffer size
    pub send_buffer_size: usize,
}

impl Default for SocketOptions {
    fn default() -> Self {
        Self {
            reuse_addr: false,
            keep_alive: false,
            recv_timeout: None,
            send_timeout: None,
            recv_buffer_size: 65536,
            send_buffer_size: 65536,
        }
    }
}

impl Socket {
    /// Create new socket
    pub fn new(domain: SocketDomain, socket_type: SocketType) -> Self {
        static NEXT_ID: AtomicU32 = AtomicU32::new(1);
        
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            socket_type,
            domain,
            state: SocketState::Closed,
            local_addr: None,
            remote_addr: None,
            recv_buffer: Mutex::new(Vec::with_capacity(65536)),
            send_buffer: Mutex::new(Vec::with_capacity(65536)),
            options: SocketOptions::default(),
        }
    }
    
    /// Bind to local address
    pub fn bind(&mut self, addr: SocketAddr) -> Result<(), SocketError> {
        if self.state != SocketState::Closed {
            return Err(SocketError::AlreadyBound);
        }
        
        self.local_addr = Some(addr);
        Ok(())
    }
    
    /// Listen for connections (TCP only)
    pub fn listen(&mut self, backlog: usize) -> Result<(), SocketError> {
        if self.socket_type != SocketType::Stream {
            return Err(SocketError::InvalidOperation);
        }
        
        if self.local_addr.is_none() {
            return Err(SocketError::NotBound);
        }
        
        self.state = SocketState::Listening;
        Ok(())
    }
    
    /// Connect to remote address
    pub fn connect(&mut self, addr: SocketAddr) -> Result<(), SocketError> {
        self.remote_addr = Some(addr);
        self.state = SocketState::Connecting;
        
        // TODO: Actual connection logic
        
        self.state = SocketState::Connected;
        Ok(())
    }
    
    /// Send data
    pub fn send(&self, data: &[u8]) -> Result<usize, SocketError> {
        if self.state != SocketState::Connected {
            return Err(SocketError::NotConnected);
        }
        
        let mut buffer = self.send_buffer.lock();
        buffer.extend_from_slice(data);
        
        // TODO: Actual send logic
        
        Ok(data.len())
    }
    
    /// Receive data
    pub fn recv(&self, buffer: &mut [u8]) -> Result<usize, SocketError> {
        if self.state != SocketState::Connected {
            return Err(SocketError::NotConnected);
        }
        
        let mut recv_buf = self.recv_buffer.lock();
        let len = recv_buf.len().min(buffer.len());
        
        buffer[..len].copy_from_slice(&recv_buf[..len]);
        recv_buf.drain(..len);
        
        Ok(len)
    }
    
    /// Get socket state
    pub fn state(&self) -> SocketState {
        self.state
    }
    
    /// Get local address
    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.local_addr
    }
    
    /// Get remote address
    pub fn remote_addr(&self) -> Option<SocketAddr> {
        self.remote_addr
    }
}

/// Socket errors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketError {
    /// Socket already bound
    AlreadyBound,
    
    /// Socket not bound
    NotBound,
    
    /// Socket not connected
    NotConnected,
    
    /// Invalid operation for socket type
    InvalidOperation,
    
    /// Connection refused
    ConnectionRefused,
    
    /// Connection reset
    ConnectionReset,
    
    /// Timeout
    Timeout,
    
    /// Would block
    WouldBlock,
}

/// Socket table (global registry)
pub struct SocketTable {
    sockets: Mutex<Vec<Box<Socket>>>,
}

impl SocketTable {
    pub const fn new() -> Self {
        Self {
            sockets: Mutex::new(Vec::new()),
        }
    }
    
    /// Create and register new socket
    pub fn create(&self, domain: SocketDomain, socket_type: SocketType) -> u32 {
        let socket = Box::new(Socket::new(domain, socket_type));
        let id = socket.id;
        
        let mut sockets = self.sockets.lock();
        sockets.push(socket);
        
        id
    }
    
    /// Get socket by ID
    pub fn get(&self, id: u32) -> Option<Box<Socket>> {
        let mut sockets = self.sockets.lock();
        
        if let Some(pos) = sockets.iter().position(|s| s.id == id) {
            Some(sockets.remove(pos))
        } else {
            None
        }
    }
    
    /// Close and remove socket
    pub fn close(&self, id: u32) -> Result<(), SocketError> {
        let mut sockets = self.sockets.lock();
        
        if let Some(pos) = sockets.iter().position(|s| s.id == id) {
            sockets.remove(pos);
            Ok(())
        } else {
            Err(SocketError::NotConnected)
        }
    }
}

/// Global socket table
pub static SOCKET_TABLE: SocketTable = SocketTable::new();

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_socket_creation() {
        let socket = Socket::new(SocketDomain::Inet, SocketType::Stream);
        assert_eq!(socket.state(), SocketState::Closed);
    }
    
    #[test]
    fn test_ipv4_addr() {
        let localhost = Ipv4Addr::localhost();
        assert_eq!(localhost.0, [127, 0, 0, 1]);
        
        let any = Ipv4Addr::any();
        assert_eq!(any.0, [0, 0, 0, 0]);
    }
}
