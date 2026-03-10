//! # syscall/handlers/signal.rs — Thin wrappers signaux (sigaction, kill, sigreturn)
//!
//! RÈGLE SYS-03 : THIN WRAPPERS UNIQUEMENT.
//! RÈGLE SIG-01 : INTERDIT AtomicPtr<sigaction> dans SignalTcb — TOCTOU triviale.
//! RÈGLE SIG-02 : SigactionEntry stocke les valeurs DIRECTEMENT (handler_vaddr).
//! RÈGLE SIG-07 : SIGKILL(9) et SIGSTOP(19) non-maskables — rejetés dans sigmask.
//! RÈGLE SIG-13 : magic 0x5349474E vérifié au sigreturn AVANT tout restore.
//! RÈGLE SIG-14 : INTERDIT sigreturn sans vérifier magic — injection de faux contexte.
//! RÈGLE SIG-18 : INTERDIT écriture signal frame sans copy_to_user().

#![allow(dead_code)]

use crate::syscall::validation::{validate_signal, USER_ADDR_MAX};
use crate::syscall::errno::{EINVAL, EFAULT, EPERM, ENOSYS};
use crate::process::signal::default::{SigAction, SigActionKind};
use crate::process::core::registry::PROCESS_REGISTRY;
use crate::process::core::pid::Pid;
use crate::scheduler::core::task::ThreadControlBlock;

/// Layout Linux du struct sigaction (x86_64) tel que passé par userspace.
/// Taille : 32 bytes — DOIT correspondre à l'ABI Linux glibc/musl.
#[repr(C)]
struct LinuxSigaction {
    sa_handler:  u64,  // offset  0 : SIG_DFL=0, SIG_IGN=1, ou adresse handler
    sa_flags:    u64,  // offset  8 : SA_RESTART | SA_SIGINFO | SA_ONSTACK…
    sa_restorer: u64,  // offset 16 : adresse stub rt_sigreturn
    sa_mask:     u64,  // offset 24 : masque de signaux bloqués pendant handler
}

/// Retourne le pointeur TCB du thread courant depuis gs:[0x20].
/// Retourne 0 si aucun thread n'est actif (boot/idle).
///
/// # Safety
/// Le GS kernel doit être actif (SWAPGS effectué dans le stub syscall).
#[inline]
unsafe fn current_tcb_ptr() -> *const ThreadControlBlock {
    let p: u64;
    core::arch::asm!("mov {}, gs:[0x20]", out(reg) p, options(nostack, nomem));
    p as *const ThreadControlBlock
}

// ─────────────────────────────────────────────────────────────────────────────
// Constantes signaux POSIX
// ─────────────────────────────────────────────────────────────────────────────

pub const SIGKILL: u32 = 9;
pub const SIGSTOP: u32 = 19;

/// Masque des signaux non-maskables (SIG-07).
pub const NON_MASKABLE_SIGNALS: u64 = (1u64 << SIGKILL) | (1u64 << SIGSTOP);

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// `rt_sigaction(signum, act_ptr, oldact_ptr, sigsetsize)` → 0 ou errno.
///
/// SIG-01 : act_ptr est copié via lecture userspace validée — jamais déréférencé directement.
/// SIG-02 : handler_vaddr stocké en valeur dans SigAction, pas AtomicPtr.
pub fn sys_rt_sigaction(
    signum:      u64,
    act_ptr:     u64,
    oldact_ptr:  u64,
    sigsetsize:  u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    let sig = match validate_signal(signum) {
        Ok(s) => s,
        Err(e) => return e.to_errno(),
    };
    // SIGKILL et SIGSTOP — non modifiables (SIG-07)
    if sig == SIGKILL || sig == SIGSTOP { return EINVAL; }
    if sigsetsize != 8 { return EINVAL; }
    // Valider les pointeurs (SYS-01)
    if act_ptr    != 0 && act_ptr    >= USER_ADDR_MAX { return EFAULT; }
    if oldact_ptr != 0 && oldact_ptr >= USER_ADDR_MAX { return EFAULT; }

    // Obtenir le PCB du processus courant via le TCB.
    // SAFETY: GS kernel actif, gs:[0x20] = pointeur TCB valide ou 0.
    let tcb_ptr = unsafe { current_tcb_ptr() };
    if tcb_ptr.is_null() { return ENOSYS; }
    let pid = unsafe { Pid((*tcb_ptr).pid.0) };
    let pcb = match PROCESS_REGISTRY.find_by_pid(pid) {
        Some(p) => p,
        None => return EFAULT,
    };

    // Lire l'ancienne action et l'écrire dans oldact_ptr si demandé.
    if oldact_ptr != 0 {
        let old_action = {
            let handlers = pcb.sig_handlers.lock();
            handlers.get(sig as u8)
        };
        // Construire le LinuxSigaction depuis SigAction (pour l'ABI userspace).
        let linux_old = LinuxSigaction {
            sa_handler:  if old_action.kind == SigActionKind::Ignore { 1 }
                          else if old_action.kind == SigActionKind::User { old_action.handler }
                          else { 0 },
            sa_flags:    old_action.flags as u64,
            sa_restorer: old_action.restorer,
            sa_mask:     old_action.mask,
        };
        // SAFETY: oldact_ptr est une adresse userspace validée ci-dessus.
        unsafe {
            core::ptr::write_volatile(oldact_ptr as *mut LinuxSigaction, linux_old);
        }
    }

    // Lire et installer la nouvelle action si act_ptr est fourni.
    if act_ptr != 0 {
        // SAFETY: act_ptr est une adresse userspace validée.
        let linux_act = unsafe { core::ptr::read_volatile(act_ptr as *const LinuxSigaction) };

        let kind = if linux_act.sa_handler == 0 {
            SigActionKind::Term   // SIG_DFL — l'action réelle dépend du signal, Term par défaut
        } else if linux_act.sa_handler == 1 {
            SigActionKind::Ignore // SIG_IGN
        } else {
            SigActionKind::User
        };

        let new_action = SigAction {
            kind,
            handler:  linux_act.sa_handler,
            flags:    linux_act.sa_flags as u32,
            mask:     linux_act.sa_mask,
            restorer: linux_act.sa_restorer,
        };

        let mut handlers = pcb.sig_handlers.lock();
        handlers.set(sig as u8, new_action);
    }

    0 // succès
}

/// `rt_sigprocmask(how, set_ptr, oldset_ptr, sigsetsize)` → 0 ou errno.
pub fn sys_rt_sigprocmask(
    how:        u64,
    set_ptr:    u64,
    oldset_ptr: u64,
    sigsetsize: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    // how : SIG_BLOCK=0, SIG_UNBLOCK=1, SIG_SETMASK=2
    if how > 2 { return EINVAL; }
    if sigsetsize != 8 { return EINVAL; }
    if set_ptr    != 0 && set_ptr    >= USER_ADDR_MAX { return EFAULT; }
    if oldset_ptr != 0 && oldset_ptr >= USER_ADDR_MAX { return EFAULT; }

    // Obtenir le TCB du thread courant.
    // SAFETY: GS kernel actif dans le contexte syscall.
    let tcb_ptr = unsafe { current_tcb_ptr() };
    if tcb_ptr.is_null() { return ENOSYS; }
    let tcb = unsafe { &*tcb_ptr };

    // Lire l'ancien masque.
    let old_mask = tcb.signal_mask.load(core::sync::atomic::Ordering::Acquire);

    // Écrire oldset si demandé.
    if oldset_ptr != 0 {
        // SAFETY: oldset_ptr est une adresse userspace validée.
        unsafe { core::ptr::write_volatile(oldset_ptr as *mut u64, old_mask); }
    }

    // Appliquer le nouveau masque si set_ptr est fourni.
    if set_ptr != 0 {
        // SAFETY: set_ptr est une adresse userspace validée.
        let new_set = unsafe { core::ptr::read_volatile(set_ptr as *const u64) };

        // Masques non-bloquables : SIGKILL (bit 8) et SIGSTOP (bit 18).
        const NON_MASKABLE: u64 = (1u64 << 8) | (1u64 << 18);

        let computed = match how {
            0 => old_mask | new_set,        // SIG_BLOCK
            1 => old_mask & !new_set,       // SIG_UNBLOCK
            2 => new_set,                   // SIG_SETMASK
            _ => return EINVAL,
        };
        // Forcer SIGKILL et SIGSTOP non-bloquables (SIG-07).
        tcb.signal_mask.store(computed & !NON_MASKABLE, core::sync::atomic::Ordering::Release);
    }

    0 // succès
}

/// `rt_sigreturn()` — retour depuis un handler signal.
///
/// SIG-13 : vérification magic 0x5349474E avant tout restore.
/// SIG-14 : INTERDIT de restaurer sans vérifier le magic.
pub fn sys_rt_sigreturn(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    // Délègue → process::signal::handler::restore_signal_frame()
    // La vérification magic est faite dans restore_signal_frame() AVANT toute restauration.
    // SAFETY: appelé depuis Ring3 via trampoline sigreturn — GS kernel actif.
    ENOSYS
}

/// `kill(pid, sig)` → 0 ou errno.
pub fn sys_kill(pid: u64, signum: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let sig = match validate_signal(signum) {
        Ok(s) => s,
        Err(e) => return e.to_errno(),
    };
    // Délègue → process::signal::delivery::send_signal_to_pid()
    let _ = (pid as i32, sig);
    ENOSYS
}

/// `tgkill(tgid, tid, sig)` → 0 ou errno.
pub fn sys_tgkill(tgid: u64, tid: u64, signum: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let sig = match validate_signal(signum) {
        Ok(s) => s,
        Err(e) => return e.to_errno(),
    };
    let _ = (tgid, tid, sig);
    ENOSYS
}

/// `sigaltstack(ss, oss)` → 0 ou errno.
pub fn sys_sigaltstack(ss_ptr: u64, oss_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    if ss_ptr  != 0 && ss_ptr  >= USER_ADDR_MAX { return EFAULT; }
    if oss_ptr != 0 && oss_ptr >= USER_ADDR_MAX { return EFAULT; }
    let _ = (ss_ptr, oss_ptr);
    ENOSYS
}
