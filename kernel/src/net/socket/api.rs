//! # High-Level Socket API
//! 
//! Production-grade BSD-compatible socket API with:
//! - Zero-copy operations
//! - io_uring integration
//! - POSIX compliance
//! - Type-safe wrappers

use super::*;
use crate::net::{IpAddress, NetError};
use alloc::boxed::Box;
use core::time::Duration;

/// Socket wrapper with high-level API
pub struct Socket {
    fd: i32,
    domain: i32,
    socket_type: i32,
    protocol: i32,
}

impl Socket {
    /// Create TCP socket
    pub fn tcp() -> Result<Self, NetError> {
        Self::new(AF_INET, SOCK_STREAM, 0)
    }
    
    /// Create UDP socket
    pub fn udp() -> Result<Self, NetError> {
        Self::new(AF_INET, SOCK_DGRAM, 0)
    }
    
    /// Create raw socket
    pub fn raw(protocol: i32) -> Result<Self, NetError> {
        Self::new(AF_INET, SOCK_RAW, protocol)
    }
    
    /// Create socket
    pub fn new(domain: i32, socket_type: i32, protocol: i32) -> Result<Self, NetError> {
        let fd = super::socket(domain, socket_type, protocol)?;
        Ok(Self {
            fd,
            domain,
            socket_type,
            protocol,
        })
    }
    
    /// Bind to address
    pub fn bind(&self, addr: &str) -> Result<(), NetError> {
        super::bind(self.fd, addr)
    }
    
    /// Connect to address
    pub fn connect(&self, addr: &str) -> Result<(), NetError> {
        super::connect(self.fd, addr)
    }
    
    /// Listen for connections
    pub fn listen(&self, backlog: i32) -> Result<(), NetError> {
        super::listen(self.fd, backlog)
    }
    
    /// Accept connection
    pub fn accept(&self) -> Result<(Socket, SocketAddr), NetError> {
        let (fd, addr) = super::accept(self.fd)?;
        let socket = Socket {
            fd,
            domain: self.domain,
            socket_type: self.socket_type,
            protocol: self.protocol,
        };
        Ok((socket, addr))
    }
    
    /// Send data
    pub fn send(&self, data: &[u8]) -> Result<usize, NetError> {
        super::send(self.fd, data, 0)
    }
    
    /// Send with flags
    pub fn send_with_flags(&self, data: &[u8], flags: i32) -> Result<usize, NetError> {
        super::send(self.fd, data, flags)
    }
    
    /// Receive data
    pub fn recv(&self, buffer: &mut [u8]) -> Result<usize, NetError> {
        super::recv(self.fd, buffer, 0)
    }
    
    /// Receive with flags
    pub fn recv_with_flags(&self, buffer: &mut [u8], flags: i32) -> Result<usize, NetError> {
        super::recv(self.fd, buffer, flags)
    }
    
    /// Send to specific address
    pub fn sendto(&self, data: &[u8], addr: &str) -> Result<usize, NetError> {
        super::sendto(self.fd, data, addr)
    }
    
    /// Receive from
    pub fn recvfrom(&self, buffer: &mut [u8]) -> Result<(usize, SocketAddr), NetError> {
        super::recvfrom(self.fd, buffer)
    }
    
    /// Set socket option
    pub fn setsockopt(&self, level: i32, optname: i32, optval: &[u8]) -> Result<(), NetError> {
        super::setsockopt(self.fd, level, optname, optval)
    }
    
    /// Get socket option
    pub fn getsockopt(&self, level: i32, optname: i32, optval: &mut [u8]) -> Result<usize, NetError> {
        super::getsockopt(self.fd, level, optname, optval)
    }
    
    /// Set non-blocking mode
    pub fn set_nonblocking(&self, nonblocking: bool) -> Result<(), NetError> {
        super::set_nonblocking(self.fd, nonblocking)
    }
    
    /// Set read timeout
    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> Result<(), NetError> {
        super::set_read_timeout(self.fd, timeout)
    }
    
    /// Set write timeout
    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> Result<(), NetError> {
        super::set_write_timeout(self.fd, timeout)
    }
    
    /// Shutdown socket
    pub fn shutdown(&self, how: ShutdownHow) -> Result<(), NetError> {
        super::shutdown(self.fd, how)
    }
    
    /// Close socket
    pub fn close(self) -> Result<(), NetError> {
        super::close(self.fd)
    }
    
    /// Get file descriptor
    pub fn fd(&self) -> i32 {
        self.fd
    }
    
    /// Get local address
    pub fn local_addr(&self) -> Result<SocketAddr, NetError> {
        super::getsockname(self.fd)
    }
    
    /// Get peer address
    pub fn peer_addr(&self) -> Result<SocketAddr, NetError> {
        super::getpeername(self.fd)
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        let _ = super::close(self.fd);
    }
}

/// Shutdown options
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownHow {
    Read = 0,
    Write = 1,
    Both = 2,
}

/// Socket address
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketAddr {
    V4 {
        addr: [u8; 4],
        port: u16,
    },
    V6 {
        addr: [u8; 16],
        port: u16,
        flowinfo: u32,
        scope_id: u32,
    },
}

impl SocketAddr {
    pub fn v4(addr: [u8; 4], port: u16) -> Self {
        SocketAddr::V4 { addr, port }
    }
    
    pub fn v6(addr: [u8; 16], port: u16, flowinfo: u32, scope_id: u32) -> Self {
        SocketAddr::V6 {
            addr,
            port,
            flowinfo,
            scope_id,
        }
    }
    
    pub fn parse(s: &str) -> Result<Self, NetError> {
        // Parse "IP:port" format
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 2 {
            return Err(NetError::InvalidAddress);
        }
        
        let port: u16 = parts[1].parse().map_err(|_| NetError::InvalidAddress)?;
        
        // Try IPv4
        let ip_parts: Vec<&str> = parts[0].split('.').collect();
        if ip_parts.len() == 4 {
            let mut addr = [0u8; 4];
            for (i, part) in ip_parts.iter().enumerate() {
                addr[i] = part.parse().map_err(|_| NetError::InvalidAddress)?;
            }
            return Ok(SocketAddr::v4(addr, port));
        }
        
        // TODO: IPv6 parsing
        Err(NetError::InvalidAddress)
    }
}

/// Socket constants
pub const AF_INET: i32 = 2;
pub const AF_INET6: i32 = 10;
pub const AF_UNIX: i32 = 1;

pub const SOCK_STREAM: i32 = 1;
pub const SOCK_DGRAM: i32 = 2;
pub const SOCK_RAW: i32 = 3;

pub const IPPROTO_TCP: i32 = 6;
pub const IPPROTO_UDP: i32 = 17;
pub const IPPROTO_ICMP: i32 = 1;

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_socket_addr_parse() {
        let addr = SocketAddr::parse("192.168.1.1:8080").unwrap();
        match addr {
            SocketAddr::V4 { addr, port } => {
                assert_eq!(addr, [192, 168, 1, 1]);
                assert_eq!(port, 8080);
            }
            _ => panic!("Expected IPv4"),
        }
    }
}
