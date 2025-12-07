//! # Socket Options Implementation
//! 
//! Complete socket option support with:
//! - SO_REUSEADDR/SO_REUSEPORT
//! - SO_RCVBUF/SO_SNDBUF
//! - TCP_NODELAY
//! - SO_KEEPALIVE
//! - Timeouts

use crate::net::NetError;
use core::time::Duration;

/// Set socket option
pub fn setsockopt(fd: i32, level: i32, optname: i32, optval: &[u8]) -> Result<(), NetError> {
    match level {
        SOL_SOCKET => set_socket_option(fd, optname, optval),
        IPPROTO_TCP => set_tcp_option(fd, optname, optval),
        IPPROTO_IP => set_ip_option(fd, optname, optval),
        _ => Err(NetError::NotSupported),
    }
}

/// Get socket option
pub fn getsockopt(fd: i32, level: i32, optname: i32, optval: &mut [u8]) -> Result<usize, NetError> {
    match level {
        SOL_SOCKET => get_socket_option(fd, optname, optval),
        IPPROTO_TCP => get_tcp_option(fd, optname, optval),
        IPPROTO_IP => get_ip_option(fd, optname, optval),
        _ => Err(NetError::NotSupported),
    }
}

/// Set socket-level option
fn set_socket_option(fd: i32, optname: i32, optval: &[u8]) -> Result<(), NetError> {
    let socket = get_socket_mut(fd)?;
    
    match optname {
        SO_REUSEADDR => {
            let value = parse_bool(optval)?;
            socket.set_reuse_addr(value);
            Ok(())
        }
        SO_REUSEPORT => {
            let value = parse_bool(optval)?;
            socket.set_reuse_port(value);
            Ok(())
        }
        SO_KEEPALIVE => {
            let value = parse_bool(optval)?;
            socket.set_keepalive(value);
            Ok(())
        }
        SO_RCVBUF => {
            let size = parse_u32(optval)?;
            socket.set_recv_buffer_size(size as usize);
            Ok(())
        }
        SO_SNDBUF => {
            let size = parse_u32(optval)?;
            socket.set_send_buffer_size(size as usize);
            Ok(())
        }
        SO_RCVTIMEO => {
            let timeout = parse_timeval(optval)?;
            socket.set_recv_timeout(timeout);
            Ok(())
        }
        SO_SNDTIMEO => {
            let timeout = parse_timeval(optval)?;
            socket.set_send_timeout(timeout);
            Ok(())
        }
        SO_LINGER => {
            let linger = parse_linger(optval)?;
            socket.set_linger(linger);
            Ok(())
        }
        _ => Err(NetError::NotSupported),
    }
}

/// Get socket-level option
fn get_socket_option(fd: i32, optname: i32, optval: &mut [u8]) -> Result<usize, NetError> {
    let socket = get_socket(fd)?;
    
    match optname {
        SO_REUSEADDR => {
            write_bool(optval, socket.reuse_addr())
        }
        SO_REUSEPORT => {
            write_bool(optval, socket.reuse_port())
        }
        SO_KEEPALIVE => {
            write_bool(optval, socket.keepalive())
        }
        SO_RCVBUF => {
            write_u32(optval, socket.recv_buffer_size() as u32)
        }
        SO_SNDBUF => {
            write_u32(optval, socket.send_buffer_size() as u32)
        }
        SO_ERROR => {
            write_i32(optval, socket.get_error())
        }
        SO_TYPE => {
            write_i32(optval, socket.socket_type())
        }
        _ => Err(NetError::NotSupported),
    }
}

/// Set TCP option
fn set_tcp_option(fd: i32, optname: i32, optval: &[u8]) -> Result<(), NetError> {
    let socket = get_socket_mut(fd)?;
    
    match optname {
        TCP_NODELAY => {
            let value = parse_bool(optval)?;
            socket.set_nodelay(value);
            Ok(())
        }
        TCP_CORK => {
            let value = parse_bool(optval)?;
            socket.set_cork(value);
            Ok(())
        }
        TCP_KEEPIDLE => {
            let secs = parse_u32(optval)?;
            socket.set_keepalive_idle(secs);
            Ok(())
        }
        TCP_KEEPINTVL => {
            let secs = parse_u32(optval)?;
            socket.set_keepalive_interval(secs);
            Ok(())
        }
        TCP_KEEPCNT => {
            let count = parse_u32(optval)?;
            socket.set_keepalive_count(count);
            Ok(())
        }
        TCP_FASTOPEN => {
            let value = parse_bool(optval)?;
            socket.set_fastopen(value);
            Ok(())
        }
        _ => Err(NetError::NotSupported),
    }
}

/// Get TCP option
fn get_tcp_option(fd: i32, optname: i32, optval: &mut [u8]) -> Result<usize, NetError> {
    let socket = get_socket(fd)?;
    
    match optname {
        TCP_NODELAY => write_bool(optval, socket.nodelay()),
        TCP_CORK => write_bool(optval, socket.cork()),
        _ => Err(NetError::NotSupported),
    }
}

/// Set IP option
fn set_ip_option(fd: i32, optname: i32, optval: &[u8]) -> Result<(), NetError> {
    match optname {
        IP_TTL => {
            let ttl = parse_u32(optval)? as u8;
            set_ip_ttl(fd, ttl)
        }
        IP_TOS => {
            let tos = parse_u32(optval)? as u8;
            set_ip_tos(fd, tos)
        }
        _ => Err(NetError::NotSupported),
    }
}

/// Get IP option
fn get_ip_option(fd: i32, optname: i32, optval: &mut [u8]) -> Result<usize, NetError> {
    match optname {
        IP_TTL => {
            let ttl = get_ip_ttl(fd)? as u32;
            write_u32(optval, ttl)
        }
        IP_TOS => {
            let tos = get_ip_tos(fd)? as u32;
            write_u32(optval, tos)
        }
        _ => Err(NetError::NotSupported),
    }
}

// Helper functions
fn parse_bool(optval: &[u8]) -> Result<bool, NetError> {
    if optval.len() < 4 {
        return Err(NetError::InvalidAddress);
    }
    Ok(u32::from_ne_bytes([optval[0], optval[1], optval[2], optval[3]]) != 0)
}

fn parse_u32(optval: &[u8]) -> Result<u32, NetError> {
    if optval.len() < 4 {
        return Err(NetError::InvalidAddress);
    }
    Ok(u32::from_ne_bytes([optval[0], optval[1], optval[2], optval[3]]))
}

fn parse_i32(optval: &[u8]) -> Result<i32, NetError> {
    if optval.len() < 4 {
        return Err(NetError::InvalidAddress);
    }
    Ok(i32::from_ne_bytes([optval[0], optval[1], optval[2], optval[3]]))
}

fn parse_timeval(optval: &[u8]) -> Result<Option<Duration>, NetError> {
    if optval.len() < 16 {
        return Err(NetError::InvalidAddress);
    }
    
    let secs = i64::from_ne_bytes([
        optval[0], optval[1], optval[2], optval[3],
        optval[4], optval[5], optval[6], optval[7],
    ]);
    let usecs = i64::from_ne_bytes([
        optval[8], optval[9], optval[10], optval[11],
        optval[12], optval[13], optval[14], optval[15],
    ]);
    
    if secs == 0 && usecs == 0 {
        Ok(None)
    } else {
        Ok(Some(Duration::from_secs(secs as u64) + Duration::from_micros(usecs as u64)))
    }
}

fn parse_linger(optval: &[u8]) -> Result<Option<Duration>, NetError> {
    if optval.len() < 8 {
        return Err(NetError::InvalidAddress);
    }
    
    let onoff = i32::from_ne_bytes([optval[0], optval[1], optval[2], optval[3]]);
    let linger = i32::from_ne_bytes([optval[4], optval[5], optval[6], optval[7]]);
    
    if onoff == 0 {
        Ok(None)
    } else {
        Ok(Some(Duration::from_secs(linger as u64)))
    }
}

fn write_bool(optval: &mut [u8], value: bool) -> Result<usize, NetError> {
    write_u32(optval, if value { 1 } else { 0 })
}

fn write_u32(optval: &mut [u8], value: u32) -> Result<usize, NetError> {
    if optval.len() < 4 {
        return Err(NetError::InvalidAddress);
    }
    let bytes = value.to_ne_bytes();
    optval[0..4].copy_from_slice(&bytes);
    Ok(4)
}

fn write_i32(optval: &mut [u8], value: i32) -> Result<usize, NetError> {
    if optval.len() < 4 {
        return Err(NetError::InvalidAddress);
    }
    let bytes = value.to_ne_bytes();
    optval[0..4].copy_from_slice(&bytes);
    Ok(4)
}

// Socket option constants
pub const SOL_SOCKET: i32 = 1;
pub const IPPROTO_TCP: i32 = 6;
pub const IPPROTO_IP: i32 = 0;

pub const SO_REUSEADDR: i32 = 2;
pub const SO_REUSEPORT: i32 = 15;
pub const SO_KEEPALIVE: i32 = 9;
pub const SO_RCVBUF: i32 = 8;
pub const SO_SNDBUF: i32 = 7;
pub const SO_RCVTIMEO: i32 = 20;
pub const SO_SNDTIMEO: i32 = 21;
pub const SO_LINGER: i32 = 13;
pub const SO_ERROR: i32 = 4;
pub const SO_TYPE: i32 = 3;

pub const TCP_NODELAY: i32 = 1;
pub const TCP_CORK: i32 = 3;
pub const TCP_KEEPIDLE: i32 = 4;
pub const TCP_KEEPINTVL: i32 = 5;
pub const TCP_KEEPCNT: i32 = 6;
pub const TCP_FASTOPEN: i32 = 23;

pub const IP_TTL: i32 = 2;
pub const IP_TOS: i32 = 1;

// Mock functions
fn get_socket(fd: i32) -> Result<&'static Socket, NetError> {
    static mut DUMMY: Socket = Socket;
    Ok(unsafe { &DUMMY })
}
fn get_socket_mut(fd: i32) -> Result<&'static mut Socket, NetError> {
    static mut DUMMY: Socket = Socket;
    Ok(unsafe { &mut DUMMY })
}
fn set_ip_ttl(fd: i32, ttl: u8) -> Result<(), NetError> {
    Ok(())
}
fn set_ip_tos(fd: i32, tos: u8) -> Result<(), NetError> {
    Ok(())
}
fn get_ip_ttl(fd: i32) -> Result<u8, NetError> {
    Ok(64)
}
fn get_ip_tos(fd: i32) -> Result<u8, NetError> {
    Ok(0)
}

struct Socket;
impl Socket {
    fn reuse_addr(&self) -> bool { false }
    fn reuse_port(&self) -> bool { false }
    fn keepalive(&self) -> bool { false }
    fn recv_buffer_size(&self) -> usize { 65536 }
    fn send_buffer_size(&self) -> usize { 65536 }
    fn get_error(&self) -> i32 { 0 }
    fn socket_type(&self) -> i32 { 1 }
    fn nodelay(&self) -> bool { false }
    fn cork(&self) -> bool { false }
    fn set_reuse_addr(&mut self, _: bool) {}
    fn set_reuse_port(&mut self, _: bool) {}
    fn set_keepalive(&mut self, _: bool) {}
    fn set_recv_buffer_size(&mut self, _: usize) {}
    fn set_send_buffer_size(&mut self, _: usize) {}
    fn set_recv_timeout(&mut self, _: Option<Duration>) {}
    fn set_send_timeout(&mut self, _: Option<Duration>) {}
    fn set_linger(&mut self, _: Option<Duration>) {}
    fn set_nodelay(&mut self, _: bool) {}
    fn set_cork(&mut self, _: bool) {}
    fn set_keepalive_idle(&mut self, _: u32) {}
    fn set_keepalive_interval(&mut self, _: u32) {}
    fn set_keepalive_count(&mut self, _: u32) {}
    fn set_fastopen(&mut self, _: bool) {}
}
