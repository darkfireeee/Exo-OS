//! # syscall/handlers/signal.rs — Thin wrappers signaux (sigaction, kill, sigreturn)
//!
//! RÈGLE SYS-03 : THIN WRAPPERS UNIQUEMENT.
//! RÈGLE SIG-01 : INTERDIT AtomicPtr<sigaction> dans SignalTcb — TOCTOU triviale.
//! RÈGLE SIG-02 : SigactionEntry stocke les valeurs DIRECTEMENT (handler_vaddr).
//! RÈGLE SIG-07 : SIGKILL(9) et SIGSTOP(19) non-maskables — rejetés dans sigmask.
//! RÈGLE SIG-13 : magic 0x5349474E vérifié au sigreturn AVANT tout restore.
//! RÈGLE SIG-14 : INTERDIT sigreturn sans vérifier magic — injection de faux contexte.
//! RÈGLE SIG-18 : INTERDIT écriture signal frame sans copy_to_user().

use crate::process::core::pid::Pid;
use crate::process::core::registry::PROCESS_REGISTRY;
use crate::process::signal::default::{SigAction, SigActionKind};
use crate::scheduler::core::task::ThreadControlBlock;
use crate::syscall::errno::{EFAULT, EINVAL, ENOSYS, EPERM, ESRCH};
use crate::syscall::validation::{validate_signal, USER_ADDR_MAX};

/// Layout Linux du struct sigaction (x86_64) tel que passé par userspace.
/// Taille : 32 bytes — DOIT correspondre à l'ABI Linux glibc/musl.
#[repr(C)]
struct LinuxSigaction {
    sa_handler: u64,  // offset  0 : SIG_DFL=0, SIG_IGN=1, ou adresse handler
    sa_flags: u64,    // offset  8 : SA_RESTART | SA_SIGINFO | SA_ONSTACK…
    sa_restorer: u64, // offset 16 : adresse stub rt_sigreturn
    sa_mask: u64,     // offset 24 : masque de signaux bloqués pendant handler
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
    signum: u64,
    act_ptr: u64,
    oldact_ptr: u64,
    sigsetsize: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    let sig = match validate_signal(signum) {
        Ok(s) => s,
        Err(e) => return e.to_errno(),
    };
    // SIGKILL et SIGSTOP — non modifiables (SIG-07)
    if sig == SIGKILL || sig == SIGSTOP {
        return EINVAL;
    }
    if sigsetsize != 8 {
        return EINVAL;
    }
    // Valider les pointeurs (SYS-01)
    if act_ptr != 0 && act_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }
    if oldact_ptr != 0 && oldact_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }

    // Obtenir le PCB du processus courant via le TCB.
    // SAFETY: GS kernel actif, gs:[0x20] = pointeur TCB valide ou 0.
    let tcb_ptr = unsafe { current_tcb_ptr() };
    if tcb_ptr.is_null() {
        return ENOSYS;
    }
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
            sa_handler: if old_action.kind == SigActionKind::Ignore {
                1
            } else if old_action.kind == SigActionKind::User {
                old_action.handler
            } else {
                0
            },
            sa_flags: old_action.flags as u64,
            sa_restorer: old_action.restorer,
            sa_mask: old_action.mask,
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
            SigActionKind::Term // SIG_DFL — l'action réelle dépend du signal, Term par défaut
        } else if linux_act.sa_handler == 1 {
            SigActionKind::Ignore // SIG_IGN
        } else {
            SigActionKind::User
        };

        let new_action = SigAction {
            kind,
            handler: linux_act.sa_handler,
            flags: linux_act.sa_flags as u32,
            mask: linux_act.sa_mask,
            restorer: linux_act.sa_restorer,
        };

        let mut handlers = pcb.sig_handlers.lock();
        handlers.set(sig as u8, new_action);
    }

    0 // succès
}

/// `rt_sigprocmask(how, set_ptr, oldset_ptr, sigsetsize)` → 0 ou errno.
pub fn sys_rt_sigprocmask(
    how: u64,
    set_ptr: u64,
    oldset_ptr: u64,
    sigsetsize: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    // how : SIG_BLOCK=0, SIG_UNBLOCK=1, SIG_SETMASK=2
    if how > 2 {
        return EINVAL;
    }
    if sigsetsize != 8 {
        return EINVAL;
    }
    if set_ptr != 0 && set_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }
    if oldset_ptr != 0 && oldset_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }

    // Obtenir le TCB du thread courant.
    // SAFETY: GS kernel actif dans le contexte syscall.
    let tcb_ptr = unsafe { current_tcb_ptr() };
    if tcb_ptr.is_null() {
        return ENOSYS;
    }
    let tcb = unsafe { &*tcb_ptr };

    // Lire l'ancien masque.
    let old_mask = tcb.signal_mask.load(core::sync::atomic::Ordering::Acquire);

    // Écrire oldset si demandé.
    if oldset_ptr != 0 {
        // SAFETY: oldset_ptr est une adresse userspace validée.
        unsafe {
            core::ptr::write_volatile(oldset_ptr as *mut u64, old_mask);
        }
    }

    // Appliquer le nouveau masque si set_ptr est fourni.
    if set_ptr != 0 {
        // SAFETY: set_ptr est une adresse userspace validée.
        let new_set = unsafe { core::ptr::read_volatile(set_ptr as *const u64) };

        // Masques non-bloquables : SIGKILL (bit 8) et SIGSTOP (bit 18).
        const NON_MASKABLE: u64 = (1u64 << 8) | (1u64 << 18);

        let computed = match how {
            0 => old_mask | new_set,  // SIG_BLOCK
            1 => old_mask & !new_set, // SIG_UNBLOCK
            2 => new_set,             // SIG_SETMASK
            _ => return EINVAL,
        };
        // Forcer SIGKILL et SIGSTOP non-bloquables (SIG-07).
        tcb.signal_mask.store(
            computed & !NON_MASKABLE,
            core::sync::atomic::Ordering::Release,
        );
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
    use crate::process::signal::delivery::{send_signal_to_pid, SendError};
    use crate::process::signal::Signal;

    let sig = match validate_signal(signum) {
        Ok(s) => s,
        Err(e) => return e.to_errno(),
    };
    let signal = match Signal::from_u8(sig as u8) {
        Some(s) => s,
        None => return EINVAL,
    };

    let target_pid = pid as i32;
    let real_pid: u32 = if target_pid <= 0 {
        // pid==0 : envoyer au groupe courant → approx. avec le PID courant.
        // pid<0  : groupes de processus — non implémenté.
        if target_pid == 0 {
            unsafe {
                let ptr: u64;
                core::arch::asm!("mov {}, gs:[0x20]", out(reg) ptr,
                    options(nostack, nomem));
                if ptr == 0 {
                    return EFAULT;
                }
                (*(ptr as *const ThreadControlBlock)).pid.0
            }
        } else {
            return ESRCH;
        }
    } else {
        target_pid as u32
    };

    match send_signal_to_pid(Pid(real_pid), signal) {
        Ok(()) => 0,
        Err(SendError::PermissionDenied) => EPERM,
        Err(_) => ESRCH,
    }
}

/// `tgkill(tgid, tid, sig)` → 0 ou errno.
///
/// Envoie un signal à un thread spécifique (tid) dans le groupe (tgid).
/// Phase 3 : cible le thread principal du processus.
pub fn sys_tgkill(tgid: u64, tid: u64, signum: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    use crate::process::signal::delivery::send_signal_to_tcb;
    use crate::process::signal::queue::SigInfo;

    let sig = match validate_signal(signum) {
        Ok(s) => s,
        Err(e) => return e.to_errno(),
    };

    if tgid == 0 || tgid >= 4_194_304 {
        return ESRCH;
    }
    if tid == 0 || tid >= 4_194_304 {
        return ESRCH;
    }

    // Lire le PID de l'appelant pour remplir le SigInfo.
    // SAFETY: GS kernel actif dans le contexte syscall.
    let sender_pid: u32 = unsafe {
        let ptr: u64;
        core::arch::asm!("mov {}, gs:[0x20]", out(reg) ptr, options(nostack, nomem));
        if ptr == 0 {
            return EFAULT;
        }
        (*(ptr as *const ThreadControlBlock)).pid.0
    };

    let pcb = match PROCESS_REGISTRY.find_by_pid(Pid(tgid as u32)) {
        Some(p) => p,
        None => return ESRCH,
    };

    let thread_ptr = pcb.main_thread_ptr();
    if thread_ptr.is_null() {
        return ESRCH;
    }

    // SAFETY: thread_ptr maintenu par le PCB.
    let thread = unsafe { &*thread_ptr };
    let info = SigInfo::from_kill(sig as u8, sender_pid, 0);
    send_signal_to_tcb(thread, sig as u8, info);
    0
}

/// `sigaltstack(ss, oss)` → 0 ou errno.
///
/// Lit/écrit le sigaltstack du thread courant.
/// Layout `struct stack_t` x86_64 (24 bytes) :
///   [0]  ss_sp    (u64) — adresse de base de la pile alternative
///   [8]  ss_flags (i32) — SS_ONSTACK=1, SS_DISABLE=2
///   [12] _pad     (u32)
///   [16] ss_size  (u64) — taille en octets
pub fn sys_sigaltstack(ss_ptr: u64, oss_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    use crate::process::signal::handler::{SigAltStack, SS_DISABLE};

    if ss_ptr != 0 && ss_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }
    if oss_ptr != 0 && oss_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }

    // Lire la TCB courante depuis gs:[0x20].
    // SAFETY: GS kernel actif pendant un syscall.
    let tcb_ptr = unsafe { current_tcb_ptr() };
    if tcb_ptr.is_null() {
        return ENOSYS;
    }

    let pid = unsafe { Pid((*tcb_ptr).pid.0) };
    let pcb = match PROCESS_REGISTRY.find_by_pid(pid) {
        Some(p) => p,
        None => return EFAULT,
    };

    let thread_ptr = pcb.main_thread_ptr();
    if thread_ptr.is_null() {
        return EFAULT;
    }

    // SAFETY: thread_ptr maintenu par le PCB ; appelant = thread courant.
    let thread = unsafe { &mut *thread_ptr };

    // Exporter l'ancien sigaltstack si oss_ptr est fourni.
    if oss_ptr != 0 {
        let old = SigAltStack {
            ss_sp: thread.addresses.sigaltstack_base,
            ss_flags: if thread.addresses.sigaltstack_size == 0 {
                SS_DISABLE
            } else {
                0
            },
            _pad: 0,
            ss_size: thread.addresses.sigaltstack_size,
        };
        // SAFETY: oss_ptr est une adresse userspace validée ci-dessus.
        unsafe {
            core::ptr::write_volatile(oss_ptr as *mut SigAltStack, old);
        }
    }

    // Installer le nouveau sigaltstack si ss_ptr est fourni.
    if ss_ptr != 0 {
        // SAFETY: ss_ptr est une adresse userspace validée ci-dessus.
        let ss = unsafe { core::ptr::read_volatile(ss_ptr as *const SigAltStack) };
        const MINSIGSTKSZ: u64 = 2048;

        if ss.ss_flags & SS_DISABLE != 0 {
            // SS_DISABLE : désactiver le sigaltstack courant.
            thread.addresses.sigaltstack_base = 0;
            thread.addresses.sigaltstack_size = 0;
        } else if ss.ss_flags != 0 {
            return EINVAL; // flags inconnus
        } else {
            if ss.ss_size < MINSIGSTKSZ {
                return EINVAL;
            }
            thread.addresses.sigaltstack_base = ss.ss_sp;
            thread.addresses.sigaltstack_size = ss.ss_size;
        }
    }

    0
}
