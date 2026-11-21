use crate::io::{self, BorrowedCursor, IoSlice, IoSliceMut};
use crate::net::{Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, SocketAddrV4, ToSocketAddrs};
use crate::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, RawFd};
use crate::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use crate::sys::fd::FileDesc;
use crate::sys::pal::syscall::{
    self, AF_INET, CLOCK_MONOTONIC, IP_MULTICAST_LOOP, IP_MULTICAST_TTL, IP_TTL, IPPROTO_IP,
    IPPROTO_TCP, POLLOUT, PollFd, RawTimeval, SO_BROADCAST, SO_ERROR, SO_LINGER, SO_RCVTIMEO,
    SO_SNDTIMEO, SOCK_DGRAM, SOCK_STREAM, SOL_SOCKET, SockAddrIn, SysError, TCP_NODELAY,
};

const SO_REUSEADDR: i32 = 2;
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
        let mut tv_sec = (ns / 1_000_000_000).min(i64::MAX as u64) as i64;
        let mut tv_usec = (((ns % 1_000_000_000) + 999) / 1_000) as i64;
        if tv_usec >= 1_000_000 {
            tv_sec = tv_sec.saturating_add(1);
            tv_usec -= 1_000_000;
        }
        if ns > 0 && tv_sec == 0 && tv_usec == 0 {
            tv_usec = 1;
        }
        let tv = RawTimeval { tv_sec, tv_usec };
        let bytes = unsafe {
            core::slice::from_raw_parts(
                &tv as *const RawTimeval as *const u8,
                core::mem::size_of::<RawTimeval>(),
            )
        };
        let opt = if is_read { SO_RCVTIMEO } else { SO_SNDTIMEO };
        syscall::setsockopt(self.fd.as_raw_fd(), SOL_SOCKET, opt, bytes).map_err(map_exo_err)?;
        if is_read {
            self.read_timeout_ns.store(ns, Ordering::Release);
        } else {
            self.write_timeout_ns.store(ns, Ordering::Release);
        }
        Ok(())
    }

    fn take_error(&self) -> io::Result<Option<io::Error>> {
        let mut buf = [0u8; 4];
        let n = syscall::getsockopt(self.fd.as_raw_fd(), SOL_SOCKET, SO_ERROR, &mut buf)
            .map_err(map_exo_err)?;
        if n >= 4 {
            let err = i32::from_ne_bytes(buf);
            if err != 0 {
                return Ok(Some(io::Error::from_raw_os_error(err)));
            }
        }
        Ok(None)
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
        let mut last_err = None;
        for a in addr.to_socket_addrs()? {
            match to_sockaddr_in(&a) {
                Ok(raw) => {
                    let fd = match syscall::socket(AF_INET, SOCK_STREAM, 0) {
                        Ok(fd) => fd,
                        Err(e) => {
                            last_err = Some(map_exo_err(e));
                            continue;
                        }
                    };
                    let sock = unsafe { Socket::from_raw_fd(fd) };
                    match syscall::connect(sock.as_raw_fd(), &raw) {
                        Ok(()) => return Ok(Self { inner: sock }),
                        Err(e) => last_err = Some(map_exo_err(e)),
                    }
                }
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or_else(|| io::Error::from(io::ErrorKind::InvalidInput)))
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
        sock.fd.set_nonblocking(true)?;
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
                    Ok(_) => {
                        if let Some(err) = sock.take_error()? {
                            return Err(err);
                        }
                    }
                    Err(pe) => return Err(map_exo_err(pe)),
                }
            }
            Err(e) => return Err(map_exo_err(e)),
        }
        sock.fd.set_nonblocking(false)?;
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
        syscall::recvfrom(self.inner.as_raw_fd(), buf, 2 /* MSG_PEEK */, None).map_err(map_exo_err)
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
        let mut raw = SockAddrIn::default();
        syscall::getpeername(self.inner.as_raw_fd(), &mut raw).map_err(map_exo_err)?;
        Ok(from_sockaddr_in(&raw))
    }

    pub fn socket_addr(&self) -> io::Result<SocketAddr> {
        let mut raw = SockAddrIn::default();
        syscall::getsockname(self.inner.as_raw_fd(), &mut raw).map_err(map_exo_err)?;
        Ok(from_sockaddr_in(&raw))
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
        let raw_linger: [i32; 2] = match linger {
            Some(d) => [1, d.as_secs().min(i32::MAX as u64) as i32],
            None => [0, 0],
        };
        let bytes = unsafe {
            core::slice::from_raw_parts(raw_linger.as_ptr() as *const u8, 8)
        };
        syscall::setsockopt(self.inner.as_raw_fd(), SOL_SOCKET, SO_LINGER, bytes)
            .map_err(map_exo_err)?;
        self.inner.linger_secs.store(secs, Ordering::Release);
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
        let val: i32 = if nodelay { 1 } else { 0 };
        syscall::setsockopt(
            self.inner.as_raw_fd(),
            IPPROTO_TCP,
            TCP_NODELAY,
            &val.to_ne_bytes(),
        )
        .map_err(map_exo_err)?;
        self.inner.nodelay.store(nodelay, Ordering::Release);
        Ok(())
    }

    pub fn nodelay(&self) -> io::Result<bool> {
        Ok(self.inner.nodelay.load(Ordering::Acquire))
    }

    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        syscall::setsockopt(
            self.inner.as_raw_fd(),
            IPPROTO_IP,
            IP_TTL,
            &ttl.to_ne_bytes(),
        )
        .map_err(map_exo_err)?;
        self.inner.ttl.store(ttl, Ordering::Release);
        Ok(())
    }

    pub fn ttl(&self) -> io::Result<u32> {
        Ok(self.inner.ttl.load(Ordering::Acquire))
    }

    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.inner.take_error()
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
        let mut last_err = None;
        for a in addr.to_socket_addrs()? {
            match to_sockaddr_in(&a) {
                Ok(raw) => {
                    let fd = match syscall::socket(AF_INET, SOCK_STREAM, 0) {
                        Ok(fd) => fd,
                        Err(e) => {
                            last_err = Some(map_exo_err(e));
                            continue;
                        }
                    };
                    let sock = unsafe { Socket::from_raw_fd(fd) };
                    let reuse: i32 = 1;
                    let _ = syscall::setsockopt(
                        sock.as_raw_fd(),
                        SOL_SOCKET,
                        SO_REUSEADDR,
                        &reuse.to_ne_bytes(),
                    );
                    if let Err(e) = syscall::bind(sock.as_raw_fd(), &raw) {
                        last_err = Some(map_exo_err(e));
                        continue;
                    }
                    if let Err(e) = syscall::listen(sock.as_raw_fd(), 128) {
                        last_err = Some(map_exo_err(e));
                        continue;
                    }
                    return Ok(Self { inner: sock });
                }
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or_else(|| io::Error::from(io::ErrorKind::InvalidInput)))
    }

    pub fn socket_addr(&self) -> io::Result<SocketAddr> {
        let mut raw = SockAddrIn::default();
        syscall::getsockname(self.inner.as_raw_fd(), &mut raw).map_err(map_exo_err)?;
        Ok(from_sockaddr_in(&raw))
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
        syscall::setsockopt(
            self.inner.as_raw_fd(),
            IPPROTO_IP,
            IP_TTL,
            &ttl.to_ne_bytes(),
        )
        .map_err(map_exo_err)?;
        self.inner.ttl.store(ttl, Ordering::Release);
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
        self.inner.take_error()
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
        let mut last_err = None;
        for a in addr.to_socket_addrs()? {
            match to_sockaddr_in(&a) {
                Ok(raw) => {
                    let fd = match syscall::socket(AF_INET, SOCK_DGRAM, 0) {
                        Ok(fd) => fd,
                        Err(e) => {
                            last_err = Some(map_exo_err(e));
                            continue;
                        }
                    };
                    let sock = unsafe { Socket::from_raw_fd(fd) };
                    match syscall::bind(sock.as_raw_fd(), &raw) {
                        Ok(()) => return Ok(Self { inner: sock }),
                        Err(e) => last_err = Some(map_exo_err(e)),
                    }
                }
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or_else(|| io::Error::from(io::ErrorKind::InvalidInput)))
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        let mut raw = SockAddrIn::default();
        syscall::getpeername(self.inner.as_raw_fd(), &mut raw).map_err(map_exo_err)?;
        Ok(from_sockaddr_in(&raw))
    }

    pub fn socket_addr(&self) -> io::Result<SocketAddr> {
        let mut raw = SockAddrIn::default();
        syscall::getsockname(self.inner.as_raw_fd(), &mut raw).map_err(map_exo_err)?;
        Ok(from_sockaddr_in(&raw))
    }

    pub fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        let mut src = SockAddrIn::default();
        let n = syscall::recvfrom(self.inner.as_raw_fd(), buf, 0, Some(&mut src))
            .map_err(map_exo_err)?;
        Ok((n, from_sockaddr_in(&src)))
    }

    pub fn peek_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        let mut src = SockAddrIn::default();
        let n = syscall::recvfrom(self.inner.as_raw_fd(), buf, 2 /* MSG_PEEK */, Some(&mut src))
            .map_err(map_exo_err)?;
        Ok((n, from_sockaddr_in(&src)))
    }

    pub fn send_to(&self, buf: &[u8], addr: &SocketAddr) -> io::Result<usize> {
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
        let val: i32 = if broadcast { 1 } else { 0 };
        syscall::setsockopt(
            self.inner.as_raw_fd(),
            SOL_SOCKET,
            SO_BROADCAST,
            &val.to_ne_bytes(),
        )
        .map_err(map_exo_err)?;
        self.inner.broadcast.store(broadcast, Ordering::Release);
        Ok(())
    }

    pub fn broadcast(&self) -> io::Result<bool> {
        Ok(self.inner.broadcast.load(Ordering::Acquire))
    }

    pub fn set_multicast_loop_v4(&self, val: bool) -> io::Result<()> {
        let raw: i32 = if val { 1 } else { 0 };
        syscall::setsockopt(
            self.inner.as_raw_fd(),
            IPPROTO_IP,
            IP_MULTICAST_LOOP,
            &raw.to_ne_bytes(),
        )
        .map_err(map_exo_err)?;
        self.inner.multicast_loop_v4.store(val, Ordering::Release);
        Ok(())
    }

    pub fn multicast_loop_v4(&self) -> io::Result<bool> {
        Ok(self.inner.multicast_loop_v4.load(Ordering::Acquire))
    }

    pub fn set_multicast_ttl_v4(&self, val: u32) -> io::Result<()> {
        syscall::setsockopt(
            self.inner.as_raw_fd(),
            IPPROTO_IP,
            IP_MULTICAST_TTL,
            &val.to_ne_bytes(),
        )
        .map_err(map_exo_err)?;
        self.inner.multicast_ttl_v4.store(val, Ordering::Release);
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
        syscall::setsockopt(
            self.inner.as_raw_fd(),
            IPPROTO_IP,
            IP_TTL,
            &ttl.to_ne_bytes(),
        )
        .map_err(map_exo_err)?;
        self.inner.ttl.store(ttl, Ordering::Release);
        Ok(())
    }

    pub fn ttl(&self) -> io::Result<u32> {
        Ok(self.inner.ttl.load(Ordering::Acquire))
    }

    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.inner.take_error()
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.inner.fd.set_nonblocking(nonblocking)
    }

    pub fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        syscall::recvfrom(self.inner.as_raw_fd(), buf, 0, None).map_err(map_exo_err)
    }

    pub fn peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        syscall::recvfrom(self.inner.as_raw_fd(), buf, 2 /* MSG_PEEK */, None).map_err(map_exo_err)
    }

    pub fn send(&self, buf: &[u8]) -> io::Result<usize> {
        syscall::sendto(self.inner.as_raw_fd(), buf, 0, None).map_err(map_exo_err)
    }

    pub fn connect<A: ToSocketAddrs>(&self, addr: A) -> io::Result<()> {
        let mut last_err = None;
        for a in addr.to_socket_addrs()? {
            match to_sockaddr_in(&a) {
                Ok(raw) => match syscall::connect(self.inner.as_raw_fd(), &raw) {
                    Ok(()) => return Ok(()),
                    Err(e) => last_err = Some(map_exo_err(e)),
                },
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or_else(|| io::Error::from(io::ErrorKind::InvalidInput)))
    }
}

pub struct LookupHost {
    iter: crate::vec::IntoIter<SocketAddr>,
}

const DNS_PORT: u16 = 53;
const DNS_TYPE_A: u16 = 1;
const DNS_CLASS_IN: u16 = 1;
const DNS_FLAG_QR: u16 = 0x8000;
const DNS_FLAG_TC: u16 = 0x0200;
const DNS_FLAG_RD: u16 = 0x0100;
const DNS_MAX_UDP_PACKET: usize = 512;
const DNS_MAX_TCP_PACKET: usize = 4096;
const DNS_MAX_PTR_JUMPS: usize = 16;

fn lookup_etc_hosts(host: &str, port: u16) -> Vec<SocketAddr> {
    let mut out = Vec::new();
    let Ok(contents) = crate::fs::read_to_string("/etc/hosts") else {
        return out;
    };
    for raw_line in contents.lines() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut tokens = line.split_ascii_whitespace();
        let Some(ip_tok) = tokens.next() else {
            continue;
        };
        let Ok(ip) = ip_tok.parse::<Ipv4Addr>() else {
            continue;
        };
        for alias in tokens {
            let trimmed_alias = alias.strip_suffix('.').unwrap_or(alias);
            if trimmed_alias.eq_ignore_ascii_case(host) {
                let sa = SocketAddr::V4(SocketAddrV4::new(ip, port));
                if !out.contains(&sa) {
                    out.push(sa);
                }
                break;
            }
        }
    }
    out
}

fn load_resolv_nameservers() -> Vec<Ipv4Addr> {
    let mut servers = Vec::new();
    if let Ok(contents) = crate::fs::read_to_string("/etc/resolv.conf") {
        for raw_line in contents.lines() {
            let line = raw_line.split('#').next().unwrap_or("").trim();
            let line = line.split(';').next().unwrap_or("").trim();
            let mut tokens = line.split_ascii_whitespace();
            if tokens.next() == Some("nameserver") {
                if let Some(ip_str) = tokens.next() {
                    if let Ok(ip) = ip_str.parse::<Ipv4Addr>() {
                        if !servers.contains(&ip) {
                            servers.push(ip);
                            if servers.len() >= 4 {
                                break;
                            }
                        }
                    }
                }
            }
        }
    }
    if servers.is_empty() {
        // QEMU SLIRP default resolver fallback when /etc/resolv.conf is absent/empty.
        servers.push(Ipv4Addr::new(10, 0, 2, 3));
    }
    servers
}

fn next_dns_txid() -> u16 {
    let mut buf = [0u8; 2];
    if syscall::getrandom(&mut buf, 0).is_ok() {
        let id = u16::from_be_bytes(buf);
        if id != 0 {
            return id;
        }
    }
    let ts = syscall::clock_gettime(CLOCK_MONOTONIC).unwrap_or_default();
    let mixed = ((ts.tv_nsec as u64) ^ (ts.tv_sec as u64).rotate_left(17)) as u16;
    if mixed == 0 { 0x4558 } else { mixed }
}

fn build_dns_a_query(host: &str, txid: u16) -> io::Result<Vec<u8>> {
    let trimmed = host.strip_suffix('.').unwrap_or(host);
    if trimmed.is_empty() || trimmed.len() > 253 {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    let mut pkt = Vec::with_capacity(12 + trimmed.len() + 6);
    pkt.extend_from_slice(&txid.to_be_bytes());
    pkt.extend_from_slice(&DNS_FLAG_RD.to_be_bytes());
    pkt.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT = 1
    pkt.extend_from_slice(&0u16.to_be_bytes()); // ANCOUNT = 0
    pkt.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT = 0
    pkt.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT = 0

    for label in trimmed.split('.') {
        let bytes = label.as_bytes();
        if bytes.is_empty() || bytes.len() > 63 || !bytes.is_ascii() {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        }
        pkt.push(bytes.len() as u8);
        pkt.extend_from_slice(bytes);
    }
    pkt.push(0); // Root label terminator
    pkt.extend_from_slice(&DNS_TYPE_A.to_be_bytes());
    pkt.extend_from_slice(&DNS_CLASS_IN.to_be_bytes());
    Ok(pkt)
}

/// Skip a DNS wire-format domain name with RFC 1035 compression pointer validation.
fn skip_dns_name(msg: &[u8], start: usize) -> Option<usize> {
    let mut pos = start;
    let mut jumps = 0usize;
    let mut end_after_first_ptr: Option<usize> = None;

    loop {
        let len = *msg.get(pos)?;
        if len == 0 {
            return Some(end_after_first_ptr.unwrap_or(pos + 1));
        }
        let kind = len & 0xC0;
        if kind == 0xC0 {
            let b2 = *msg.get(pos + 1)?;
            let offset = (((len & 0x3F) as usize) << 8) | (b2 as usize);
            if offset >= msg.len() || jumps >= DNS_MAX_PTR_JUMPS {
                return None;
            }
            if end_after_first_ptr.is_none() {
                end_after_first_ptr = Some(pos + 2);
            }
            pos = offset;
            jumps += 1;
        } else if kind == 0 {
            pos = pos.checked_add(1 + (len as usize))?;
            if pos > msg.len() {
                return None;
            }
        } else {
            return None;
        }
    }
}

enum DnsParseOutcome {
    Addresses(Vec<SocketAddr>),
    Truncated,
    NxDomain,
    Malformed,
}

fn parse_dns_a_response(resp: &[u8], expected_txid: u16, port: u16) -> DnsParseOutcome {
    if resp.len() < 12 {
        return DnsParseOutcome::Malformed;
    }
    let rx_txid = u16::from_be_bytes([resp[0], resp[1]]);
    let flags = u16::from_be_bytes([resp[2], resp[3]]);
    if rx_txid != expected_txid || (flags & DNS_FLAG_QR) == 0 {
        return DnsParseOutcome::Malformed;
    }
    if (flags & DNS_FLAG_TC) != 0 {
        return DnsParseOutcome::Truncated;
    }
    let rcode = flags & 0x000F;
    if rcode == 3 {
        return DnsParseOutcome::NxDomain;
    }
    if rcode != 0 {
        return DnsParseOutcome::Malformed;
    }

    let qdcount = u16::from_be_bytes([resp[4], resp[5]]) as usize;
    let ancount = u16::from_be_bytes([resp[6], resp[7]]) as usize;
    let mut pos = 12usize;

    for _ in 0..qdcount {
        let Some(next) = skip_dns_name(resp, pos) else {
            return DnsParseOutcome::Malformed;
        };
        if next + 4 > resp.len() {
            return DnsParseOutcome::Malformed;
        }
        pos = next + 4;
    }

    let mut addrs = Vec::new();
    for _ in 0..ancount {
        let Some(next) = skip_dns_name(resp, pos) else {
            return DnsParseOutcome::Malformed;
        };
        if next + 10 > resp.len() {
            return DnsParseOutcome::Malformed;
        }
        let rtype = u16::from_be_bytes([resp[next], resp[next + 1]]);
        let rclass = u16::from_be_bytes([resp[next + 2], resp[next + 3]]);
        let rdlen = u16::from_be_bytes([resp[next + 8], resp[next + 9]]) as usize;
        let rdata_start = next + 10;
        let Some(rdata_end) = rdata_start.checked_add(rdlen) else {
            return DnsParseOutcome::Malformed;
        };
        if rdata_end > resp.len() {
            return DnsParseOutcome::Malformed;
        }
        if rtype == DNS_TYPE_A && rclass == DNS_CLASS_IN && rdlen == 4 {
            let ip = Ipv4Addr::new(
                resp[rdata_start],
                resp[rdata_start + 1],
                resp[rdata_start + 2],
                resp[rdata_start + 3],
            );
            let sa = SocketAddr::V4(SocketAddrV4::new(ip, port));
            if !addrs.contains(&sa) {
                addrs.push(sa);
            }
        }
        pos = rdata_end;
    }
    DnsParseOutcome::Addresses(addrs)
}

fn tcp_write_all(stream: &TcpStream, mut data: &[u8]) -> io::Result<()> {
    while !data.is_empty() {
        let n = stream.write(data)?;
        if n == 0 {
            return Err(io::Error::from(io::ErrorKind::WriteZero));
        }
        data = &data[n..];
    }
    Ok(())
}

fn tcp_read_exact(stream: &TcpStream, mut buf: &mut [u8]) -> io::Result<()> {
    while !buf.is_empty() {
        let n = stream.read(buf)?;
        if n == 0 {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }
        buf = &mut buf[n..];
    }
    Ok(())
}

fn dns_query_tcp(server: Ipv4Addr, query: &[u8], txid: u16, port: u16) -> io::Result<Vec<SocketAddr>> {
    let addr = SocketAddr::V4(SocketAddrV4::new(server, DNS_PORT));
    let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let len_be = (query.len() as u16).to_be_bytes();
    tcp_write_all(&stream, &len_be)?;
    tcp_write_all(&stream, query)?;

    let mut rx_len_buf = [0u8; 2];
    tcp_read_exact(&stream, &mut rx_len_buf)?;
    let rx_len = u16::from_be_bytes(rx_len_buf) as usize;
    if rx_len < 12 || rx_len > DNS_MAX_TCP_PACKET {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    let mut resp = crate::vec![0u8; rx_len];
    tcp_read_exact(&stream, &mut resp)?;
    match parse_dns_a_response(&resp, txid, port) {
        DnsParseOutcome::Addresses(addrs) if !addrs.is_empty() => Ok(addrs),
        _ => Err(io::Error::new(
            io::ErrorKind::Other,
            "failed to lookup address information",
        )),
    }
}

fn dns_query_udp_or_tcp(server: Ipv4Addr, query: &[u8], txid: u16, port: u16) -> io::Result<Vec<SocketAddr>> {
    let bind_addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0));
    let sock = UdpSocket::bind(bind_addr)?;
    let _ = sock.set_read_timeout(Some(Duration::from_millis(1500)));
    let _ = sock.set_write_timeout(Some(Duration::from_millis(1500)));
    let target = SocketAddr::V4(SocketAddrV4::new(server, DNS_PORT));
    let mut resp = [0u8; DNS_MAX_UDP_PACKET];

    for _attempt in 0..2 {
        if sock.send_to(query, &target).is_err() {
            continue;
        }
        match sock.recv_from(&mut resp) {
            Ok((n, _src)) => match parse_dns_a_response(&resp[..n], txid, port) {
                DnsParseOutcome::Addresses(addrs) if !addrs.is_empty() => return Ok(addrs),
                DnsParseOutcome::Truncated => return dns_query_tcp(server, query, txid, port),
                DnsParseOutcome::NxDomain => {
                    return Err(io::Error::new(
                        io::ErrorKind::Other,
                        "failed to lookup address information: Name or service not known",
                    ));
                }
                DnsParseOutcome::Addresses(_) | DnsParseOutcome::Malformed => continue,
            },
            Err(_) => continue,
        }
    }
    Err(io::Error::new(
        io::ErrorKind::Other,
        "failed to lookup address information",
    ))
}

pub fn lookup_host(host: &str, port: u16) -> io::Result<LookupHost> {
    if let Ok(ip) = host.parse::<Ipv4Addr>() {
        let mut v = Vec::with_capacity(1);
        v.push(SocketAddr::V4(SocketAddrV4::new(ip, port)));
        return Ok(LookupHost { iter: v.into_iter() });
    }

    let trimmed = host.strip_suffix('.').unwrap_or(host);
    if trimmed.is_empty() {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }

    let hosts_matches = lookup_etc_hosts(trimmed, port);
    if !hosts_matches.is_empty() {
        return Ok(LookupHost {
            iter: hosts_matches.into_iter(),
        });
    }

    if trimmed.eq_ignore_ascii_case("localhost") {
        let mut v = Vec::with_capacity(1);
        v.push(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)));
        return Ok(LookupHost { iter: v.into_iter() });
    }

    let txid = next_dns_txid();
    let query = build_dns_a_query(trimmed, txid)?;
    let servers = load_resolv_nameservers();
    let mut last_err = None;
    for server in servers {
        match dns_query_udp_or_tcp(server, &query, txid, port) {
            Ok(addrs) if !addrs.is_empty() => {
                return Ok(LookupHost {
                    iter: addrs.into_iter(),
                });
            }
            Ok(_) => {}
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| {
        io::Error::new(io::ErrorKind::Other, "failed to lookup address information")
    }))
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
