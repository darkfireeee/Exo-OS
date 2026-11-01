use crate::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use crate::sys::pal::{map_exo_err, syscall};
use crate::sys::{AsInner, FromInner, IntoInner};
use crate::{io, process, sys};

pub const STDIN_BUF_SIZE: usize = crate::sys::io::DEFAULT_BUF_SIZE;

pub struct Stdin {}

impl Stdin {
    pub const fn new() -> Self {
        Self {}
    }
}

pub struct Stdout {}

impl Stdout {
    pub const fn new() -> Self {
        Self {}
    }
}

pub struct Stderr {}

impl Stderr {
    pub const fn new() -> Self {
        Self {}
    }
}

impl io::Read for Stdin {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        syscall::read(0, buf).map_err(map_exo_err)
    }
}

impl io::Write for Stdout {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        syscall::write(1, buf).map_err(map_exo_err)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl io::Write for Stderr {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match syscall::write(2, buf) {
            Ok(n) => Ok(n),
            Err(_) => {
                syscall::exo_log(buf);
                Ok(buf.len())
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub fn panic_output() -> Option<impl io::Write> {
    Some(Stderr::new())
}

pub fn is_ebadf(err: &io::Error) -> bool {
    err.raw_os_error() == Some(9)
}

#[stable(feature = "process_extensions", since = "1.2.0")]
impl FromRawFd for process::Stdio {
    #[inline]
    unsafe fn from_raw_fd(fd: RawFd) -> process::Stdio {
        let fd = unsafe { sys::fd::FileDesc::from_raw_fd(fd) };
        let io = sys::process::Stdio::Fd(fd);
        process::Stdio::from_inner(io)
    }
}

#[stable(feature = "io_safety", since = "1.63.0")]
impl From<OwnedFd> for process::Stdio {
    #[inline]
    fn from(fd: OwnedFd) -> process::Stdio {
        let fd = sys::fd::FileDesc::from_inner(fd);
        let io = sys::process::Stdio::Fd(fd);
        process::Stdio::from_inner(io)
    }
}

#[stable(feature = "process_extensions", since = "1.2.0")]
impl AsRawFd for process::ChildStdin {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        self.as_inner().as_raw_fd()
    }
}

#[stable(feature = "process_extensions", since = "1.2.0")]
impl AsRawFd for process::ChildStdout {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        self.as_inner().as_raw_fd()
    }
}

#[stable(feature = "process_extensions", since = "1.2.0")]
impl AsRawFd for process::ChildStderr {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        self.as_inner().as_raw_fd()
    }
}

#[stable(feature = "into_raw_os", since = "1.4.0")]
impl IntoRawFd for process::ChildStdin {
    #[inline]
    fn into_raw_fd(self) -> RawFd {
        self.into_inner().into_raw_fd()
    }
}

#[stable(feature = "into_raw_os", since = "1.4.0")]
impl IntoRawFd for process::ChildStdout {
    #[inline]
    fn into_raw_fd(self) -> RawFd {
        self.into_inner().into_raw_fd()
    }
}

#[stable(feature = "into_raw_os", since = "1.4.0")]
impl IntoRawFd for process::ChildStderr {
    #[inline]
    fn into_raw_fd(self) -> RawFd {
        self.into_inner().into_raw_fd()
    }
}

#[stable(feature = "io_safety", since = "1.63.0")]
impl AsFd for crate::process::ChildStdin {
    #[inline]
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.as_inner().as_fd()
    }
}

#[stable(feature = "io_safety", since = "1.63.0")]
impl From<crate::process::ChildStdin> for OwnedFd {
    #[inline]
    fn from(child_stdin: crate::process::ChildStdin) -> OwnedFd {
        child_stdin.into_inner().into_inner()
    }
}

#[stable(feature = "child_stream_from_fd", since = "1.74.0")]
impl From<OwnedFd> for process::ChildStdin {
    #[inline]
    fn from(fd: OwnedFd) -> process::ChildStdin {
        let pipe = sys::process::ChildPipe::from_inner(fd);
        process::ChildStdin::from_inner(pipe)
    }
}

#[stable(feature = "io_safety", since = "1.63.0")]
impl AsFd for crate::process::ChildStdout {
    #[inline]
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.as_inner().as_fd()
    }
}

#[stable(feature = "io_safety", since = "1.63.0")]
impl From<crate::process::ChildStdout> for OwnedFd {
    #[inline]
    fn from(child_stdout: crate::process::ChildStdout) -> OwnedFd {
        child_stdout.into_inner().into_inner()
    }
}

#[stable(feature = "child_stream_from_fd", since = "1.74.0")]
impl From<OwnedFd> for process::ChildStdout {
    #[inline]
    fn from(fd: OwnedFd) -> process::ChildStdout {
        let pipe = sys::process::ChildPipe::from_inner(fd);
        process::ChildStdout::from_inner(pipe)
    }
}

#[stable(feature = "io_safety", since = "1.63.0")]
impl AsFd for crate::process::ChildStderr {
    #[inline]
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.as_inner().as_fd()
    }
}

#[stable(feature = "io_safety", since = "1.63.0")]
impl From<crate::process::ChildStderr> for OwnedFd {
    #[inline]
    fn from(child_stderr: crate::process::ChildStderr) -> OwnedFd {
        child_stderr.into_inner().into_inner()
    }
}

#[stable(feature = "child_stream_from_fd", since = "1.74.0")]
impl From<OwnedFd> for process::ChildStderr {
    #[inline]
    fn from(fd: OwnedFd) -> process::ChildStderr {
        let pipe = sys::process::ChildPipe::from_inner(fd);
        process::ChildStderr::from_inner(pipe)
    }
}
