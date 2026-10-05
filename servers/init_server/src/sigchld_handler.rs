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

    // FIX-SIGACTION-25 : la course #25 a ete partiellement adressee par les correctifs
    // precedents (vfork -> fork + CoW reserve aux pages vraiment inscriptibles +
    // liberation de la derniere reference CoW). On retablit maintenant sigsetsize = 8
    // (4eme argument de rt_sigaction) pour que SIGCHLD soit effectivement livre au
    // handler userspace et qu'init_server puisse reap ses enfants via wait4(WNOHANG).
    //
    // Sans cette correction, sys_rt_sigaction() retourne EINVAL car sigsetsize == 0,
    // les handlers ne sont PAS installes, et SIGCHLD tombe sur l'action par defaut
    // (Ignore). Init_server ne peut alors plus collecter les zombies, et la table
    // de services se desynchronise : c'est un pre-requis pour que la supervision
    // fonctionne et pour que le shell `exosh` soit atteint.
    //
    // Le noyau exige rigoureusement sigsetsize == 8 (cf. syscall/handlers/signal.rs
    // ligne 77 : `if sigsetsize != 8 { return EINVAL; }`), et l'ABI Linux x86_64
    // rt_sigaction(2) prend bien 4 arguments (signum, act, oldact, sigsetsize).
    let _ = syscall::syscall4(
        syscall::SYS_RT_SIGACTION,
        17,
        &chld_sa as *const Sigaction as u64,
        0,
        8, // sigsetsize — taille du champ sa_mask en bytes (1 * sizeof(u64) = 8)
    );
    let _ = syscall::syscall4(
        syscall::SYS_RT_SIGACTION,
        15,
        &term_sa as *const Sigaction as u64,
        0,
        8, // sigsetsize
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
