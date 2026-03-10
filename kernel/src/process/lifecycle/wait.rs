// kernel/src/process/lifecycle/wait.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// waitpid() — Attente de la terminaison d'un processus fils (Exo-OS Couche 1.5)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Implémentation :
//   • Scan de la registry pour trouver le fils Zombie.
//   • Si non trouvé et WNOHANG absent : blocage sur une wait_queue.
//   • La SIGCHLD handler réveille la wait_queue.
//   • Retour du PID terminé + code de sortie.
// ═══════════════════════════════════════════════════════════════════════════════


use core::sync::atomic::Ordering;
use crate::process::core::pid::Pid;
use crate::process::core::pcb::ProcessState;
use crate::process::core::registry::PROCESS_REGISTRY;

// ─────────────────────────────────────────────────────────────────────────────
// Types
// ─────────────────────────────────────────────────────────────────────────────

/// Options de waitpid (WNOHANG, WUNTRACED, WCONTINUED).
#[derive(Copy, Clone, Default, Debug)]
pub struct WaitOptions(pub u32);

impl WaitOptions {
    /// Retour immédiat si aucun fils n'est terminé.
    pub const WNOHANG:    u32 = 1 << 0;
    /// Rapporté quand un fils est arrêté (SIGSTOP).
    pub const WUNTRACED:  u32 = 1 << 1;
    /// Rapporté quand un fils reprend (SIGCONT).
    pub const WCONTINUED: u32 = 1 << 2;
    /// Attendre n'importe quel fils (pid=-1).
    pub const WALL:       u32 = 1 << 3;

    pub fn has(self, flag: u32) -> bool { self.0 & flag != 0 }
}

/// Résultat d'un waitpid réussi.
#[derive(Debug, Clone, Copy)]
pub struct WaitResult {
    /// PID du fils terminé (ou arrêté/continué).
    pub pid:       Pid,
    /// Code de sortie brut (wstatus POSIX : exit_code << 8).
    pub wstatus:   u32,
    /// Raison de la terminaison.
    pub reason:    WaitReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitReason {
    /// Terminaison normale (exit()).
    Exited,
    /// Tué par un signal.
    Signaled,
    /// Arrêté (SIGSTOP).
    Stopped,
    /// Repris (SIGCONT).
    Continued,
}

/// Erreur de waitpid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitError {
    /// Aucun fils correspondant (ECHILD).
    NoChild,
    /// Aucun fils terminé et WNOHANG positionné (EAGAIN).
    WouldBlock,
    /// Attente interrompue par un signal (EINTR).
    Interrupted,
    /// PID demandé invalide.
    InvalidPid,
}

impl WaitResult {
    /// Encode un code de sortie normal (exit_code << 8, signal=0).
    pub fn exited(pid: Pid, code: u8) -> Self {
        Self {
            pid,
            wstatus: (code as u32) << 8,
            reason:  WaitReason::Exited,
        }
    }

    /// Encode un code de terminaison par signal (code POSIX).
    pub fn signaled(pid: Pid, sig: u8, core_dumped: bool) -> Self {
        let dump_bit = if core_dumped { 0x80u32 } else { 0 };
        Self {
            pid,
            wstatus: (sig as u32) | dump_bit,
            reason:  WaitReason::Signaled,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// WaitTable — table des parents en attente
// ─────────────────────────────────────────────────────────────────────────────

use crate::scheduler::sync::wait_queue::WaitQueue;
use crate::scheduler::sync::spinlock::SpinLock;

/// Entrée dans la table d'attente des parents.
#[allow(dead_code)]
struct WaitEntry {
    /// PID du parent attendant.
    parent_pid:  u32,
    /// PID du fils attendu (0 = n'importe quel fils du parent).
    child_pid:   u32,
    /// Résultat déposé par SIGCHLD / exit().
    result_pid:  core::sync::atomic::AtomicU32,
    result_code: core::sync::atomic::AtomicU32,
}

/// Table des wait en cours — max 512 entrees simultanées.
#[allow(dead_code)]
struct WaitTable {
    entries: SpinLock<[Option<WaitEntry>; 512]>,
    queue:   WaitQueue,
}

// SAFETY: WaitTable accessible depuis plusieurs CPUs, protégé par SpinLock.
unsafe impl Sync for WaitTable {}

static WAIT_TABLE: WaitQueue = WaitQueue::new();

// ─────────────────────────────────────────────────────────────────────────────
// do_waitpid
// ─────────────────────────────────────────────────────────────────────────────

/// Attend la terminaison d'un processus fils.
///
/// # Arguments
/// * `caller_pid`  — PID du processus appelant.
/// * `wait_pid`    — PID à attendre (−0 = n'importe quel fils).
/// * `opts`        — flags d'attente.
/// * `caller_tcb`  — TCB du thread appelant (pour blocage).
pub fn do_waitpid(
    caller_pid: Pid,
    wait_pid:   i32,
    opts:       WaitOptions,
    caller_tcb: &crate::scheduler::core::task::ThreadControlBlock,
) -> Result<WaitResult, WaitError> {
    // Scan rapide : chercher un fils déjà Zombie dans la registry.
    let result = scan_zombie_children(caller_pid, wait_pid, opts);
    if let Some(r) = result {
        return Ok(r);
    }
    // Aucun fils Zombie trouvé.
    if opts.has(WaitOptions::WNOHANG) {
        // Vérifier si au moins un fils existe.
        if has_children(caller_pid) {
            return Err(WaitError::WouldBlock);
        } else {
            return Err(WaitError::NoChild);
        }
    }
    // Bloquer — attendre la notification via SIGCHLD/wait_queue.
    // NOTE : la wait_queue utilise l'EmergencyPool (RÈGLE WAITQ-01).
    // Boucle : les wakeup spurieux sont tolérés.
    loop {
        // Vérifier signal pending (EINTR si signal non-SIGCHLD).
        if caller_tcb.has_signal_pending() {
            return Err(WaitError::Interrupted);
        }
        // Se mettre en attente sur WAIT_TABLE.
        // SAFETY: WaitQueue EmergencyPool (WAITQ-01); caller_tcb TCB courant, pas d'alias &mut actif.
        unsafe {
            WAIT_TABLE.wait_interruptible(caller_tcb as *const _ as *mut _);
        }
        // Réessàyer.
        if let Some(r) = scan_zombie_children(caller_pid, wait_pid, opts) {
            return Ok(r);
        }
        if !has_children(caller_pid) {
            return Err(WaitError::NoChild);
        }
    }
}

/// Réveille tous les parents en attente (appelé par SIGCHLD delivery).
pub fn wake_waiting_parents(child_pid: Pid, parent_pid: Pid) {
    let _ = (child_pid, parent_pid);
    WAIT_TABLE.notify_all();
}

/// Scanne la registry pour trouver un fils Zombie du parent.
fn scan_zombie_children(
    parent_pid: Pid,
    wait_pid:   i32,
    _opts:      WaitOptions,
) -> Option<WaitResult> {
    let any_child = wait_pid < 0 || wait_pid == 0;
    let mut found: Option<WaitResult> = None;

    PROCESS_REGISTRY.for_each(|pcb| {
        if found.is_some() { return; }
        let ppid = Pid(pcb.ppid.load(Ordering::Relaxed));
        if ppid != parent_pid { return; }
        if !any_child && pcb.pid.0 != wait_pid as u32 { return; }
        if pcb.state() == ProcessState::Zombie {
            let code = pcb.exit_code.load(Ordering::Acquire);
            found = Some(WaitResult::exited(pcb.pid, code as u8));
        }
    });
    found
}

/// Vérifie si le parent a au moins un fils actif.
fn has_children(parent_pid: Pid) -> bool {
    let mut found = false;
    PROCESS_REGISTRY.for_each(|pcb| {
        if found { return; }
        let ppid = Pid(pcb.ppid.load(Ordering::Relaxed));
        if ppid == parent_pid { found = true; }
    });
    found
}
