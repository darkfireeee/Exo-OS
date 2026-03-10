// kernel/src/process/thread/pthread_compat.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Interface pthread POSIX — wrappers kernel (Exo-OS Couche 1.5)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Ce module exporte les fonctions "pthread_*" qui seront appelées depuis
// la couche syscall (syscall/thread.rs) via les numéros syscall Linux :
//   sys_clone   → PTHREAD_CREATE
//   sys_exit    → PTHREAD_EXIT
//   sys_futex   → PTHREAD_MTX impl (mutex/condvar)
//

#![allow(non_snake_case)]
// CODES DE RETOUR :
//   0         = succès
//   EAGAIN     = ressources temporairement indisponibles
//   EPERM     = permission refusée
//   EINVAL    = argument invalide
//   EDEADLK   = détection de deadlock
// ═══════════════════════════════════════════════════════════════════════════════


use core::sync::atomic::{AtomicU32, Ordering};
use crate::process::core::pcb::ProcessControlBlock;
use crate::process::core::tcb::ProcessThread;
use crate::process::thread::creation::{create_thread, ThreadCreateParams, ThreadAttr};
use crate::process::thread::join::thread_join;
use crate::process::thread::detach::thread_detach;
use crate::scheduler::sync::wait_queue::WaitQueue;

/// File d'attente globale pour les mutex pthread (slow path).
static MUTEX_WQ: WaitQueue = WaitQueue::new();

// ─────────────────────────────────────────────────────────────────────────────
// Codes d'erreur POSIX (sous-ensemble)
// ─────────────────────────────────────────────────────────────────────────────

pub const ESUCCESS: i32 = 0;
pub const EAGAIN:   i32 = 11;
pub const ENOMEM:   i32 = 12;
pub const EBUSY:    i32 = 16;
pub const EINVAL:   i32 = 22;
pub const EDEADLK:  i32 = 35;
pub const ETIMEDOUT:i32 = 110;

// ─────────────────────────────────────────────────────────────────────────────
// PTHREAD_CREATE — syscall clone() wrapper
// ─────────────────────────────────────────────────────────────────────────────

/// Implémentation kernel de pthread_create().
///
/// # Arguments
/// `pcb`          — PCB du processus appelant.
/// `start_func`   — adresse userspace du point d'entrée.
/// `arg`          — argument passé au thread.
/// `stack_ptr`    — adresse de la pile fournie par pthread_create() (userspace).
/// `stack_size`   — taille de la pile en bytes.
/// `pthread_out`  — adresse userspace de la structure pthread_t de sortie.
/// `target_cpu`   — CPU cible ( affiné au CPU courant par défaut).
///
/// # Returns
/// 0 en cas de succès, ou code errno négatif.
///
/// # Safety
/// `pcb` doit pointer vers le PCB du thread appelant.
pub fn PTHREAD_CREATE(
    pcb:        *const ProcessControlBlock,
    start_func: u64,
    arg:        u64,
    stack_ptr:  u64,
    stack_size: u64,
    pthread_out: u64,
    target_cpu: u32,
) -> i32 {
    let params = ThreadCreateParams {
        pcb,
        attr: ThreadAttr {
            stack_size,
            stack_addr: stack_ptr,
            policy:     crate::scheduler::core::task::SchedPolicy::Normal,
            priority:   crate::scheduler::core::task::Priority::NORMAL_DEFAULT,
            detached:   false,
            cpu_affinity: -1,
            sigaltstack_size: 0,
        },
        start_func,
        arg,
        target_cpu,
        pthread_out,
    };
    match create_thread(&params) {
        Ok(handle) => {
            // Écrire le TID dans la struct pthread_t (offset 0).
            if pthread_out != 0 {
                // SAFETY: pthread_out = adresse userspace validée par le syscall avant cet appel.
                unsafe {
                    let p = pthread_out as *mut u32;
                    *p = handle.tid.0;
                }
            }
            ESUCCESS
        }
        Err(e) => match e {
            crate::process::thread::creation::ThreadCreateError::TidExhausted   => -EAGAIN,
            crate::process::thread::creation::ThreadCreateError::OutOfMemory    => -ENOMEM,
            crate::process::thread::creation::ThreadCreateError::ProcessExiting => -EINVAL,
            _ => -EINVAL,
        },
    }
}

/// Implémentation kernel de pthread_join().
///
/// # Safety
/// `thread_ptr` doit pointer vers un ProcessThread valide.
/// `caller_tcb` doit être le TCB du thread appelant.
pub fn PTHREAD_JOIN(
    thread_ptr:  *const ProcessThread,
    caller_tcb:  &crate::scheduler::core::task::ThreadControlBlock,
    retval_out:  *mut u64,
) -> i32 {
    match thread_join(thread_ptr, caller_tcb) {
        Ok(val) => {
            if !retval_out.is_null() {
                // SAFETY: retval_out validé par le syscall.
                unsafe { *retval_out = val; }
            }
            ESUCCESS
        }
        Err(e) => match e {
            crate::process::thread::join::JoinError::Detached      => -EINVAL,
            crate::process::thread::join::JoinError::Interrupted   => -4, // EINTR
            _ => -EINVAL,
        },
    }
}

/// Implémentation kernel de pthread_detach().
pub fn PTHREAD_DETACH(thread_ptr: *mut ProcessThread) -> i32 {
    match thread_detach(thread_ptr) {
        Ok(_)  => ESUCCESS,
        Err(_) => -EINVAL,
    }
}

/// Retourne le TID du thread courant (pthread_self).
/// Appelé via GS:[0] en userspace ; ici implémenté pour le kernel bridge.
pub fn PTHREAD_SELF(thread: &ProcessThread) -> u64 {
    thread.tid.0 as u64
}

/// Implémentation kernel de pthread_exit() — appelle do_exit_thread.
/// Ne retourne JAMAIS.
///
/// # Safety
/// `thread` et `pcb` doivent correspondre au thread courant.
pub fn PTHREAD_EXIT(
    thread:     &mut ProcessThread,
    pcb:        &ProcessControlBlock,
    retval:     u64,
) -> ! {
    crate::process::lifecycle::exit::do_exit_thread(thread, pcb, retval)
}

// ─────────────────────────────────────────────────────────────────────────────
// PthreadMutex — mutex kernel simple (spinlock + wait_queue)
// ───────────────────────────────────────────────────────────────────────────══

/// Structure mutex partagée kernel/userspace.
/// Placée dans l'espace mémoire userspace, accédée via les syscalls futex.
#[repr(C, align(16))]
pub struct PthreadMutex {
    /// 0 = libre, 1 = acquis, 2 = acquis par des threads en attente.
    pub state:   AtomicU32,
    /// TID du détenteur courant (debugging).
    pub owner:   AtomicU32,
    /// Nombre de threads en attente.
    pub waiters: AtomicU32,
    /// Type : 0=NORMAL, 1=ERRORCHECK, 2=RECURSIVE.
    pub mtype:   u32,
    /// Compteur de verrouillage récursif (type RECURSIVE uniquement).
    pub recursive_count: AtomicU32,
    _pad: [u32; 3],
}

const _: () = assert!(core::mem::size_of::<PthreadMutex>() == 32, "PthreadMutex must be 32B");

/// Initialise un PthreadMutex.
///
/// # Safety
/// `mutex_ptr` = adresse userspace d'un PthreadMutex validé par le syscall.
pub unsafe fn PTHREAD_MUTEX_INIT(
    mutex_ptr: *mut PthreadMutex,
    mtype:     u32,
) -> i32 {
    if mutex_ptr.is_null() { return -EINVAL; }
    // SAFETY: mutex_ptr validé par le syscall via vérification userspace.
    (*mutex_ptr).state.store(0, Ordering::Release);
    (*mutex_ptr).owner.store(0, Ordering::Relaxed);
    (*mutex_ptr).waiters.store(0, Ordering::Relaxed);
    (*(mutex_ptr as *mut u32).add(3)) = mtype;
    (*mutex_ptr).recursive_count.store(0, Ordering::Relaxed);
    ESUCCESS
}

/// Verrouille un PthreadMutex (bloquant si déjà tenu).
///
/// # Safety
/// `mutex_ptr` = adresse userspace d'un PthreadMutex validé, `owner_tid` = TID du thread appelant.
pub unsafe fn PTHREAD_MUTEX_LOCK(
    mutex_ptr:  *mut PthreadMutex,
    owner_tid:  u32,
    caller_tcb: &crate::scheduler::core::task::ThreadControlBlock,
) -> i32 {
    if mutex_ptr.is_null() { return -EINVAL; }
    let m = &*mutex_ptr;
    // Fast path : CAS 0 → 1.
    if m.state.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_ok() {
        m.owner.store(owner_tid, Ordering::Relaxed);
        return ESUCCESS;
    }
    // Détection ERRORCHECK : déjà détenteur.
    if m.mtype == 1 && m.owner.load(Ordering::Relaxed) == owner_tid {
        return -EDEADLK;
    }
    // Slow path : attente sur futex (impl simplifiée via wait_queue).
    m.waiters.fetch_add(1, Ordering::Relaxed);
    loop {
        if caller_tcb.has_signal_pending() {
            m.waiters.fetch_sub(1, Ordering::Relaxed);
            return -4; // EINTR
        }
        if m.state.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_ok() {
            m.owner.store(owner_tid, Ordering::Relaxed);
            m.waiters.fetch_sub(1, Ordering::Relaxed);
            return ESUCCESS;
        }
        // SAFETY: MUTEX_WQ EmergencyPool (WAITQ-01); caller_tcb TCB courant, pas d'alias &mut actif.
        unsafe { MUTEX_WQ.wait_interruptible(caller_tcb as *const _ as *mut _); }
    }
}

/// Déverrouille un PthreadMutex.
///
/// # Safety
/// `mutex_ptr` validé par le syscall.
pub unsafe fn PTHREAD_MUTEX_UNLOCK(
    mutex_ptr: *mut PthreadMutex,
    caller_tid: u32,
) -> i32 {
    if mutex_ptr.is_null() { return -EINVAL; }
    let m = &*mutex_ptr;
    if m.mtype == 1 && m.owner.load(Ordering::Relaxed) != caller_tid {
        return -EPERM;
    }
    m.owner.store(0, Ordering::Relaxed);
    m.state.store(0, Ordering::Release);
    // Réveiller un waiter si nécessaire.
    if m.waiters.load(Ordering::Relaxed) > 0 {
        MUTEX_WQ.notify_one();
    }
    ESUCCESS
}

/// Détruit un PthreadMutex.
pub unsafe fn PTHREAD_MUTEX_DESTROY(mutex_ptr: *mut PthreadMutex) -> i32 {
    if mutex_ptr.is_null() { return -EINVAL; }
    let m = &*mutex_ptr;
    if m.state.load(Ordering::Relaxed) != 0 {
        return -EBUSY; // Ne pas détruire un mutex tenu.
    }
    ESUCCESS
}

const EPERM: i32 = 1;
