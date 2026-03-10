// kernel/src/process/core/tcb.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ProcessThread — Extension process/ du ThreadControlBlock scheduler
// ═══════════════════════════════════════════════════════════════════════════════
//
// Architecture :
//   Le scheduler/core/task.rs définit `ThreadControlBlock` (128 bytes, hot path).
//   Ce fichier définit `ProcessThread` qui ENVELOPPE le TCB scheduler et ajoute
//   les données niveau process (adresse de stack, TLS, signaux...) sans toucher
//   aux 128 bytes du TCB scheduler.
//
// RÈGLES :
//   • ProcessThread est alloué par lifecycle/create.rs, libéré par lifecycle/exit.rs.
//   • Le pointeur `sched_tcb` pointe vers un TCB en mémoire statique ou heap.
//   • Tous les accès à sched_tcb depuis process/ sont documentaires.
//   • PROC-04 : signal_pending est ÉCRIT ici (process/signal/), LU par scheduler.
// ═══════════════════════════════════════════════════════════════════════════════

#![allow(dead_code)]

use core::sync::atomic::{AtomicU64, AtomicUsize, AtomicBool};
use alloc::boxed::Box;
use crate::scheduler::core::task::{
    ThreadControlBlock, ThreadId, ProcessId, Priority, SchedPolicy,
    TaskState,
};
use super::pid::{Pid, Tid};
use crate::process::signal::queue::{SigQueue, RTSigQueue};

/// Taille du stack kernel par thread (4 pages × 4096 = 16 384 bytes).
pub const KSTACK_SIZE: usize = 16 * 1024;

/// Canari de stack pour détecter les débordements.
const STACK_CANARY: u64 = 0xDEAD_BEEF_CAFE_BABE;

// ─────────────────────────────────────────────────────────────────────────────
// ThreadAddress — adresses de l'espace utilisateur d'un thread
// ─────────────────────────────────────────────────────────────────────────────

/// Adresses liées au cycle de vie du thread côté userspace.
#[derive(Copy, Clone, Debug, Default)]
#[repr(C)]
pub struct ThreadAddress {
    /// Base du stack utilisateur alloué (plus basse adresse).
    pub stack_base:      u64,
    /// Taille du stack utilisateur (bytes).
    pub stack_size:      u64,
    /// Registre d’instruction de retour (RIP initial au lancement).
    pub entry_point:     u64,
    /// Pointeur de cadre utilisateur initial (RSP au démarrage).
    pub initial_rsp:     u64,
    /// Pointeur vers la TLS statique (GS.base pour x86_64).
    pub tls_base:        u64,
    /// Pointeur vers la structure `pthread_t` userspace.
    pub pthread_ptr:     u64,
    /// Zone `sigaltstack` (stack alternatif pour signaux).
    pub sigaltstack_base: u64,
    pub sigaltstack_size: u64,
}

impl ThreadAddress {
    /// Adresse du sommet du sigaltstack (base + size).
    #[inline(always)]
    pub fn sigaltstack_top(&self) -> u64 {
        self.sigaltstack_base.saturating_add(self.sigaltstack_size)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// KernelStack — stack kernel dédié du thread
// ─────────────────────────────────────────────────────────────────────────────

/// Stack kernel alloué dynamiquement pour un thread.
/// La page la plus basse est une guard page (à mapper NX + non-present = trap overflow).
pub struct KernelStack {
    /// Pointeur vers la mémoire allouée (bas du buffer, y compris garde).
    base:  *mut u8,
    /// Taille totale en bytes.
    size:  usize,
    /// Adresse du sommet (base + size, aligné 16).
    top:   u64,
}

impl KernelStack {
    /// Alloue un nouveau stack kernel de `size` bytes.
    /// Pose un canari 8 bytes au bas (détection overflow).
    pub fn alloc(size: usize) -> Option<Self> {
        use alloc::alloc::{alloc, Layout};
        let layout = Layout::from_size_align(size, 16).ok()?;
        // SAFETY: layout est valide, on vérifie le pointeur.
        let base = unsafe { alloc(layout) };
        if base.is_null() { return None; }
        // Écriture du canari au bas du stack.
        // SAFETY: base pointe vers `size` bytes alloués, canari à offset 0.
        unsafe {
            core::ptr::write(base as *mut u64, STACK_CANARY);
        }
        // SAFETY: base a été alloué avec `size` bytes ; base.add(size) pointe juste après la fin,
        //         ce qui est un pointeur valide (sentinel, jamais déréférencé).
        let top = unsafe { base.add(size) } as u64;
        // Aligner sur 16 bytes (x86_64 ABI) : le top doit être 16-aligné - 8.
        let top_aligned = (top & !0xF) - 8;
        Some(Self { base, size, top: top_aligned })
    }

    /// Adresse du sommet utile (valeur initiale du RSP kernel).
    #[inline(always)]
    pub fn top_addr(&self) -> u64 { self.top }

    /// Vérifie le canari — retourne false si débordement détecté.
    pub fn check_canary(&self) -> bool {
        // SAFETY: base a été alloué avec au moins 8 bytes et le canari y est posé.
        unsafe { core::ptr::read(self.base as *const u64) == STACK_CANARY }
    }

    /// Taille en bytes.
    #[inline(always)]
    pub fn size(&self) -> usize { self.size }
}

impl Drop for KernelStack {
    fn drop(&mut self) {
        use alloc::alloc::{dealloc, Layout};
        // SAFETY: base a été alloué avec ce layout.
        unsafe {
            let layout = Layout::from_size_align_unchecked(self.size, 16);
            dealloc(self.base, layout);
        }
    }
}

// SAFETY: KernelStack non partagé entre threads (propriété exclusive).
unsafe impl Send for KernelStack {}

// ─────────────────────────────────────────────────────────────────────────────
// ProcessThread — vue process/ d'un thread
// ─────────────────────────────────────────────────────────────────────────────

/// ProcessThread : extension du TCB scheduler avec les données niveau process.
///
/// Propriétaire unique du stack kernel et du TCB scheduler.
/// Référencé depuis le ProcessControlBlock de son processus parent.
pub struct ProcessThread {
    // ── TCB scheduler (hot path) ───────────────────────────────────────────────
    /// TCB scheduler — propriété exclusive de ce ProcessThread.
    /// Borrowé de manière exclusive par le scheduler pour les context switches.
    pub sched_tcb: Box<ThreadControlBlock>,

    // ── Stack kernel ───────────────────────────────────────────────────────────
    /// Stack kernel dédié à ce thread.
    pub kernel_stack: KernelStack,

    // ── Identité process ───────────────────────────────────────────────────────
    /// PID du processus propriétaire.
    pub pid:          Pid,
    /// TID de ce thread.
    pub tid:          Tid,

    // ── Adresses userspace ─────────────────────────────────────────────────────
    /// Adresses du thread côté userspace.
    pub addresses:    ThreadAddress,

    // ── TLS (Thread Local Storage) ─────────────────────────────────────────────
    /// Base du segment TLS (valeur de GS.base en mode kernel).
    pub tls_gs_base:  AtomicU64,
    /// Bloc TLS statique (segment .tdata/.tbss du binaire).
    pub tls_block:    AtomicUsize,  // *mut u8 opaque
    /// Taille du bloc TLS.
    pub tls_size:     usize,

    // ── État de join ───────────────────────────────────────────────────────────
    /// true = thread detaché (le joineur n'attendra pas).
    pub detached:     AtomicBool,
    /// true = join terminé (résultat disponible dans join_result).
    pub join_done:    AtomicBool,
    /// Valeur de retour du thread (ptr vers donnée userspace).
    pub join_result:  AtomicU64,

    // ── Files de signaux ───────────────────────────────────────────────────────
    /// File de signaux standard (signaux 1..31).
    pub sig_queue:    SigQueue,
    /// File de signaux temps-réel (signaux 32..63).
    pub rt_sig_queue: RTSigQueue,
}

impl ProcessThread {
    /// Crée un nouveau ProcessThread avec un stack kernel frais.
    ///
    /// # Arguments
    /// * `tid`   — TID alloué depuis TID_ALLOCATOR.
    /// * `pid`   — PID du processus propriétaire.
    /// * `cr3`   — CR3 de l'espace d'adressage.
    /// * `policy`/`prio` — politique et priorité d'ordonnancement.
    pub fn new(
        tid:    Tid,
        pid:    Pid,
        cr3:    u64,
        policy: SchedPolicy,
        prio:   Priority,
    ) -> Option<Box<Self>> {
        let kstack = KernelStack::alloc(KSTACK_SIZE)?;
        let stack_top = kstack.top_addr();

        let sched_tcb = Box::new(ThreadControlBlock::new(
            ThreadId(tid.0),
            ProcessId(pid.0),
            policy,
            prio,
            cr3,
            stack_top,
        ));

        Some(Box::new(Self {
            sched_tcb,
            kernel_stack: kstack,
            pid,
            tid,
            addresses:    ThreadAddress::default(),
            tls_gs_base:  AtomicU64::new(0),
            tls_block:    AtomicUsize::new(0),
            tls_size:     0,
            detached:     AtomicBool::new(false),
            join_done:    AtomicBool::new(false),
            join_result:  AtomicU64::new(0),
            sig_queue:    SigQueue::new(),
            rt_sig_queue: RTSigQueue::new(),
        }))
    }

    /// Crée un thread kernel dédié (pid=1, KTHREAD flag).
    pub fn new_kthread(tid: Tid, cr3: u64) -> Option<Box<Self>> {
        Self::new(tid, Pid(1), cr3, SchedPolicy::Normal, Priority::NORMAL_DEFAULT)
    }

    /// Référence au TCB scheduler (short-lived, hot path).
    #[inline(always)]
    pub fn tcb(&self) -> &ThreadControlBlock {
        &self.sched_tcb
    }

    /// Référence mutable au TCB scheduler.
    #[inline(always)]
    pub fn tcb_mut(&mut self) -> &mut ThreadControlBlock {
        &mut self.sched_tcb
    }

    /// Pointeur brut vers le TCB (utilisé par les run queues).
    #[inline(always)]
    pub fn tcb_ptr(&self) -> *mut ThreadControlBlock {
        &*self.sched_tcb as *const ThreadControlBlock as *mut ThreadControlBlock
    }

    /// Vérifie l'intégrité du canari kernel stack.
    #[inline(always)]
    pub fn check_stack_canary(&self) -> bool {
        self.kernel_stack.check_canary()
    }

    /// Elève le signal_pending dans le TCB scheduler (PROC-04).
    /// Appelé UNIQUEMENT depuis process/signal/delivery.rs.
    #[inline(always)]
    pub fn raise_signal_pending(&self) {
        self.sched_tcb.set_signal_pending();
        // Requérir un reschedule pour livraison rapide.
        self.sched_tcb.request_preemption();
    }

    /// Lit l'état courant du thread via le TCB scheduler.
    #[inline(always)]
    pub fn state(&self) -> TaskState {
        self.sched_tcb.state()
    }

    /// Définit l'état du thread.
    #[inline(always)]
    pub fn set_state(&self, s: TaskState) {
        self.sched_tcb.set_state(s);
    }
}

// SAFETY: ProcessThread est accédé depuis un seul CPU à la fois (propriété scheduler).
// Les champs atomiques permettent les lectures concurrentes.
unsafe impl Send for ProcessThread {}
unsafe impl Sync for ProcessThread {}
