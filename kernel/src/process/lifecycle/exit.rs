// kernel/src/process/lifecycle/exit.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// exit() — Terminaison process/thread avec cleanup RAII (Exo-OS Couche 1.5)
// ═══════════════════════════════════════════════════════════════════════════════
//
// do_exit() — terminaison du processus (dernier thread ou exit_group) :
//   1. Marquer EXITING dans le PCB.
//   2. Changer état thread → Zombie.
//   3. Libérer les fds (notifier fs/ via handles).
//   4. Envoyer SIGCHLD au parent + réveiller les waits.
//   5. Transitions tcb scheduler → Dead.
//   6. Se dissocier du scheduler (se retirer de la run queue).
//   7. Enqueuer dans la file du reaper kthread.
//
// RÈGLE PROC-07 : zombie reaper = kthread dédié, jamais inline exit.
// ═══════════════════════════════════════════════════════════════════════════════


use core::sync::atomic::Ordering;
use crate::process::core::pid::{Pid, TID_ALLOCATOR};
use crate::process::core::pcb::{ProcessControlBlock, ProcessState};
use crate::process::core::tcb::ProcessThread;
use crate::process::signal::delivery::send_signal_to_pid;
use crate::process::signal::default::Signal;
use crate::scheduler::core::task::TaskState;
use crate::scheduler::core::preempt::PreemptGuard;
use crate::scheduler::core::runqueue::run_queue;
use crate::scheduler::schedule_block;
use super::reap::REAPER_QUEUE;

// ─────────────────────────────────────────────────────────────────────────────
// do_exit — terminaison du processus
// ─────────────────────────────────────────────────────────────────────────────

/// Termine le processus courant avec le code de sortie donné.
///
/// Cette fonction est appelée depuis le syscall exit() ou exit_group().
/// Elle ne retourne JAMAIS.
///
/// # Safety
/// `thread` et `pcb` doivent correspondre au thread courant.
/// La préemption est désactivée à la fin pour garantir la transition Zombie.
pub fn do_exit(
    thread:    &mut ProcessThread,
    pcb:       &ProcessControlBlock,
    exit_code: u32,
) -> ! {
    // 1. Marquer le processus comme en cours de terminaison (atome pour vis. cross-CPU).
    pcb.set_exiting();
    pcb.exit_code.store(exit_code, Ordering::Release);

    // 2. Décrémenter le compteur de threads actifs.
    let remaining = pcb.dec_threads();

    // 3. Si c'est le dernier thread, libérer les ressources du processus.
    if remaining == 0 {
        // Libérer tous les fds ouverts.
        let handles_to_close: alloc::vec::Vec<u64> = {
            let mut files = pcb.files.lock();
            let mut h = alloc::vec::Vec::new();
            // Parcourir et retirer tous les descripteurs.
            for fd in 0..1024i32 {
                if let Some(handle) = files.close(fd) {
                    h.push(handle);
                }
            }
            h
        };
        // NOTE: la fermeture effective des handles est asynchrone
        // (fs/ collecte via GC de handles) — aucun import fs/ direct.
        drop(handles_to_close);

        // 4. Envoyer SIGCHLD au parent.
        let ppid = Pid(pcb.ppid.load(Ordering::Relaxed));
        if ppid.is_valid() {
            let _ = send_signal_to_pid(ppid, Signal::SIGCHLD);
        }

        // 5. Transition PCB → Zombie (watchable par waitpid).
        pcb.set_state(ProcessState::Zombie);
    }

    // 6. Transition du thread → Dead.
    thread.set_state(TaskState::Dead);

    // 7. Libérer le TID (immédiatement réutilisable).
    TID_ALLOCATOR.free(thread.tid.0);

    // 8. Enqueuer dans la file reaper pour la libération asynchrone.
    //    RÈGLE PROC-07 : jamais de libération inline (risque double-free + latence).
    let tid = thread.tid;
    let pid = thread.pid;
    REAPER_QUEUE.enqueue(pid, tid);

    // 9. Appeler schedule_block() — ce thread ne sera plus jamais choisi par pick_next_task()
    //    car son état est Dead. schedule_block ne le ré-enfile pas.
    // SAFETY: sched_tcb = TCB courant; thread en mémoire jusqu'au reap; schedule_block ne retourne pas (Dead).
    unsafe {
        let _preempt = PreemptGuard::new();
        let cpu_id = thread.sched_tcb.current_cpu();
        let rq = run_queue(cpu_id);
        schedule_block(rq, &mut *thread.sched_tcb);
    }

    // schedule_yield() never returns when state is Dead.
    // Pour satisfaire le type `!` :
    loop {
        // SAFETY: le code ici n'est jamais exécuté (le scheduler ne reviendra pas).
        unsafe { core::arch::asm!("hlt"); }
    }
}

/// Termine uniquement le thread courant (pthread_exit / exit thread).
/// Si c'est le dernier thread du processus, appelle do_exit().
pub fn do_exit_thread(
    thread:      &mut ProcessThread,
    pcb:         &ProcessControlBlock,
    return_val:  u64,
) -> ! {
    // Stocker la valeur de retour pour join().
    thread.join_result.store(return_val, Ordering::Release);
    thread.join_done.store(true, Ordering::Release);

    // Réveiller tout thread en attente via join (futex-based).
    // Le réveil est effectué via le signal: wait_queue notifié.
    // Voir process/thread/join.rs pour la logique de join.

    let remaining = pcb.dec_threads();
    if remaining == 0 {
        // Dernier thread — terminer le processus complet.
        pcb.inc_threads(); // remettre à 1 pour que do_exit le décrémente.
        do_exit(thread, pcb, 0);
    } else {
        // Autres threads actifs — juste terminer ce thread.
        thread.set_state(TaskState::Dead);
        TID_ALLOCATOR.free(thread.tid.0);
        let tid = thread.tid;
        let pid = thread.pid;
        REAPER_QUEUE.enqueue(pid, tid);
        // SAFETY: sched_tcb = TCB courant; thread libéré par reaper après cet appel; schedule_block no-return.
        unsafe {
            let _preempt = PreemptGuard::new();
            let cpu_id = thread.sched_tcb.current_cpu();
            let rq = run_queue(cpu_id);
            schedule_block(rq, &mut *thread.sched_tcb);
        }
        loop {
            // SAFETY: jamais atteint.
            unsafe { core::arch::asm!("hlt"); }
        }
    }
}
