use crate::os::fd::AsFd;
use crate::os::fd::AsRawFd;

pub fn is_terminal(fd: &impl AsFd) -> bool {
    let raw = fd.as_fd().as_raw_fd();
    (0..=2).contains(&raw)
}
