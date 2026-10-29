//! `std::sys::exo_os::net` — Networking (`TcpStream`, `TcpListener`, `UdpSocket`, `Epoll`)
//! over `exo-libc::ExoOs` (`PalSocket`).

use exo_libc::platform::PalSocket;
use exo_libc::{ExoOs, Pal};
use exo_syscall::net::{SockAddrIn, AF_INET, SOCK_DGRAM, SOCK_STREAM};
use exo_types::epoll::EpollEventAbi;
use super::stdio::{Error, Read, Result, Write};

/// IPv4 socket address (`std::net::SocketAddrV4`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SocketAddrV4 {
    ip: [u8; 4],
    port: u16,
}

impl SocketAddrV4 {
    #[inline]
    pub const fn new(ip: [u8; 4], port: u16) -> Self {
        Self { ip, port }
    }

    #[inline]
    pub const fn ip(&self) -> [u8; 4] {
        self.ip
    }

    #[inline]
    pub const fn port(&self) -> u16 {
        self.port
    }

    #[inline]
    pub const fn to_sockaddr_in(&self) -> SockAddrIn {
        SockAddrIn {
            sin_family: AF_INET as u16,
            sin_port: self.port.to_be(),
            sin_addr: self.ip,
            sin_zero: [0u8; 8],
        }
    }
}

/// Connected TCP stream (`std::net::TcpStream`).
#[derive(Debug)]
pub struct TcpStream {
    fd: i32,
}

impl TcpStream {
    pub fn connect(addr: SocketAddrV4) -> Result<Self> {
        let fd = ExoOs::socket(AF_INET, SOCK_STREAM, 0)
            .map_err(|e| Error::from_raw_os_error(e.errno()))?;
        let raw = addr.to_sockaddr_in();
        if let Err(e) = ExoOs::connect(fd, &raw) {
            let _ = ExoOs::close(fd);
            return Err(Error::from_raw_os_error(e.errno()));
        }
        Ok(Self { fd })
    }

    #[inline]
    pub const fn as_raw_fd(&self) -> i32 {
        self.fd
    }

    pub fn shutdown(&self, how: i32) -> Result<()> {
        ExoOs::shutdown(self.fd, how).map_err(|e| Error::from_raw_os_error(e.errno()))
    }
}

impl Read for TcpStream {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        ExoOs::recvfrom(self.fd, buf, 0, None).map_err(|e| Error::from_raw_os_error(e.errno()))
    }
}

impl Write for TcpStream {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        ExoOs::sendto(self.fd, buf, 0, None).map_err(|e| Error::from_raw_os_error(e.errno()))
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

impl Drop for TcpStream {
    fn drop(&mut self) {
        if self.fd >= 0 {
            let _ = ExoOs::close(self.fd);
        }
    }
}

/// Listening TCP socket (`std::net::TcpListener`).
#[derive(Debug)]
pub struct TcpListener {
    fd: i32,
}

impl TcpListener {
    pub fn bind(addr: SocketAddrV4) -> Result<Self> {
        let fd = ExoOs::socket(AF_INET, SOCK_STREAM, 0)
            .map_err(|e| Error::from_raw_os_error(e.errno()))?;
        let raw = addr.to_sockaddr_in();
        if let Err(e) = ExoOs::bind(fd, &raw) {
            let _ = ExoOs::close(fd);
            return Err(Error::from_raw_os_error(e.errno()));
        }
        if let Err(e) = ExoOs::listen(fd, 128) {
            let _ = ExoOs::close(fd);
            return Err(Error::from_raw_os_error(e.errno()));
        }
        Ok(Self { fd })
    }

    pub fn accept(&self) -> Result<TcpStream> {
        let client_fd =
            ExoOs::accept(self.fd, None).map_err(|e| Error::from_raw_os_error(e.errno()))?;
        Ok(TcpStream { fd: client_fd })
    }

    #[inline]
    pub const fn as_raw_fd(&self) -> i32 {
        self.fd
    }
}

impl Drop for TcpListener {
    fn drop(&mut self) {
        if self.fd >= 0 {
            let _ = ExoOs::close(self.fd);
        }
    }
}

/// UDP datagram socket (`std::net::UdpSocket`).
#[derive(Debug)]
pub struct UdpSocket {
    fd: i32,
}

impl UdpSocket {
    pub fn bind(addr: SocketAddrV4) -> Result<Self> {
        let fd = ExoOs::socket(AF_INET, SOCK_DGRAM, 0)
            .map_err(|e| Error::from_raw_os_error(e.errno()))?;
        let raw = addr.to_sockaddr_in();
        if let Err(e) = ExoOs::bind(fd, &raw) {
            let _ = ExoOs::close(fd);
            return Err(Error::from_raw_os_error(e.errno()));
        }
        Ok(Self { fd })
    }

    pub fn send_to(&self, buf: &[u8], addr: SocketAddrV4) -> Result<usize> {
        let raw = addr.to_sockaddr_in();
        ExoOs::sendto(self.fd, buf, 0, Some(&raw)).map_err(|e| Error::from_raw_os_error(e.errno()))
    }

    pub fn recv_from(&self, buf: &mut [u8]) -> Result<usize> {
        ExoOs::recvfrom(self.fd, buf, 0, None).map_err(|e| Error::from_raw_os_error(e.errno()))
    }
}

impl Drop for UdpSocket {
    fn drop(&mut self) {
        if self.fd >= 0 {
            let _ = ExoOs::close(self.fd);
        }
    }
}

/// Epoll multiplexer backed by `EpollEventAbi` (`#[repr(C, packed)]`, 12 bytes, `CORR-06`).
#[derive(Debug)]
pub struct Epoll {
    epfd: i32,
}

impl Epoll {
    pub fn create(flags: u32) -> Result<Self> {
        let epfd =
            ExoOs::epoll_create1(flags).map_err(|e| Error::from_raw_os_error(e.errno()))?;
        Ok(Self { epfd })
    }

    pub fn ctl(&self, op: i32, fd: i32, event: &EpollEventAbi) -> Result<()> {
        ExoOs::epoll_ctl(self.epfd, op, fd, Some(event))
            .map_err(|e| Error::from_raw_os_error(e.errno()))
    }

    pub fn wait(&self, events: &mut [EpollEventAbi], timeout_ms: i32) -> Result<usize> {
        ExoOs::epoll_wait(self.epfd, events, timeout_ms)
            .map_err(|e| Error::from_raw_os_error(e.errno()))
    }
}

impl Drop for Epoll {
    fn drop(&mut self) {
        if self.epfd >= 0 {
            let _ = ExoOs::close(self.epfd);
        }
    }
}
