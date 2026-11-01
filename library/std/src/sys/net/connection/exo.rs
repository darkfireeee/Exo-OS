use crate::io::{self, BorrowedCursor, IoSlice, IoSliceMut};
use crate::net::{Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, SocketAddrV4, ToSocketAddrs};
use crate::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, RawFd};
use crate::sys::fd::FileDesc;
use crate::sys::pal::syscall::{self, AF_INET, SOCK_DGRAM, SOCK_STREAM, SockAddrIn};
use crate::sys::pal::{map_exo_err, unsupported};
use crate::sys::{AsInner, FromInner, IntoInner};
use crate::time::Duration;
use crate::vec::Vec;

#[derive(Debug)]
pub struct Socket(FileDesc);

fn to_sockaddr_in(addr: &SocketAddr) -> io::Result<SockAddrIn> {
    match addr {
        SocketAddr::V4(v4) => Ok(SockAddrIn {
            sin_family: AF_INET as u16,
            sin_port: v4.port().to_be(),
            sin_addr: v4.ip().octets(),
            sin_zero: [0u8; 8],
        }),
        SocketAddr::V6(_) => unsupported(),
    }
}

fn from_sockaddr_in(raw: &SockAddrIn) -> SocketAddr {
    SocketAddr::V4(SocketAddrV4::new(
        Ipv4Addr::from(raw.sin_addr),
        u16::from_be(raw.sin_port),
    ))
}

#[derive(Debug)]
pub struct TcpStream {
    inner: Socket,
}

impl TcpStream {
    pub fn socket(&self) -> &Socket {
        &self.inner
    }

    pub fn into_socket(self) -> Socket {
        self.inner
    }

    pub fn connect<A: ToSocketAddrs>(addr: A) -> io::Result<TcpStream> {
        let first = addr
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        let raw = to_sockaddr_in(&first)?;
        let fd = syscall::socket(AF_INET, SOCK_STREAM, 0).map_err(map_exo_err)?;
        let sock = unsafe { Socket::from_raw_fd(fd) };
        syscall::connect(sock.as_raw_fd(), &raw).map_err(map_exo_err)?;
        Ok(Self { inner: sock })
    }

    pub fn connect_timeout(addr: &SocketAddr, _timeout: Duration) -> io::Result<TcpStream> {
        Self::connect(addr)
    }

    pub fn set_read_timeout(&self, _timeout: Option<Duration>) -> io::Result<()> {
        Ok(())
    }

    pub fn set_write_timeout(&self, _timeout: Option<Duration>) -> io::Result<()> {
        Ok(())
    }

    pub fn read_timeout(&self) -> io::Result<Option<Duration>> {
        Ok(None)
    }

    pub fn write_timeout(&self) -> io::Result<Option<Duration>> {
        Ok(None)
    }

    pub fn peek(&self, _buf: &mut [u8]) -> io::Result<usize> {
        unsupported()
    }

    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        syscall::recvfrom(self.inner.as_raw_fd(), buf, 0, None).map_err(map_exo_err)
    }

    pub fn read_buf(&self, mut cursor: BorrowedCursor<'_>) -> io::Result<()> {
        let n = self.read(cursor.ensure_init().init_mut())?;
        cursor.advance(n);
        Ok(())
    }

    pub fn read_vectored(&self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        io::default_read_vectored(|b| self.read(b), bufs)
    }

    pub fn is_read_vectored(&self) -> bool {
        false
    }

    pub fn write(&self, buf: &[u8]) -> io::Result<usize> {
        syscall::sendto(self.inner.as_raw_fd(), buf, 0, None).map_err(map_exo_err)
    }

    pub fn write_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        io::default_write_vectored(|b| self.write(b), bufs)
    }

    pub fn is_write_vectored(&self) -> bool {
        false
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        Ok(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
    }

    pub fn socket_addr(&self) -> io::Result<SocketAddr> {
        Ok(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
    }

    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        let how_int = match how {
            Shutdown::Read => 0,
            Shutdown::Write => 1,
            Shutdown::Both => 2,
        };
        syscall::shutdown(self.inner.as_raw_fd(), how_int).map_err(map_exo_err)
    }

    pub fn duplicate(&self) -> io::Result<TcpStream> {
        Ok(Self {
            inner: Socket(self.inner.0.duplicate()?),
        })
    }

    pub fn set_linger(&self, _linger: Option<Duration>) -> io::Result<()> {
        Ok(())
    }

    pub fn linger(&self) -> io::Result<Option<Duration>> {
        Ok(None)
    }

    pub fn set_nodelay(&self, _nodelay: bool) -> io::Result<()> {
        Ok(())
    }

    pub fn nodelay(&self) -> io::Result<bool> {
        Ok(false)
    }

    pub fn set_ttl(&self, _ttl: u32) -> io::Result<()> {
        Ok(())
    }

    pub fn ttl(&self) -> io::Result<u32> {
        Ok(64)
    }

    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        Ok(None)
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.inner.0.set_nonblocking(nonblocking)
    }
}

#[derive(Debug)]
pub struct TcpListener {
    inner: Socket,
}

impl TcpListener {
    pub fn socket(&self) -> &Socket {
        &self.inner
    }

    pub fn into_socket(self) -> Socket {
        self.inner
    }

    pub fn bind<A: ToSocketAddrs>(addr: A) -> io::Result<TcpListener> {
        let first = addr
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        let raw = to_sockaddr_in(&first)?;
        let fd = syscall::socket(AF_INET, SOCK_STREAM, 0).map_err(map_exo_err)?;
        let sock = unsafe { Socket::from_raw_fd(fd) };
        syscall::bind(sock.as_raw_fd(), &raw).map_err(map_exo_err)?;
        syscall::listen(sock.as_raw_fd(), 128).map_err(map_exo_err)?;
        Ok(Self { inner: sock })
    }

    pub fn socket_addr(&self) -> io::Result<SocketAddr> {
        Ok(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
    }

    pub fn accept(&self) -> io::Result<(TcpStream, SocketAddr)> {
        let mut peer = SockAddrIn::default();
        let cfd = syscall::accept(self.inner.as_raw_fd(), Some(&mut peer)).map_err(map_exo_err)?;
        let stream = TcpStream {
            inner: unsafe { Socket::from_raw_fd(cfd) },
        };
        Ok((stream, from_sockaddr_in(&peer)))
    }

    pub fn duplicate(&self) -> io::Result<TcpListener> {
        Ok(Self {
            inner: Socket(self.inner.0.duplicate()?),
        })
    }

    pub fn set_ttl(&self, _ttl: u32) -> io::Result<()> {
        Ok(())
    }

    pub fn ttl(&self) -> io::Result<u32> {
        Ok(64)
    }

    pub fn set_only_v6(&self, _only_v6: bool) -> io::Result<()> {
        unsupported()
    }

    pub fn only_v6(&self) -> io::Result<bool> {
        unsupported()
    }

    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        Ok(None)
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.inner.0.set_nonblocking(nonblocking)
    }
}

#[derive(Debug)]
pub struct UdpSocket {
    inner: Socket,
}

impl UdpSocket {
    pub fn socket(&self) -> &Socket {
        &self.inner
    }

    pub fn into_socket(self) -> Socket {
        self.inner
    }

    pub fn bind<A: ToSocketAddrs>(addr: A) -> io::Result<UdpSocket> {
        let first = addr
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        let raw = to_sockaddr_in(&first)?;
        let fd = syscall::socket(AF_INET, SOCK_DGRAM, 0).map_err(map_exo_err)?;
        let sock = unsafe { Socket::from_raw_fd(fd) };
        syscall::bind(sock.as_raw_fd(), &raw).map_err(map_exo_err)?;
        Ok(Self { inner: sock })
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        Ok(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
    }

    pub fn socket_addr(&self) -> io::Result<SocketAddr> {
        Ok(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
    }

    pub fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        let mut src = SockAddrIn::default();
        let n = syscall::recvfrom(self.inner.as_raw_fd(), buf, 0, Some(&mut src))
            .map_err(map_exo_err)?;
        Ok((n, from_sockaddr_in(&src)))
    }

    pub fn peek_from(&self, _buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        unsupported()
    }

    pub fn send_to(&self, buf: &[u8], addr: &SocketAddr) -> io::Result<usize> {
        let raw = to_sockaddr_in(addr)?;
        syscall::sendto(self.inner.as_raw_fd(), buf, 0, Some(&raw)).map_err(map_exo_err)
    }

    pub fn duplicate(&self) -> io::Result<UdpSocket> {
        Ok(Self {
            inner: Socket(self.inner.0.duplicate()?),
        })
    }

    pub fn set_read_timeout(&self, _timeout: Option<Duration>) -> io::Result<()> {
        Ok(())
    }

    pub fn set_write_timeout(&self, _timeout: Option<Duration>) -> io::Result<()> {
        Ok(())
    }

    pub fn read_timeout(&self) -> io::Result<Option<Duration>> {
        Ok(None)
    }

    pub fn write_timeout(&self) -> io::Result<Option<Duration>> {
        Ok(None)
    }

    pub fn set_broadcast(&self, _broadcast: bool) -> io::Result<()> {
        Ok(())
    }

    pub fn broadcast(&self) -> io::Result<bool> {
        Ok(false)
    }

    pub fn set_multicast_loop_v4(&self, _val: bool) -> io::Result<()> {
        Ok(())
    }

    pub fn multicast_loop_v4(&self) -> io::Result<bool> {
        Ok(false)
    }

    pub fn set_multicast_ttl_v4(&self, _val: u32) -> io::Result<()> {
        Ok(())
    }

    pub fn multicast_ttl_v4(&self) -> io::Result<u32> {
        Ok(1)
    }

    pub fn set_multicast_loop_v6(&self, _val: bool) -> io::Result<()> {
        unsupported()
    }

    pub fn multicast_loop_v6(&self) -> io::Result<bool> {
        unsupported()
    }

    pub fn join_multicast_v4(&self, _addr: &Ipv4Addr, _iface: &Ipv4Addr) -> io::Result<()> {
        unsupported()
    }

    pub fn join_multicast_v6(&self, _addr: &Ipv6Addr, _iface: u32) -> io::Result<()> {
        unsupported()
    }

    pub fn leave_multicast_v4(&self, _addr: &Ipv4Addr, _iface: &Ipv4Addr) -> io::Result<()> {
        unsupported()
    }

    pub fn leave_multicast_v6(&self, _addr: &Ipv6Addr, _iface: u32) -> io::Result<()> {
        unsupported()
    }

    pub fn set_ttl(&self, _ttl: u32) -> io::Result<()> {
        Ok(())
    }

    pub fn ttl(&self) -> io::Result<u32> {
        Ok(64)
    }

    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        Ok(None)
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.inner.0.set_nonblocking(nonblocking)
    }

    pub fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        syscall::recvfrom(self.inner.as_raw_fd(), buf, 0, None).map_err(map_exo_err)
    }

    pub fn peek(&self, _buf: &mut [u8]) -> io::Result<usize> {
        unsupported()
    }

    pub fn send(&self, buf: &[u8]) -> io::Result<usize> {
        syscall::sendto(self.inner.as_raw_fd(), buf, 0, None).map_err(map_exo_err)
    }

    pub fn connect<A: ToSocketAddrs>(&self, addr: A) -> io::Result<()> {
        let first = addr
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        let raw = to_sockaddr_in(&first)?;
        syscall::connect(self.inner.as_raw_fd(), &raw).map_err(map_exo_err)
    }
}

pub struct LookupHost {
    iter: crate::vec::IntoIter<SocketAddr>,
}

pub fn lookup_host(host: &str, port: u16) -> io::Result<LookupHost> {
    let ip = if host == "localhost" {
        Ipv4Addr::LOCALHOST
    } else {
        host.parse::<Ipv4Addr>()
            .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?
    };
    let mut v = Vec::with_capacity(1);
    v.push(SocketAddr::V4(SocketAddrV4::new(ip, port)));
    Ok(LookupHost {
        iter: v.into_iter(),
    })
}

impl Iterator for LookupHost {
    type Item = SocketAddr;
    fn next(&mut self) -> Option<SocketAddr> {
        self.iter.next()
    }
}

impl TryFrom<&str> for LookupHost {
    type Error = io::Error;

    fn try_from(host_port: &str) -> io::Result<LookupHost> {
        let (host, port_str) = host_port
            .rsplit_once(':')
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        let port: u16 = port_str
            .parse()
            .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        lookup_host(host, port)
    }
}

impl<'a> TryFrom<(&'a str, u16)> for LookupHost {
    type Error = io::Error;

    fn try_from(host_port: (&'a str, u16)) -> io::Result<LookupHost> {
        lookup_host(host_port.0, host_port.1)
    }
}

impl AsInner<FileDesc> for Socket {
    #[inline]
    fn as_inner(&self) -> &FileDesc {
        &self.0
    }
}

impl IntoInner<FileDesc> for Socket {
    fn into_inner(self) -> FileDesc {
        self.0
    }
}

impl FromInner<FileDesc> for Socket {
    fn from_inner(file_desc: FileDesc) -> Self {
        Self(file_desc)
    }
}

impl AsFd for Socket {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl AsRawFd for Socket {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

impl IntoRawFd for Socket {
    fn into_raw_fd(self) -> RawFd {
        self.0.into_raw_fd()
    }
}

impl FromRawFd for Socket {
    unsafe fn from_raw_fd(raw_fd: RawFd) -> Self {
        Self(unsafe { FromRawFd::from_raw_fd(raw_fd) })
    }
}

impl AsInner<Socket> for TcpStream {
    #[inline]
    fn as_inner(&self) -> &Socket {
        &self.inner
    }
}

impl FromInner<Socket> for TcpStream {
    fn from_inner(socket: Socket) -> TcpStream {
        TcpStream { inner: socket }
    }
}

impl FromInner<Socket> for TcpListener {
    fn from_inner(socket: Socket) -> TcpListener {
        TcpListener { inner: socket }
    }
}

impl FromInner<Socket> for UdpSocket {
    fn from_inner(socket: Socket) -> UdpSocket {
        UdpSocket { inner: socket }
    }
}
