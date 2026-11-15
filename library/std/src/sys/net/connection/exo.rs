use crate::io::{self, BorrowedCursor, IoSlice, IoSliceMut};
use crate::net::{Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, SocketAddrV4, ToSocketAddrs};
use crate::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, RawFd};
use crate::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use crate::sys::fd::FileDesc;
use crate::sys::pal::syscall::{
    self, AF_INET, IP_MULTICAST_LOOP, IP_MULTICAST_TTL, IP_TTL, IPPROTO_IP, IPPROTO_TCP, POLLIN,
    POLLOUT, PollFd, RawTimespec, SO_BROADCAST, SO_ERROR, SO_LINGER, SO_RCVTIMEO, SO_SNDTIMEO,
    SOCK_DGRAM, SOCK_STREAM, SOL_SOCKET, SockAddrIn, SysError, TCP_NODELAY,
};
use crate::sys::pal::{map_exo_err, unsupported};
use crate::sys::{AsInner, FromInner, IntoInner};
use crate::time::Duration;
use crate::vec::Vec;

#[derive(Debug)]
pub struct Socket {
    fd: FileDesc,
    read_timeout_ns: AtomicU64,
    write_timeout_ns: AtomicU64,
    nodelay: AtomicBool,
    ttl: AtomicU32,
    broadcast: AtomicBool,
    multicast_loop_v4: AtomicBool,
    multicast_ttl_v4: AtomicU32,
    linger_secs: AtomicU64,
}

impl Socket {
    fn new(fd: FileDesc) -> Self {
        Self {
            fd,
            read_timeout_ns: AtomicU64::new(0),
            write_timeout_ns: AtomicU64::new(0),
            nodelay: AtomicBool::new(false),
            ttl: AtomicU32::new(64),
            broadcast: AtomicBool::new(false),
            multicast_loop_v4: AtomicBool::new(true),
            multicast_ttl_v4: AtomicU32::new(1),
            linger_secs: AtomicU64::new(u64::MAX),
        }
    }

    fn duplicate(&self) -> io::Result<Self> {
        Ok(Self {
            fd: self.fd.duplicate()?,
            read_timeout_ns: AtomicU64::new(self.read_timeout_ns.load(Ordering::Acquire)),
            write_timeout_ns: AtomicU64::new(self.write_timeout_ns.load(Ordering::Acquire)),
            nodelay: AtomicBool::new(self.nodelay.load(Ordering::Acquire)),
            ttl: AtomicU32::new(self.ttl.load(Ordering::Acquire)),
            broadcast: AtomicBool::new(self.broadcast.load(Ordering::Acquire)),
            multicast_loop_v4: AtomicBool::new(self.multicast_loop_v4.load(Ordering::Acquire)),
            multicast_ttl_v4: AtomicU32::new(self.multicast_ttl_v4.load(Ordering::Acquire)),
            linger_secs: AtomicU64::new(self.linger_secs.load(Ordering::Acquire)),
        })
    }

    fn set_timeout(&self, dur: Option<Duration>, is_read: bool) -> io::Result<()> {
        let ns = match dur {
            Some(d) => {
                if d.is_zero() {
                    return Err(io::const_error!(
                        io::ErrorKind::InvalidInput,
                        "cannot set a 0 duration timeout"
                    ));
                }
                d.as_nanos().min(u64::MAX as u128) as u64
            }
            None => 0,
        };
        if is_read {
            self.read_timeout_ns.store(ns, Ordering::Release);
        } else {
            self.write_timeout_ns.store(ns, Ordering::Release);
        }
        let ts = RawTimespec {
            tv_sec: (ns / 1_000_000_000) as i64,
            tv_nsec: ((ns % 1_000_000_000) / 1_000) as i64,
        };
        let bytes = unsafe {
            core::slice::from_raw_parts(
                &ts as *const RawTimespec as *const u8,
                core::mem::size_of::<RawTimespec>(),
            )
        };
        let opt = if is_read { SO_RCVTIMEO } else { SO_SNDTIMEO };
        let _ = syscall::setsockopt(self.fd.as_raw_fd(), SOL_SOCKET, opt, bytes);
        Ok(())
    }

    fn timeout(&self, is_read: bool) -> Option<Duration> {
        let ns = if is_read {
            self.read_timeout_ns.load(Ordering::Acquire)
        } else {
            self.write_timeout_ns.load(Ordering::Acquire)
        };
        if ns == 0 {
            None
        } else {
            Some(Duration::from_nanos(ns))
        }
    }

    fn wait_io(&self, events: i16, is_read: bool) -> io::Result<()> {
        let ns = if is_read {
            self.read_timeout_ns.load(Ordering::Acquire)
        } else {
            self.write_timeout_ns.load(Ordering::Acquire)
        };
        if ns == 0 {
            return Ok(());
        }
        let timeout_ms = ((ns + 999_999) / 1_000_000).clamp(1, i32::MAX as u64) as i32;
        let mut pfd = [PollFd {
            fd: self.fd.as_raw_fd(),
            events,
            revents: 0,
        }];
        match syscall::poll(&mut pfd, timeout_ms) {
            Ok(0) => Err(io::Error::from(io::ErrorKind::TimedOut)),
            Ok(_) => Ok(()),
            Err(e) => Err(map_exo_err(e)),
        }
    }
}

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

    pub fn connect_timeout(addr: &SocketAddr, timeout: Duration) -> io::Result<TcpStream> {
        if timeout.is_zero() {
            return Err(io::const_error!(
                io::ErrorKind::InvalidInput,
                "cannot set a 0 duration timeout"
            ));
        }
        let raw = to_sockaddr_in(addr)?;
        let fd = syscall::socket(AF_INET, SOCK_STREAM, 0).map_err(map_exo_err)?;
        let sock = unsafe { Socket::from_raw_fd(fd) };
        let _ = sock.fd.set_nonblocking(true);
        match syscall::connect(sock.as_raw_fd(), &raw) {
            Ok(()) => {}
            Err(e) if e == SysError::EINPROGRESS || e == SysError::EAGAIN => {
                let timeout_ms = timeout.as_millis().clamp(1, i32::MAX as u128) as i32;
                let mut pfd = [PollFd {
                    fd: sock.as_raw_fd(),
                    events: POLLOUT,
                    revents: 0,
                }];
                match syscall::poll(&mut pfd, timeout_ms) {
                    Ok(0) => return Err(io::Error::from(io::ErrorKind::TimedOut)),
                    Ok(_) => {}
                    Err(pe) => return Err(map_exo_err(pe)),
                }
            }
            Err(e) => return Err(map_exo_err(e)),
        }
        let _ = sock.fd.set_nonblocking(false);
        Ok(Self { inner: sock })
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.inner.set_timeout(timeout, true)
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.inner.set_timeout(timeout, false)
    }

    pub fn read_timeout(&self) -> io::Result<Option<Duration>> {
        Ok(self.inner.timeout(true))
    }

    pub fn write_timeout(&self) -> io::Result<Option<Duration>> {
        Ok(self.inner.timeout(false))
    }

    pub fn peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.wait_io(POLLIN, true)?;
        syscall::recvfrom(self.inner.as_raw_fd(), buf, 2 /* MSG_PEEK */, None).map_err(map_exo_err)
    }

    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.wait_io(POLLIN, true)?;
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
        self.inner.wait_io(POLLOUT, false)?;
        syscall::sendto(self.inner.as_raw_fd(), buf, 0, None).map_err(map_exo_err)
    }

    pub fn write_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        io::default_write_vectored(|b| self.write(b), bufs)
    }

    pub fn is_write_vectored(&self) -> bool {
        false
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        let mut raw = SockAddrIn::default();
        if syscall::getpeername(self.inner.as_raw_fd(), &mut raw).is_ok() {
            return Ok(from_sockaddr_in(&raw));
        }
        Ok(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
    }

    pub fn socket_addr(&self) -> io::Result<SocketAddr> {
        let mut raw = SockAddrIn::default();
        if syscall::getsockname(self.inner.as_raw_fd(), &mut raw).is_ok() {
            return Ok(from_sockaddr_in(&raw));
        }
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
            inner: self.inner.duplicate()?,
        })
    }

    pub fn set_linger(&self, linger: Option<Duration>) -> io::Result<()> {
        let secs = linger.map(|d| d.as_secs()).unwrap_or(u64::MAX);
        self.inner.linger_secs.store(secs, Ordering::Release);
        let raw_linger: [i32; 2] = match linger {
            Some(d) => [1, d.as_secs().min(i32::MAX as u64) as i32],
            None => [0, 0],
        };
        let bytes = unsafe {
            core::slice::from_raw_parts(raw_linger.as_ptr() as *const u8, 8)
        };
        let _ = syscall::setsockopt(self.inner.as_raw_fd(), SOL_SOCKET, SO_LINGER, bytes);
        Ok(())
    }

    pub fn linger(&self) -> io::Result<Option<Duration>> {
        let secs = self.inner.linger_secs.load(Ordering::Acquire);
        if secs == u64::MAX {
            Ok(None)
        } else {
            Ok(Some(Duration::from_secs(secs)))
        }
    }

    pub fn set_nodelay(&self, nodelay: bool) -> io::Result<()> {
        self.inner.nodelay.store(nodelay, Ordering::Release);
        let val: i32 = if nodelay { 1 } else { 0 };
        let _ = syscall::setsockopt(
            self.inner.as_raw_fd(),
            IPPROTO_TCP,
            TCP_NODELAY,
            &val.to_ne_bytes(),
        );
        Ok(())
    }

    pub fn nodelay(&self) -> io::Result<bool> {
        Ok(self.inner.nodelay.load(Ordering::Acquire))
    }

    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        self.inner.ttl.store(ttl, Ordering::Release);
        let _ = syscall::setsockopt(
            self.inner.as_raw_fd(),
            IPPROTO_IP,
            IP_TTL,
            &ttl.to_ne_bytes(),
        );
        Ok(())
    }

    pub fn ttl(&self) -> io::Result<u32> {
        Ok(self.inner.ttl.load(Ordering::Acquire))
    }

    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        let mut buf = [0u8; 4];
        if let Ok(4) = syscall::getsockopt(self.inner.as_raw_fd(), SOL_SOCKET, SO_ERROR, &mut buf) {
            let err = i32::from_ne_bytes(buf);
            if err != 0 {
                return Ok(Some(io::Error::from_raw_os_error(err)));
            }
        }
        Ok(None)
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.inner.fd.set_nonblocking(nonblocking)
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
        let mut raw = SockAddrIn::default();
        if syscall::getsockname(self.inner.as_raw_fd(), &mut raw).is_ok() {
            return Ok(from_sockaddr_in(&raw));
        }
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
            inner: self.inner.duplicate()?,
        })
    }

    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        self.inner.ttl.store(ttl, Ordering::Release);
        let _ = syscall::setsockopt(
            self.inner.as_raw_fd(),
            IPPROTO_IP,
            IP_TTL,
            &ttl.to_ne_bytes(),
        );
        Ok(())
    }

    pub fn ttl(&self) -> io::Result<u32> {
        Ok(self.inner.ttl.load(Ordering::Acquire))
    }

    pub fn set_only_v6(&self, _only_v6: bool) -> io::Result<()> {
        unsupported()
    }

    pub fn only_v6(&self) -> io::Result<bool> {
        unsupported()
    }

    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        let mut buf = [0u8; 4];
        if let Ok(4) = syscall::getsockopt(self.inner.as_raw_fd(), SOL_SOCKET, SO_ERROR, &mut buf) {
            let err = i32::from_ne_bytes(buf);
            if err != 0 {
                return Ok(Some(io::Error::from_raw_os_error(err)));
            }
        }
        Ok(None)
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.inner.fd.set_nonblocking(nonblocking)
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
        let mut raw = SockAddrIn::default();
        if syscall::getpeername(self.inner.as_raw_fd(), &mut raw).is_ok() {
            return Ok(from_sockaddr_in(&raw));
        }
        Ok(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
    }

    pub fn socket_addr(&self) -> io::Result<SocketAddr> {
        let mut raw = SockAddrIn::default();
        if syscall::getsockname(self.inner.as_raw_fd(), &mut raw).is_ok() {
            return Ok(from_sockaddr_in(&raw));
        }
        Ok(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
    }

    pub fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.inner.wait_io(POLLIN, true)?;
        let mut src = SockAddrIn::default();
        let n = syscall::recvfrom(self.inner.as_raw_fd(), buf, 0, Some(&mut src))
            .map_err(map_exo_err)?;
        Ok((n, from_sockaddr_in(&src)))
    }

    pub fn peek_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.inner.wait_io(POLLIN, true)?;
        let mut src = SockAddrIn::default();
        let n = syscall::recvfrom(self.inner.as_raw_fd(), buf, 2 /* MSG_PEEK */, Some(&mut src))
            .map_err(map_exo_err)?;
        Ok((n, from_sockaddr_in(&src)))
    }

    pub fn send_to(&self, buf: &[u8], addr: &SocketAddr) -> io::Result<usize> {
        self.inner.wait_io(POLLOUT, false)?;
        let raw = to_sockaddr_in(addr)?;
        syscall::sendto(self.inner.as_raw_fd(), buf, 0, Some(&raw)).map_err(map_exo_err)
    }

    pub fn duplicate(&self) -> io::Result<UdpSocket> {
        Ok(Self {
            inner: self.inner.duplicate()?,
        })
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.inner.set_timeout(timeout, true)
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.inner.set_timeout(timeout, false)
    }

    pub fn read_timeout(&self) -> io::Result<Option<Duration>> {
        Ok(self.inner.timeout(true))
    }

    pub fn write_timeout(&self) -> io::Result<Option<Duration>> {
        Ok(self.inner.timeout(false))
    }

    pub fn set_broadcast(&self, broadcast: bool) -> io::Result<()> {
        self.inner.broadcast.store(broadcast, Ordering::Release);
        let val: i32 = if broadcast { 1 } else { 0 };
        let _ = syscall::setsockopt(
            self.inner.as_raw_fd(),
            SOL_SOCKET,
            SO_BROADCAST,
            &val.to_ne_bytes(),
        );
        Ok(())
    }

    pub fn broadcast(&self) -> io::Result<bool> {
        Ok(self.inner.broadcast.load(Ordering::Acquire))
    }

    pub fn set_multicast_loop_v4(&self, val: bool) -> io::Result<()> {
        self.inner.multicast_loop_v4.store(val, Ordering::Release);
        let raw: i32 = if val { 1 } else { 0 };
        let _ = syscall::setsockopt(
            self.inner.as_raw_fd(),
            IPPROTO_IP,
            IP_MULTICAST_LOOP,
            &raw.to_ne_bytes(),
        );
        Ok(())
    }

    pub fn multicast_loop_v4(&self) -> io::Result<bool> {
        Ok(self.inner.multicast_loop_v4.load(Ordering::Acquire))
    }

    pub fn set_multicast_ttl_v4(&self, val: u32) -> io::Result<()> {
        self.inner.multicast_ttl_v4.store(val, Ordering::Release);
        let _ = syscall::setsockopt(
            self.inner.as_raw_fd(),
            IPPROTO_IP,
            IP_MULTICAST_TTL,
            &val.to_ne_bytes(),
        );
        Ok(())
    }

    pub fn multicast_ttl_v4(&self) -> io::Result<u32> {
        Ok(self.inner.multicast_ttl_v4.load(Ordering::Acquire))
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

    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        self.inner.ttl.store(ttl, Ordering::Release);
        let _ = syscall::setsockopt(
            self.inner.as_raw_fd(),
            IPPROTO_IP,
            IP_TTL,
            &ttl.to_ne_bytes(),
        );
        Ok(())
    }

    pub fn ttl(&self) -> io::Result<u32> {
        Ok(self.inner.ttl.load(Ordering::Acquire))
    }

    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        let mut buf = [0u8; 4];
        if let Ok(4) = syscall::getsockopt(self.inner.as_raw_fd(), SOL_SOCKET, SO_ERROR, &mut buf) {
            let err = i32::from_ne_bytes(buf);
            if err != 0 {
                return Ok(Some(io::Error::from_raw_os_error(err)));
            }
        }
        Ok(None)
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.inner.fd.set_nonblocking(nonblocking)
    }

    pub fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.wait_io(POLLIN, true)?;
        syscall::recvfrom(self.inner.as_raw_fd(), buf, 0, None).map_err(map_exo_err)
    }

    pub fn peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.wait_io(POLLIN, true)?;
        syscall::recvfrom(self.inner.as_raw_fd(), buf, 2 /* MSG_PEEK */, None).map_err(map_exo_err)
    }

    pub fn send(&self, buf: &[u8]) -> io::Result<usize> {
        self.inner.wait_io(POLLOUT, false)?;
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
        &self.fd
    }
}

impl IntoInner<FileDesc> for Socket {
    fn into_inner(self) -> FileDesc {
        self.fd
    }
}

impl FromInner<FileDesc> for Socket {
    fn from_inner(file_desc: FileDesc) -> Self {
        Self::new(file_desc)
    }
}

impl AsFd for Socket {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

impl AsRawFd for Socket {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

impl IntoRawFd for Socket {
    fn into_raw_fd(self) -> RawFd {
        self.fd.into_raw_fd()
    }
}

impl FromRawFd for Socket {
    unsafe fn from_raw_fd(raw_fd: RawFd) -> Self {
        Self::new(unsafe { FromRawFd::from_raw_fd(raw_fd) })
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
