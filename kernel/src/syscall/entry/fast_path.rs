//! # Implémentation du Fast Path pour les Syscalls
//!
//! Ce module contient les handlers pour les syscalls les plus simples et les plus rapides.
//! L'objectif est de minimiser le nombre d'instructions et d'accès mémoire.
//! Aucune allocation, aucune validation complexe, aucun lock.

use crate::syscall::abi::{SyscallArgs, SyscallResult};
use crate::syscall::numbers::Syscall;
use crate::task;

/// Gère un syscall du "fast path".
///
/// # Performance
///
/// Cette fonction est conçue pour être exécutée en moins de 50 cycles.
/// Elle utilise un `match` qui sera compilé en une table de saut (jump table)
/// pour une performance de branchement O(1).
#[inline(always)]
pub fn handle(number: usize, args: SyscallArgs) -> isize {
    // Le `match` est optimisé par le compilateur en une table de saut.
    let result: SyscallResult = match Syscall::from_number(number) {
        Some(Syscall::GetPid) => sys_getpid(),
        Some(Syscall::SchedYield) => sys_sched_yield(),
        Some(Syscall::ExitThread) => sys_exit_thread(args.arg1 as i32),
        Some(Syscall::GetTid) => sys_gettid(),
        // Ne devrait jamais arriver si `is_fast_path` est correct
        _ => Err(crate::error::KernelError::Nosys),
    };

    crate::syscall::abi::result_to_isize(result)
}

/// Retourne le PID (Process ID) du processus actuel.
///
/// # Performance
/// Lit le PID depuis une structure de données per-CPU ou directement depuis le `TaskControlBlock`.
/// Aucun lock nécessaire. C'est une simple lecture mémoire.
#[inline(always)]
fn sys_getpid() -> SyscallResult {
    Ok(task::current().pid())
}

/// Cède volontairement le processeur à un autre thread.
///
/// # Performance
/// Appelle directement le planificateur pour marquer le thread comme "prêt à s'exécuter"
/// et déclencher une commutation de contexte si nécessaire.
#[inline(always)]
fn sys_sched_yield() -> SyscallResult {
    crate::scheduler::yield_now();
    Ok(0) // Convention: `sched_yield` retourne toujours 0.
}

/// Termine le thread actuel avec un code de sortie.
///
/// # Performance
/// Marque le thread comme "mort" et déclenche une commutation de contexte.
/// Le nettoyage des ressources du thread est fait plus tard.
#[inline(always)]
fn sys_exit_thread(exit_code: i32) -> SyscallResult {
    log::debug!("Thread {} exiting with code {}", task::current().id(), exit_code);
    task::exit_thread(exit_code);
    // Cette fonction ne retourne jamais.
    // Le code qui suit est pour le type checker.
    unreachable!("sys_exit_thread should not return");
}

/// Retourne le TID (Thread ID) du thread actuel.
///
/// # Performance
/// Similaire à `getpid`, c'est une lecture directe depuis le `TaskControlBlock`.
#[inline(always)]
fn sys_gettid() -> SyscallResult {
    Ok(task::current().id())
}