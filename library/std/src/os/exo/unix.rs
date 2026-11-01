//! Unix-compatible extensions (`std::os::unix`) for Exo-OS (`target_os = "exo"`).
//!
//! Provides `io`, `ffi`, `fs`, `net` (`UnixStream`, `UnixListener`, `UnixDatagram`, `SocketAddr`),
//! and `prelude` backed by Exo-OS `AF_UNIX` sockets and VFS descriptors.

#![stable(feature = "rust1", since = "1.0.0")]
#![allow(missing_docs, dead_code)]

pub mod io {
    #![stable(feature = "rust1", since = "1.0.0")]
    #[stable(feature = "rust1", since = "1.0.0")]
    pub use crate::os::fd::*;
}

pub mod ffi {
    #![stable(feature = "rust1", since = "1.0.0")]
    #[stable(feature = "rust1", since = "1.0.0")]
    pub use crate::os::exo::ffi::*;
}

pub mod fs {
    #![stable(feature = "rust1", since = "1.0.0")]

    use crate::fs::{FileType, Metadata, OpenOptions, Permissions};
    use crate::sys::{AsInner, AsInnerMut, FromInner};

    #[stable(feature = "rust1", since = "1.0.0")]
    pub trait OpenOptionsExt {
        #[stable(feature = "rust1", since = "1.0.0")]
        fn mode(&mut self, mode: u32) -> &mut Self;

        #[stable(feature = "rust1", since = "1.0.0")]
        fn custom_flags(&mut self, flags: i32) -> &mut Self;
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    impl OpenOptionsExt for OpenOptions {
        fn mode(&mut self, mode: u32) -> &mut OpenOptions {
            self.as_inner_mut().mode(mode);
            self
        }

        fn custom_flags(&mut self, flags: i32) -> &mut OpenOptions {
            self.as_inner_mut().custom_flags(flags);
            self
        }
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub trait MetadataExt {
        #[stable(feature = "rust1", since = "1.0.0")]
        fn dev(&self) -> u64;
        #[stable(feature = "rust1", since = "1.0.0")]
        fn ino(&self) -> u64;
        #[stable(feature = "rust1", since = "1.0.0")]
        fn mode(&self) -> u32;
        #[stable(feature = "rust1", since = "1.0.0")]
        fn nlink(&self) -> u64;
        #[stable(feature = "rust1", since = "1.0.0")]
        fn uid(&self) -> u32;
        #[stable(feature = "rust1", since = "1.0.0")]
        fn gid(&self) -> u32;
        #[stable(feature = "rust1", since = "1.0.0")]
        fn rdev(&self) -> u64;
        #[stable(feature = "rust1", since = "1.0.0")]
        fn size(&self) -> u64;
        #[stable(feature = "rust1", since = "1.0.0")]
        fn atime(&self) -> i64;
        #[stable(feature = "rust1", since = "1.0.0")]
        fn atime_nsec(&self) -> i64;
        #[stable(feature = "rust1", since = "1.0.0")]
        fn mtime(&self) -> i64;
        #[stable(feature = "rust1", since = "1.0.0")]
        fn mtime_nsec(&self) -> i64;
        #[stable(feature = "rust1", since = "1.0.0")]
        fn ctime(&self) -> i64;
        #[stable(feature = "rust1", since = "1.0.0")]
        fn ctime_nsec(&self) -> i64;
        #[stable(feature = "rust1", since = "1.0.0")]
        fn blksize(&self) -> u64;
        #[stable(feature = "rust1", since = "1.0.0")]
        fn blocks(&self) -> u64;
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    impl MetadataExt for Metadata {
        fn dev(&self) -> u64 {
            0
        }
        fn ino(&self) -> u64 {
            let oid = &self.as_inner().stat.object_id;
            u64::from_le_bytes([
                oid[0], oid[1], oid[2], oid[3], oid[4], oid[5], oid[6], oid[7],
            ])
        }
        fn mode(&self) -> u32 {
            self.as_inner().stat.mode
        }
        fn nlink(&self) -> u64 {
            1
        }
        fn uid(&self) -> u32 {
            0
        }
        fn gid(&self) -> u32 {
            0
        }
        fn rdev(&self) -> u64 {
            0
        }
        fn size(&self) -> u64 {
            self.as_inner().stat.size
        }
        fn atime(&self) -> i64 {
            (self.as_inner().stat.mtime_ns / 1_000_000_000) as i64
        }
        fn atime_nsec(&self) -> i64 {
            (self.as_inner().stat.mtime_ns % 1_000_000_000) as i64
        }
        fn mtime(&self) -> i64 {
            (self.as_inner().stat.mtime_ns / 1_000_000_000) as i64
        }
        fn mtime_nsec(&self) -> i64 {
            (self.as_inner().stat.mtime_ns % 1_000_000_000) as i64
        }
        fn ctime(&self) -> i64 {
            (self.as_inner().stat.ctime_ns / 1_000_000_000) as i64
        }
        fn ctime_nsec(&self) -> i64 {
            (self.as_inner().stat.ctime_ns % 1_000_000_000) as i64
        }
        fn blksize(&self) -> u64 {
            4096
        }
        fn blocks(&self) -> u64 {
            (self.as_inner().stat.size + 511) / 512
        }
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub trait PermissionsExt {
        #[stable(feature = "rust1", since = "1.0.0")]
        fn mode(&self) -> u32;
        #[stable(feature = "rust1", since = "1.0.0")]
        fn set_mode(&mut self, mode: u32);
        #[stable(feature = "rust1", since = "1.0.0")]
        fn from_mode(mode: u32) -> Self;
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    impl PermissionsExt for Permissions {
        fn mode(&self) -> u32 {
            self.as_inner().mode()
        }
        fn set_mode(&mut self, mode: u32) {
            *self = Permissions::from_inner(crate::sys::fs::FilePermissions { mode });
        }
        fn from_mode(mode: u32) -> Permissions {
            Permissions::from_inner(crate::sys::fs::FilePermissions { mode })
        }
    }

    #[stable(feature = "file_type_ext", since = "1.5.0")]
    pub trait FileTypeExt {
        #[stable(feature = "file_type_ext", since = "1.5.0")]
        fn is_block_device(&self) -> bool;
        #[stable(feature = "file_type_ext", since = "1.5.0")]
        fn is_char_device(&self) -> bool;
        #[stable(feature = "file_type_ext", since = "1.5.0")]
        fn is_fifo(&self) -> bool;
        #[stable(feature = "file_type_ext", since = "1.5.0")]
        fn is_socket(&self) -> bool;
    }

    #[stable(feature = "file_type_ext", since = "1.5.0")]
    impl FileTypeExt for FileType {
        fn is_block_device(&self) -> bool {
            (self.as_inner().mode & 0o170000) == 0o060000
        }
        fn is_char_device(&self) -> bool {
            (self.as_inner().mode & 0o170000) == 0o020000
        }
        fn is_fifo(&self) -> bool {
            (self.as_inner().mode & 0o170000) == 0o010000
        }
        fn is_socket(&self) -> bool {
            (self.as_inner().mode & 0o170000) == 0o140000
        }
    }
}

pub mod net {
    #![stable(feature = "unix_socket", since = "1.10.0")]

    use crate::fmt;
    use crate::io::{self, IoSlice, IoSliceMut, Read, Write};
    use crate::net::Shutdown;
    use crate::os::exo::ffi::OsStrExt;
    use crate::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
    use crate::path::{Path, PathBuf};
    use crate::sys::fd::FileDesc;
    use crate::sys::syscall::{
        self, AF_UNIX, SOCK_DGRAM, SOCK_STREAM, SockAddrUn,
    };
    use crate::sys::map_exo_err;
    use crate::sys::{AsInner, FromInner, IntoInner};
    use crate::time::Duration;

    fn sockaddr_un_from_path(path: &Path) -> io::Result<SockAddrUn> {
        let bytes = path.as_os_str().as_bytes();
        if bytes.is_empty() || bytes.len() >= 108 {
            return Err(io::const_error!(
                io::ErrorKind::InvalidInput,
                "unix socket path must be between 1 and 107 bytes"
            ));
        }
        let mut addr = SockAddrUn::default();
        addr.sun_family = AF_UNIX as u16;
        addr.sun_path[..bytes.len()].copy_from_slice(bytes);
        Ok(addr)
    }

    #[derive(Clone)]
    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub struct SocketAddr {
        path: Option<PathBuf>,
    }

    impl SocketAddr {
        #[stable(feature = "unix_socket", since = "1.10.0")]
        pub fn is_unnamed(&self) -> bool {
            self.path.is_none()
        }

        #[stable(feature = "unix_socket", since = "1.10.0")]
        pub fn as_pathname(&self) -> Option<&Path> {
            self.path.as_deref()
        }
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    impl fmt::Debug for SocketAddr {
        fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
            match &self.path {
                Some(p) => write!(fmt, "{p:?} (pathname)"),
                None => write!(fmt, "(unnamed)"),
            }
        }
    }

    #[derive(Debug)]
    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub struct UnixStream(FileDesc);

    impl UnixStream {
        #[stable(feature = "unix_socket", since = "1.10.0")]
        pub fn connect<P: AsRef<Path>>(path: P) -> io::Result<UnixStream> {
            let addr = sockaddr_un_from_path(path.as_ref())?;
            let raw = syscall::socket(AF_UNIX, SOCK_STREAM, 0).map_err(map_exo_err)?;
            let fd = unsafe { FileDesc::from_raw_fd(raw) };
            syscall::connect_unix(fd.as_raw_fd(), &addr).map_err(map_exo_err)?;
            Ok(UnixStream(fd))
        }

        #[stable(feature = "unix_socket", since = "1.10.0")]
        pub fn pair() -> io::Result<(UnixStream, UnixStream)> {
            let [a, b] = syscall::socketpair(AF_UNIX, SOCK_STREAM, 0).map_err(map_exo_err)?;
            unsafe {
                Ok((
                    UnixStream(FileDesc::from_raw_fd(a)),
                    UnixStream(FileDesc::from_raw_fd(b)),
                ))
            }
        }

        #[stable(feature = "unix_socket", since = "1.10.0")]
        pub fn try_clone(&self) -> io::Result<UnixStream> {
            self.0.duplicate().map(UnixStream)
        }

        #[stable(feature = "unix_socket", since = "1.10.0")]
        pub fn local_addr(&self) -> io::Result<SocketAddr> {
            Ok(SocketAddr { path: None })
        }

        #[stable(feature = "unix_socket", since = "1.10.0")]
        pub fn peer_addr(&self) -> io::Result<SocketAddr> {
            Ok(SocketAddr { path: None })
        }

        #[stable(feature = "unix_socket", since = "1.10.0")]
        pub fn set_read_timeout(&self, _timeout: Option<Duration>) -> io::Result<()> {
            Ok(())
        }

        #[stable(feature = "unix_socket", since = "1.10.0")]
        pub fn set_write_timeout(&self, _timeout: Option<Duration>) -> io::Result<()> {
            Ok(())
        }

        #[stable(feature = "unix_socket", since = "1.10.0")]
        pub fn read_timeout(&self) -> io::Result<Option<Duration>> {
            Ok(None)
        }

        #[stable(feature = "unix_socket", since = "1.10.0")]
        pub fn write_timeout(&self) -> io::Result<Option<Duration>> {
            Ok(None)
        }

        #[stable(feature = "unix_socket", since = "1.10.0")]
        pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
            self.0.set_nonblocking(nonblocking)
        }

        #[stable(feature = "unix_socket", since = "1.10.0")]
        pub fn take_error(&self) -> io::Result<Option<io::Error>> {
            Ok(None)
        }

        #[stable(feature = "unix_socket", since = "1.10.0")]
        pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
            let how = match how {
                Shutdown::Read => 0,
                Shutdown::Write => 1,
                Shutdown::Both => 2,
            };
            syscall::shutdown(self.0.as_raw_fd(), how).map_err(map_exo_err)
        }
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    impl Read for UnixStream {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.0.read(buf)
        }
        fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
            self.0.read_vectored(bufs)
        }
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    impl Read for &UnixStream {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.0.read(buf)
        }
        fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
            self.0.read_vectored(bufs)
        }
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    impl Write for UnixStream {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.write(buf)
        }
        fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
            self.0.write_vectored(bufs)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    impl Write for &UnixStream {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.write(buf)
        }
        fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
            self.0.write_vectored(bufs)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    impl AsFd for UnixStream {
        fn as_fd(&self) -> BorrowedFd<'_> {
            self.0.as_fd()
        }
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    impl AsRawFd for UnixStream {
        fn as_raw_fd(&self) -> RawFd {
            self.0.as_raw_fd()
        }
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    impl FromRawFd for UnixStream {
        unsafe fn from_raw_fd(fd: RawFd) -> UnixStream {
            UnixStream(unsafe { FileDesc::from_raw_fd(fd) })
        }
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    impl IntoRawFd for UnixStream {
        fn into_raw_fd(self) -> RawFd {
            self.0.into_raw_fd()
        }
    }

    #[stable(feature = "io_safety", since = "1.63.0")]
    impl From<OwnedFd> for UnixStream {
        fn from(fd: OwnedFd) -> UnixStream {
            UnixStream(FileDesc::from_inner(fd))
        }
    }

    #[stable(feature = "io_safety", since = "1.63.0")]
    impl From<UnixStream> for OwnedFd {
        fn from(stream: UnixStream) -> OwnedFd {
            stream.0.into_inner()
        }
    }

    #[derive(Debug)]
    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub struct UnixListener {
        fd: FileDesc,
        path: Option<PathBuf>,
    }

    impl UnixListener {
        #[stable(feature = "unix_socket", since = "1.10.0")]
        pub fn bind<P: AsRef<Path>>(path: P) -> io::Result<UnixListener> {
            let p = path.as_ref();
            let addr = sockaddr_un_from_path(p)?;
            let raw = syscall::socket(AF_UNIX, SOCK_STREAM, 0).map_err(map_exo_err)?;
            let fd = unsafe { FileDesc::from_raw_fd(raw) };
            syscall::bind_unix(fd.as_raw_fd(), &addr).map_err(map_exo_err)?;
            syscall::listen(fd.as_raw_fd(), 128).map_err(map_exo_err)?;
            Ok(UnixListener {
                fd,
                path: Some(p.to_path_buf()),
            })
        }

        #[stable(feature = "unix_socket", since = "1.10.0")]
        pub fn accept(&self) -> io::Result<(UnixStream, SocketAddr)> {
            let cfd = syscall::accept_unix(self.fd.as_raw_fd()).map_err(map_exo_err)?;
            let stream = unsafe { UnixStream::from_raw_fd(cfd) };
            Ok((stream, SocketAddr { path: None }))
        }

        #[stable(feature = "unix_socket", since = "1.10.0")]
        pub fn try_clone(&self) -> io::Result<UnixListener> {
            Ok(UnixListener {
                fd: self.fd.duplicate()?,
                path: self.path.clone(),
            })
        }

        #[stable(feature = "unix_socket", since = "1.10.0")]
        pub fn local_addr(&self) -> io::Result<SocketAddr> {
            Ok(SocketAddr {
                path: self.path.clone(),
            })
        }

        #[stable(feature = "unix_socket", since = "1.10.0")]
        pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
            self.fd.set_nonblocking(nonblocking)
        }

        #[stable(feature = "unix_socket", since = "1.10.0")]
        pub fn take_error(&self) -> io::Result<Option<io::Error>> {
            Ok(None)
        }

        #[stable(feature = "unix_socket", since = "1.10.0")]
        pub fn incoming(&self) -> Incoming<'_> {
            Incoming { listener: self }
        }
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    impl AsFd for UnixListener {
        fn as_fd(&self) -> BorrowedFd<'_> {
            self.fd.as_fd()
        }
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    impl AsRawFd for UnixListener {
        fn as_raw_fd(&self) -> RawFd {
            self.fd.as_raw_fd()
        }
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    impl FromRawFd for UnixListener {
        unsafe fn from_raw_fd(fd: RawFd) -> UnixListener {
            UnixListener {
                fd: unsafe { FileDesc::from_raw_fd(fd) },
                path: None,
            }
        }
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    impl IntoRawFd for UnixListener {
        fn into_raw_fd(self) -> RawFd {
            self.fd.into_raw_fd()
        }
    }

    #[stable(feature = "io_safety", since = "1.63.0")]
    impl From<OwnedFd> for UnixListener {
        fn from(fd: OwnedFd) -> UnixListener {
            UnixListener {
                fd: FileDesc::from_inner(fd),
                path: None,
            }
        }
    }

    #[stable(feature = "io_safety", since = "1.63.0")]
    impl From<UnixListener> for OwnedFd {
        fn from(listener: UnixListener) -> OwnedFd {
            listener.fd.into_inner()
        }
    }

    #[derive(Debug)]
    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub struct Incoming<'a> {
        listener: &'a UnixListener,
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    impl<'a> Iterator for Incoming<'a> {
        type Item = io::Result<UnixStream>;

        fn next(&mut self) -> Option<io::Result<UnixStream>> {
            Some(self.listener.accept().map(|p| p.0))
        }
    }

    #[derive(Debug)]
    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub struct UnixDatagram(FileDesc);

    impl UnixDatagram {
        #[stable(feature = "unix_socket", since = "1.10.0")]
        pub fn unbound() -> io::Result<UnixDatagram> {
            let raw = syscall::socket(AF_UNIX, SOCK_DGRAM, 0).map_err(map_exo_err)?;
            Ok(UnixDatagram(unsafe { FileDesc::from_raw_fd(raw) }))
        }

        #[stable(feature = "unix_socket", since = "1.10.0")]
        pub fn pair() -> io::Result<(UnixDatagram, UnixDatagram)> {
            let [a, b] = syscall::socketpair(AF_UNIX, SOCK_DGRAM, 0).map_err(map_exo_err)?;
            unsafe {
                Ok((
                    UnixDatagram(FileDesc::from_raw_fd(a)),
                    UnixDatagram(FileDesc::from_raw_fd(b)),
                ))
            }
        }

        #[stable(feature = "unix_socket", since = "1.10.0")]
        pub fn bind<P: AsRef<Path>>(path: P) -> io::Result<UnixDatagram> {
            let addr = sockaddr_un_from_path(path.as_ref())?;
            let raw = syscall::socket(AF_UNIX, SOCK_DGRAM, 0).map_err(map_exo_err)?;
            let fd = unsafe { FileDesc::from_raw_fd(raw) };
            syscall::bind_unix(fd.as_raw_fd(), &addr).map_err(map_exo_err)?;
            Ok(UnixDatagram(fd))
        }

        #[stable(feature = "unix_socket", since = "1.10.0")]
        pub fn connect<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
            let addr = sockaddr_un_from_path(path.as_ref())?;
            syscall::connect_unix(self.0.as_raw_fd(), &addr).map_err(map_exo_err)
        }

        #[stable(feature = "unix_socket", since = "1.10.0")]
        pub fn send(&self, buf: &[u8]) -> io::Result<usize> {
            self.0.write(buf)
        }

        #[stable(feature = "unix_socket", since = "1.10.0")]
        pub fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
            self.0.read(buf)
        }

        #[stable(feature = "unix_socket", since = "1.10.0")]
        pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
            self.0.set_nonblocking(nonblocking)
        }

        #[stable(feature = "unix_socket", since = "1.10.0")]
        pub fn try_clone(&self) -> io::Result<UnixDatagram> {
            self.0.duplicate().map(UnixDatagram)
        }
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    impl AsFd for UnixDatagram {
        fn as_fd(&self) -> BorrowedFd<'_> {
            self.0.as_fd()
        }
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    impl AsRawFd for UnixDatagram {
        fn as_raw_fd(&self) -> RawFd {
            self.0.as_raw_fd()
        }
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    impl FromRawFd for UnixDatagram {
        unsafe fn from_raw_fd(fd: RawFd) -> UnixDatagram {
            UnixDatagram(unsafe { FileDesc::from_raw_fd(fd) })
        }
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    impl IntoRawFd for UnixDatagram {
        fn into_raw_fd(self) -> RawFd {
            self.0.into_raw_fd()
        }
    }

    #[stable(feature = "io_safety", since = "1.63.0")]
    impl From<OwnedFd> for UnixDatagram {
        fn from(fd: OwnedFd) -> UnixDatagram {
            UnixDatagram(FileDesc::from_inner(fd))
        }
    }

    #[stable(feature = "io_safety", since = "1.63.0")]
    impl From<UnixDatagram> for OwnedFd {
        fn from(sock: UnixDatagram) -> OwnedFd {
            sock.0.into_inner()
        }
    }

    impl AsInner<FileDesc> for UnixStream {
        #[inline]
        fn as_inner(&self) -> &FileDesc {
            &self.0
        }
    }
}

pub mod prelude {
    #![stable(feature = "rust1", since = "1.0.0")]
    #[doc(no_inline)]
    #[stable(feature = "rust1", since = "1.0.0")]
    pub use super::ffi::{OsStrExt, OsStringExt};
    #[doc(no_inline)]
    #[stable(feature = "rust1", since = "1.0.0")]
    pub use super::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
    #[doc(no_inline)]
    #[stable(feature = "rust1", since = "1.0.0")]
    pub use super::io::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
}
