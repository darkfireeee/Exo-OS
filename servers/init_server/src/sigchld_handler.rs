use core::sync::atomic::{AtomicBool, Ordering};

use super::syscall;

static SIGCHLD_RECEIVED: AtomicBool = AtomicBool::new(false);
static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

#[repr(C)]
struct Sigaction {
    handler: u64,
    flags: u64,
    mask: [u64; 2],
}

extern "C" fn sigchld_handler(_sig: i32) {
    SIGCHLD_RECEIVED.store(true, Ordering::Release);
}

extern "C" fn sigterm_handler(_sig: i32) {
    SHUTDOWN_REQUESTED.store(true, Ordering::Release);
}

pub unsafe fn install_handlers() {
    let chld_sa = Sigaction {
        handler: sigchld_handler as *const () as u64,
        flags: syscall::SA_RESTART,
        mask: [0; 2],
    };
    let term_sa = Sigaction {
        handler: sigterm_handler as *const () as u64,
        flags: syscall::SA_RESTART,
        mask: [0; 2],
    };

    let _ = syscall::syscall3(
        syscall::SYS_SIGACTION,
        17,
        &chld_sa as *const Sigaction as u64,
        0,
    );
    let _ = syscall::syscall3(
        syscall::SYS_SIGACTION,
        15,
        &term_sa as *const Sigaction as u64,
        0,
    );
}

#[inline]
pub fn take_sigchld() -> bool {
    SIGCHLD_RECEIVED.swap(false, Ordering::AcqRel)
}

#[inline]
pub fn shutdown_requested() -> bool {
    SHUTDOWN_REQUESTED.load(Ordering::Acquire)
}
