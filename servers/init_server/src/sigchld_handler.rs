use core::sync::atomic::{AtomicBool, Ordering};

use super::syscall;

static SIGCHLD_RECEIVED: AtomicBool = AtomicBool::new(false);
static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

// Layout ABI noyau (rt_sigaction) : handler@0, flags@8, restorer@16, mask@24.
// Le champ `restorer` est OBLIGATOIRE : sans lui, le noyau lit restorer=0 et le
// `ret` du handler saute à 0 (SEGV). cf. exo_syscall_abi::sigreturn_trampoline.
#[repr(C)]
struct Sigaction {
    handler: u64,
    flags: u64,
    restorer: u64,
    mask: u64,
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
        flags: syscall::SA_RESTART | syscall::SA_RESTORER,
        restorer: syscall::sigreturn_trampoline(),
        mask: 0,
    };
    let term_sa = Sigaction {
        handler: sigterm_handler as *const () as u64,
        flags: syscall::SA_RESTART | syscall::SA_RESTORER,
        restorer: syscall::sigreturn_trampoline(),
        mask: 0,
    };

    let _ = syscall::syscall3(
        syscall::SYS_RT_SIGACTION,
        17,
        &chld_sa as *const Sigaction as u64,
        0,
    );
    let _ = syscall::syscall3(
        syscall::SYS_RT_SIGACTION,
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
