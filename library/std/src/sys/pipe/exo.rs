use crate::io;
use crate::os::fd::FromRawFd;
use crate::sys::fd::FileDesc;
use crate::sys::pal::syscall::{self, O_CLOEXEC};
use crate::sys::pal::map_exo_err;

pub type Pipe = FileDesc;

#[inline]
pub fn pipe() -> io::Result<(Pipe, Pipe)> {
    let mut fds = [-1i32; 2];
    syscall::pipe2(&mut fds, O_CLOEXEC).map_err(map_exo_err)?;
    Ok(unsafe { (FileDesc::from_raw_fd(fds[0]), FileDesc::from_raw_fd(fds[1])) })
}
