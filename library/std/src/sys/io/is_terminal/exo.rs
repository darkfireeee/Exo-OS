use crate::os::fd::AsFd;
use crate::os::fd::AsRawFd;
use crate::sys::pal::syscall::{self, TIOCGWINSZ, Winsize};

pub fn is_terminal(fd: &impl AsFd) -> bool {
    let raw = fd.as_fd().as_raw_fd();
    let mut ws = Winsize::default();
    syscall::ioctl(raw, TIOCGWINSZ, &mut ws as *mut Winsize as u64).is_ok()
}
