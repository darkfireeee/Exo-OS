// kernel/src/ipc/sync/sched_hooks.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// SCHED HOOKS — Points d'injection scheduler pour la synchronisation IPC
// (Exo-OS · IPC Couche 2a)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Ce module centralise l'intégration scheduler → IPC pour le blocage réel
// des threads. Il remplace le spin-poll omniprésent dans les primitives IPC
// (futex, wait_queue, canal synchrone, event, barrier).
//
// ARCHITECTURE :
//   BLOCK_HOOK   : fn() injectée par scheduler/ — bloque le thread courant
//   wake_thread  : appelle directement scheduler/ (ipc/ peut l'importer)
//   SLEEP_REGISTRY : table tid → *TCB pour retrouver les TCBs en attente IPC
//
// RÈGLE : install_block_hook() est appelé depuis la séquence d'init noyau
//         (après scheduler::init), pas depuis le scheduler lui-même.
//
// COUCHE : ipc/ dépend de scheduler/ (autorisé — voir DOC5).
// ═══════════════════════════════════════════════════════════════════════════════

use core::ptr::NonNull;

use crate::scheduler::core::preempt::IrqGuard;
use crate::scheduler::core::preempt::MAX_CPUS;
use crate::scheduler::core::runqueue::run_queue;
use crate::scheduler::core::switch::current_thread_raw;
use crate::scheduler::core::task::{TaskState, ThreadControlBlock};
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────────────────────
// Hook de blocage — fourni par le scheduler au boot
// ─────────────────────────────────────────────────────────────────────────────

/// Type du hook : suspend le thread courant, retourne après réveil.
pub type BlockFn = unsafe fn();

/// Hook injecté par scheduler::init(). None = spin-poll (avant init).
static BLOCK_HOOK: SpinLock<Option<BlockFn>> = SpinLock::new(None);

// ─────────────────────────────────────────────────────────────────────────────
// Registre des threads IPC endormis (tid → *TCB)
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre maximum de threads IPC pouvant dormir simultanément.
pub const MAX_SLEEPING_IPC: usize = 128;

#[repr(C)]
struct SleepEntry {
    tid: u64,
    tcb_ptr: usize, // *mut ThreadControlBlock encodé en usize
}

impl SleepEntry {
    const fn empty() -> Self {
        Self { tid: 0, tcb_ptr: 0 }
    }
    fn is_free(&self) -> bool {
        self.tcb_ptr == 0
    }
    fn is_pending_wake(&self) -> bool {
        self.tcb_ptr == usize::MAX
    }
}

struct SleepRegistry {
    entries: [SleepEntry; MAX_SLEEPING_IPC],
}

enum RegisterResult {
    Registered,
    AlreadyWoken,
    Full,
}

// SAFETY: accès protégé par SpinLock.
unsafe impl Send for SleepRegistry {}

impl SleepRegistry {
    const fn new() -> Self {
        const E: SleepEntry = SleepEntry::empty();
        Self {
            entries: [E; MAX_SLEEPING_IPC],
        }
    }

    /// Enregistre (tid, tcb), ou consomme un réveil anticipé.
    fn register(&mut self, tid: u64, tcb: *mut ThreadControlBlock) -> RegisterResult {
        for e in self.entries.iter_mut() {
            if e.tid == tid && e.is_pending_wake() {
                *e = SleepEntry::empty();
                return RegisterResult::AlreadyWoken;
            }
        }
        for e in self.entries.iter_mut() {
            if e.is_free() {
                e.tid = tid;
                e.tcb_ptr = tcb as usize;
                return RegisterResult::Registered;
            }
        }
        RegisterResult::Full
    }

    /// Désenregistre et retourne le TCB pour `tid`, ou null si absent.
    fn pop(&mut self, tid: u64) -> *mut ThreadControlBlock {
        for e in self.entries.iter_mut() {
            if e.tid == tid && !e.is_free() && !e.is_pending_wake() {
                let ptr = e.tcb_ptr as *mut ThreadControlBlock;
                *e = SleepEntry::empty();
                return ptr;
            }
        }
        core::ptr::null_mut()
    }

    fn mark_pending_wake(&mut self, tid: u64) {
        for e in self.entries.iter_mut() {
            if e.tid == tid && e.is_pending_wake() {
                return;
            }
        }
        for e in self.entries.iter_mut() {
            if e.is_free() {
                e.tid = tid;
                e.tcb_ptr = usize::MAX;
                return;
            }
        }
    }
}

static SLEEP_REGISTRY: SpinLock<SleepRegistry> = SpinLock::new(SleepRegistry::new());

// ─────────────────────────────────────────────────────────────────────────────
// API publique
// ─────────────────────────────────────────────────────────────────────────────

/// Installe le hook de blocage depuis le scheduler.
///
/// Appelé UNE SEULE FOIS durant la séquence d'initialisation noyau,
/// après `scheduler::init()`, avant tout usage IPC bloquant.
pub fn install_block_hook(block: BlockFn) {
    *BLOCK_HOOK.lock() = Some(block);
}

/// Retourne `true` si le hook de blocage est installé.
#[inline]
pub fn hooks_installed() -> bool {
    BLOCK_HOOK.lock().is_some()
}

/// Retourne le ThreadId du thread courant (0 si inconnu).
///
/// Utilisé par les canaux synchrones pour enregistrer leur TID avant blocage.
#[inline]
pub fn current_tid() -> u64 {
    let tcb_ptr = current_thread_raw();
    if tcb_ptr.is_null() {
        return 0;
    }
    // SAFETY: tcb_ptr non-null (vérifié ci-dessus); pointe dans le pool statique de TCBs.
    unsafe { (*tcb_ptr).tid }
}

/// Bloque le thread courant identifié par `tid`.
///
/// Enregistre le couple (tid, tcb) dans SLEEP_REGISTRY AVANT de bloquer,
/// ce qui permet à `wake_thread(tid)` de retrouver le TCB.
///
/// Après réveil (par `wake_thread`), sort du registre et retourne.
/// Si BLOCK_HOOK n'est pas installé, effectue un spin court (avant scheduler init).
///
/// # Safety
/// - Le thread doit être dans une file d'attente IPC (IpcWaiter actif, futex waiter, etc.)
/// - L'appelant doit vérifier le drapeau `woken` AVANT d'appeler cette fonction
///   pour éviter un blocage si le réveil a déjà eu lieu.
pub unsafe fn block_current(tid: u64) {
    let tcb_ptr = current_thread_raw();

    if !tcb_ptr.is_null() {
        let tcb = &mut *tcb_ptr;
        tcb.set_state(TaskState::Sleeping);
        let registered = SLEEP_REGISTRY.lock().register(tid, tcb_ptr);
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        match registered {
            RegisterResult::Registered => {
                if tcb.state() == TaskState::Runnable {
                    SLEEP_REGISTRY.lock().pop(tid);
                    return;
                }
            }
            RegisterResult::AlreadyWoken => {
                tcb.set_state(TaskState::Runnable);
                return;
            }
            RegisterResult::Full => {
                tcb.set_state(TaskState::Running);
                for _ in 0..10_000 {
                    core::hint::spin_loop();
                }
                return;
            }
        }
    }

    if let Some(block_fn) = *BLOCK_HOOK.lock() {
        block_fn();
        // Après réveil : désenregistrer si pas encore fait par wake_thread.
        if !tcb_ptr.is_null() {
            SLEEP_REGISTRY.lock().pop(tid);
        }
    } else {
        // Avant init scheduler : spin court (~10 µs à 3 GHz).
        for _ in 0..10_000 {
            core::hint::spin_loop();
        }
        if !tcb_ptr.is_null() {
            SLEEP_REGISTRY.lock().pop(tid);
            (*tcb_ptr).set_state(TaskState::Running);
        }
    }
}

/// Réveille le thread identifié par `tid`.
///
/// Trouve le TCB via SLEEP_REGISTRY, tente la transition TaskState::Sleeping →
/// Runnable, et l'enfile dans la run queue de son CPU.
/// No-op si le thread n'est pas enregistré (réveil anticipé ou appelé deux fois).
pub fn wake_thread(tid: u64) {
    let tcb_ptr = {
        let mut reg = SLEEP_REGISTRY.lock();
        reg.pop(tid)
    };

    if tcb_ptr.is_null() {
        let mut reg = SLEEP_REGISTRY.lock();
        reg.mark_pending_wake(tid);
        return;
    }
    // SAFETY: tcb_ptr non-null (testé ci-dessus); TCB dans pool statique, non libéré pendant l'exécution.
    unsafe {
        let tcb = &mut *tcb_ptr;
        let cpu_id = tcb.current_cpu();
        if (cpu_id.0 as usize) < MAX_CPUS {
            let _irq = IrqGuard::new();
            let rq = run_queue(cpu_id);
            // CAS Sleeping → Runnable protège contre les doubles-réveils.
            if tcb.try_transition(TaskState::Sleeping, TaskState::Runnable) {
                // SAFETY: tcb_ptr est valide (vit dans le pool statique de TCBs).
                rq.enqueue(NonNull::new_unchecked(tcb_ptr));
            }
        }
    }
}
