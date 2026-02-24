// kernel/src/process/signal/delivery.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Livraison des signaux POSIX — Exo-OS Couche 1.5
// RÈGLE SIGNAL-01 : handle_pending_signals appelé UNIQUEMENT au retour syscall.
// ═══════════════════════════════════════════════════════════════════════════════

#![allow(dead_code)]

use core::sync::atomic::Ordering;
use crate::scheduler::core::task::ThreadControlBlock;
use crate::process::core::pid::Pid;
use crate::process::core::registry::PROCESS_REGISTRY;
use crate::process::core::pcb::ProcessState;
use super::default::{Signal, SigAction, SigActionKind, default_action};
use super::queue::{SigInfo, SigQueue, RTSigQueue};
use super::mask::SigMask;

// ─────────────────────────────────────────────────────────────────────────────
// Erreurs
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendError {
    /// PID cible inexistant.
    NoSuchProcess,
    /// Signal numéro invalide.
    InvalidSignal,
    /// Permission refusée (EPERM).
    PermissionDenied,
}

// ─────────────────────────────────────────────────────────────────────────────
// Envoi de signal depuis noyau (kill(2), raise, SIGCHLD...)
// ─────────────────────────────────────────────────────────────────────────────

/// Envoie un signal à un processus cible par PID.
/// Appelé par do_exit() (SIGCHLD), kill(2), etc.
///
/// Algorithme :
/// 1. Cherche le PCB dans PROCESS_REGISTRY.
/// 2. Pour un signal standard : met le bit dans pending_signals du TCB
///    du thread « principal » (thread 0) du processus.
/// 3. Pour un RT signal : empile dans la RTSigQueue du thread.
/// 4. Démande une préemption via raise_signal_pending().
pub fn send_signal_to_pid(pid: Pid, sig: Signal) -> Result<(), SendError> {
    let sig_n = sig.number();
    let pcb = PROCESS_REGISTRY.find_by_pid(pid)
        .ok_or(SendError::NoSuchProcess)?;

    // Un processus zombie ne peut plus recevoir de signaux (sauf SIGCHLD ignore).
    let state = pcb.state();
    if state == ProcessState::Zombie || state == ProcessState::Dead {
        return Ok(());
    }

    // Récupère le thread principal (TID == PID).
    let thread_ptr = pcb.main_thread_ptr();
    if thread_ptr.is_null() { return Err(SendError::NoSuchProcess); }

    // SAFETY : thread_ptr est valide tant que le PCB est vivant ; on est sous
    // spinlock PCB (write_lock) -- ici lecture seule du pointeur suffisante.
    let thread = unsafe { &*thread_ptr };

    // Mettre le signal en file.
    if sig_n < 32 {
        thread.sig_queue.enqueue(sig_n);
    } else {
        let info = SigInfo::kernel(sig_n);
        thread.rt_sig_queue.enqueue(sig_n, info);
    }

    // Notifier le scheduler : signal_pending = true (PROC-04 via raise_signal_pending).
    thread.raise_signal_pending();
    Ok(())
}

/// Envoie un signal directement à un TCB scheduler connu.
/// Utilisé quand on a déjà le punteur TCB (ex: livraison d'exception).
pub fn send_signal_to_tcb(
    thread:    &crate::process::core::tcb::ProcessThread,
    sig:       u8,
    info:      SigInfo,
) {
    if sig == 0 || sig > 63 { return; }
    if sig < 32 {
        thread.sig_queue.enqueue(sig);
    } else {
        thread.rt_sig_queue.enqueue(sig, info);
    }
    thread.raise_signal_pending();
}

// ─────────────────────────────────────────────────────────────────────────────
// handle_pending_signals — RÈGLE SIGNAL-01 : appelé uniquement au retour syscall
// ─────────────────────────────────────────────────────────────────────────────

/// Contexte de retour syscall (pointeurs vers les registres sauvegardés).
/// Rempli par l'asm syscall-entry et passé ici pour permettre au handler
/// de modifier les registres utilisateur (rip, rsp, rax...).
#[repr(C)]
pub struct SyscallFrame {
    pub user_rsp:    u64,
    pub user_rip:    u64,
    pub user_rflags: u64,
    pub user_rax:    u64,  // valeur de retour syscall
    pub user_rdi:    u64,
    pub user_rsi:    u64,
    pub user_rdx:    u64,
    pub user_rcx:    u64,
    pub user_r8:     u64,
    pub user_r9:     u64,
    pub user_cs:     u64,
    pub user_ss:     u64,
}

/// Traite tous les signaux en attente non-bloqués.
/// **Appelé UNIQUEMENT au retour d'un syscall** (RÈGLE SIGNAL-01).
///
/// Algorithme pour chaque signal défilé :
/// 1. Vérifier le masque -> skip si bloqué.
/// 2. Lire SigAction dans la table PCB.
/// 3. Dispatcher selon kind : User | Ignore | Term | Core | Stop | Cont.
/// 4. SA_RESETHAND : réinitialiser handler après délivrance.
pub fn handle_pending_signals(
    thread: &mut crate::process::core::tcb::ProcessThread,
    frame:  &mut SyscallFrame,
) {
    use crate::process::core::pid::Pid;
    use super::handler::setup_signal_frame;

    // Pas de signal_pending ? Sortie rapide.
    if !thread.sched_tcb.signal_pending.load(Ordering::Acquire) { return; }

    let mask = thread.sched_tcb.signal_mask.load(Ordering::Acquire);
    let pid  = Pid(thread.sched_tcb.pid.0);

    // Lire la table des handlers depuis le PCB.
    let pcb = match PROCESS_REGISTRY.find_by_pid(pid) {
        Some(p) => p,
        None    => { clear_signal_pending(thread); return; }
    };

    // Boucle de livraison : on traite jusqu'à ce qu'il n'y ait plus rien.
    loop {
        // Défiler depuis la queue standard.
        let maybe = thread.sig_queue.dequeue(mask);
        let (sig_n, info) = if let Some(pair) = maybe {
            pair
        } else {
            // Essayer la queue RT.
            let rt_mask = (mask >> 32) as u32;
            if let Some(pair) = thread.rt_sig_queue.dequeue(rt_mask) {
                pair
            } else {
                break; // Rien d'autre.
            }
        };

        let action = {
            let handlers = pcb.sig_handlers.lock();
            handlers.get(sig_n)
        };

        deliver_one(thread, frame, sig_n, info, action, pcb);

        // SA_RESETHAND : handler → SIG_DFL après première livraison.
        if action.flags & SigAction::SA_RESETHAND != 0 {
            let def = default_action(sig_n);
            let mut handlers = pcb.sig_handlers.lock();
            handlers.set(sig_n, def);
        }
    }

    // Effacer le drapeau si plus rien en attente.
    let remaining_std = thread.sig_queue.pending.load(Ordering::Acquire) & !mask;
    let remaining_rt  = (thread.rt_sig_queue.pending_mask.load(Ordering::Acquire) as u32)
                        & !((mask >> 32) as u32);
    if remaining_std == 0 && remaining_rt == 0 {
        clear_signal_pending(thread);
    }
}

/// Livre un seul signal.
fn deliver_one(
    thread: &mut crate::process::core::tcb::ProcessThread,
    frame:  &mut SyscallFrame,
    sig_n:  u8,
    info:   SigInfo,
    action: SigAction,
    pcb:    &crate::process::core::pcb::ProcessControlBlock,
) {
    use super::handler::setup_signal_frame;
    use crate::process::lifecycle::exit::do_exit;

    match action.kind {
        SigActionKind::Ignore => {
            // Rien à faire.
        }
        SigActionKind::User => {
            // Construire un frame utilisateur pour exécuter le handler.
            // RÈGLE SIGNAL-01 : setup_signal_frame modifie frame->user_rip / user_rsp.
            setup_signal_frame(thread, frame, sig_n, &info, &action);
        }
        SigActionKind::Stop => {
            // Mettre le processus en état Stopped + notifier parent.
            pcb.set_state(crate::process::core::pcb::ProcessState::Stopped);
            // Envoyer SIGCHLD au parent (POSIX obligation).
            let ppid = pcb.ppid();
            if ppid.0 != 0 {
                let _ = send_signal_to_pid(
                    ppid,
                    Signal::SIGCHLD,
                );
            }
            // Bloquer le thread jusqu'à SIGCONT (POSIX SIGSTOP sémantique).
            // Le thread sera réveillé par deliver_one(SIGCONT) via PCB::set_state(Running).
            thread.sched_tcb.set_state(crate::scheduler::core::task::TaskState::Sleeping);
            // SAFETY: thread.sched_tcb est le TCB du thread courant ; la déréférence &mut *
            //         est sûre car on est le seul détenteur sur ce CPU (préemption désactivée).
            unsafe {
                let cpu_id = thread.sched_tcb.current_cpu();
                let rq = crate::scheduler::core::runqueue::run_queue(cpu_id);
                crate::scheduler::schedule_block(rq, &mut *thread.sched_tcb);
            }
        }
        SigActionKind::Cont => {
            // Réveiller le processus s'il était arrêté.
            let state = pcb.state();
            if state == crate::process::core::pcb::ProcessState::Stopped {
                pcb.set_state(crate::process::core::pcb::ProcessState::Running);
            }
        }
        SigActionKind::Term | SigActionKind::Core => {
            // Terminer le processus.
            let exit_status: u32 = sig_n as u32;
            do_exit(thread, pcb, exit_status);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers internes
// ─────────────────────────────────────────────────────────────────────────────

#[inline(always)]
fn clear_signal_pending(thread: &crate::process::core::tcb::ProcessThread) {
    thread.sched_tcb.signal_pending.store(false, Ordering::Release);
}
