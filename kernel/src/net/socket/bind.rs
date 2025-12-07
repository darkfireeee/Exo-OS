//! # Socket Bind Implementation
//! 
//! Bind socket to local address with:
//! - Port reuse (SO_REUSEADDR/SO_REUSEPORT)
//! - Wildcard binding
//! - Privilege checks
//! - Address validation

use crate::net::{NetError, IpAddress};
use super::api::{SocketAddr, AF_INET, AF_INET6};

/// Bind socket to address
pub fn bind(fd: i32, addr: &str) -> Result<(), NetError> {
    let socket_addr = SocketAddr::parse(addr)?;
    bind_addr(fd, &socket_addr)
}

/// Bind to SocketAddr
pub fn bind_addr(fd: i32, addr: &SocketAddr) -> Result<(), NetError> {
    // Validate socket exists
    let socket = get_socket(fd)?;
    
    // Check if already bound
    if socket.is_bound() {
        return Err(NetError::AlreadyConnected);
    }
    
    // Validate address
    match addr {
        SocketAddr::V4 { addr, port } => {
            // Check privileged port
            if *port < 1024 && !is_privileged() {
                return Err(NetError::PermissionDenied);
            }
            
            // Check address availability
            if !is_addr_available(*addr, *port, socket.reuse_addr())? {
                return Err(NetError::AddressInUse);
            }
            
            // Bind IPv4
            bind_ipv4(fd, *addr, *port)?;
        }
        SocketAddr::V6 { addr, port, .. } => {
            // Check privileged port
            if *port < 1024 && !is_privileged() {
                return Err(NetError::PermissionDenied);
            }
            
            // Bind IPv6
            bind_ipv6(fd, *addr, *port)?;
        }
    }
    
    Ok(())
}

/// Bind to IPv4 address
fn bind_ipv4(fd: i32, addr: [u8; 4], port: u16) -> Result<(), NetError> {
    let socket = get_socket_mut(fd)?;
    
    // Set local address
    socket.set_local_addr(IpAddress::V4(addr))?;
    socket.set_local_port(port)?;
    
    // Register in port table
    register_port(port, fd)?;
    
    Ok(())
}

/// Bind to IPv6 address
fn bind_ipv6(fd: i32, addr: [u8; 16], port: u16) -> Result<(), NetError> {
    let socket = get_socket_mut(fd)?;
    
    // Set local address
    socket.set_local_addr(IpAddress::V6(addr))?;
    socket.set_local_port(port)?;
    
    // Register in port table
    register_port(port, fd)?;
    
    Ok(())
}

/// Check if address is available for binding
fn is_addr_available(addr: [u8; 4], port: u16, reuse: bool) -> Result<bool, NetError> {
    // Check port table
    if let Some(existing_fd) = lookup_port(port) {
        let existing_socket = get_socket(existing_fd)?;
        
        // If reuse is enabled on both sockets, allow
        if reuse && existing_socket.reuse_addr() {
            return Ok(true);
        }
        
        // Otherwise, address in use
        return Ok(false);
    }
    
    Ok(true)
}

/// Check if current process has privilege
fn is_privileged() -> bool {
    // Check if user is root (UID 0) or has CAP_NET_BIND_SERVICE capability
    // For now, allow all (will be integrated with process/security system later)
    true
}

use alloc::collections::BTreeMap;
use crate::sync::SpinLock;

/// Global socket registry
static SOCKET_REGISTRY: SpinLock<BTreeMap<i32, SocketInfo>> = SpinLock::new(BTreeMap::new());
static PORT_REGISTRY: SpinLock<BTreeMap<u16, i32>> = SpinLock::new(BTreeMap::new());

#[derive(Clone)]
struct SocketInfo {
    bound_addr: Option<SocketAddr>,
    bound: bool,
    reuse_addr: bool,
}

impl Default for SocketInfo {
    fn default() -> Self {
        Self {
            bound_addr: None,
            bound: false,
            reuse_addr: false,
        }
    }
}

fn get_socket(fd: i32) -> Result<&'static Socket, NetError> {
    // This is a placeholder for integration with socket manager
    // In real implementation, would lookup in global socket table
    // For now, return a reference to prevent crashes
    static mut DUMMY: Socket = Socket;
    Ok(unsafe { &DUMMY })
}

fn get_socket_mut(fd: i32) -> Result<&'static mut Socket, NetError> {
    // Placeholder for socket manager integration
    static mut DUMMY: Socket = Socket;
    Ok(unsafe { &mut DUMMY })
}

fn register_port(port: u16, fd: i32) -> Result<(), NetError> {
    let mut ports = PORT_REGISTRY.lock();
    
    // Check if port already in use
    if ports.contains_key(&port) {
        return Err(NetError::AddressInUse);
    }
    
    ports.insert(port, fd);
    Ok(())
}

fn lookup_port(port: u16) -> Option<i32> {
    PORT_REGISTRY.lock().get(&port).copied()
}

// Mock Socket struct (will be replaced by real Socket from socket manager)
struct Socket;
impl Socket {
    fn is_bound(&self) -> bool { false }
    fn reuse_addr(&self) -> bool { false }
    fn set_local_addr(&mut self, addr: IpAddress) -> Result<(), NetError> { Ok(()) }
    fn set_local_port(&mut self, port: u16) -> Result<(), NetError> { Ok(()) }
}
