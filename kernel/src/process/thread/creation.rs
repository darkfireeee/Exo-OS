// kernel/src/process/thread/creation.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Création de threads POSIX <500ns (Exo-OS Couche 1.5)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Objectif performance : création <500ns mesurée depuis syscall clone().
//
// Séquence create_thread() :
//   1. Allouer TID.
//   2. Créer ProcessThread (stack kernel + TCB).
//   3. Configurer le point d'entrée + stack utilisateur.
//   4. Configurer la TLS.
//   5. Enregistrer dans le PCB (inc_threads).
//   6. Enqueuer dans la run queue.
// ═══════════════════════════════════════════════════════════════════════════════

use crate::process::core::pcb::ProcessControlBlock;
use crate::process::core::pid::{Tid, TID_ALLOCATOR};
use crate::process::core::tcb::{ProcessThread, ThreadAddress};
use crate::scheduler::core::preempt::{PreemptGuard, MAX_CPUS};
use crate::scheduler::core::runqueue::run_queue;
use crate::scheduler::core::task::{CpuId, Priority, SchedPolicy};
use alloc::boxed::Box;
use core::ptr::NonNull;
use core::sync::atomic::Ordering;

/// Erreur de création de thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadCreateError {
    TidExhausted,
    OutOfMemory,
    InvalidCpu,
    ProcessExiting,
    StackSetupFailed,
}

/// Attributs d'un thread POSIX à créer.
#[derive(Clone)]
pub struct ThreadAttr {
    /// Taille du stack utilisateur demandée (0 = défaut).
    pub stack_size: u64,
    /// Adresse de base du stack fournie par l'utilisateur (0 = allouer).
    pub stack_addr: u64,
    /// Politique d'ordonnancement.
    pub policy: SchedPolicy,
    /// Priorité.
    pub priority: Priority,
    /// Thread détaché par défaut.
    pub detached: bool,
    /// CPU d'affinité (-1 = sans préférence).
    pub cpu_affinity: i32,
    /// Taille de la zone sigaltstack (0 = SIGSTKSZ défaut 8192).
    pub sigaltstack_size: u64,
}

impl Default for ThreadAttr {
    fn default() -> Self {
        Self {
            stack_size: 8 * 1024 * 1024, // 8 MiB par défaut
            stack_addr: 0,
            policy: SchedPolicy::Normal,
            priority: Priority::NORMAL_DEFAULT,
            detached: false,
            cpu_affinity: -1,
            sigaltstack_size: 8192,
        }
    }
}

/// Paramètres de création d'un thread.
pub struct ThreadCreateParams {
    /// PCB du processus auquel ajouter le thread.
    pub pcb: *const ProcessControlBlock,
    /// Attributs du thread.
    pub attr: ThreadAttr,
    /// Point d'entrée du thread (adresse userspace).
    pub start_func: u64,
    /// Argument passé au thread (valeur userspace, rdi).
    pub arg: u64,
    /// CPU cible pour l'enqueue.
    pub target_cpu: u32,
    /// Adresse de la structure pthread_t à remplir.
    pub pthread_out: u64,
}

/// Handle d'un thread créé.
pub struct ThreadHandle {
    pub tid: Tid,
    pub thread: *mut ProcessThread,
}

// SAFETY: transféré d'un seul thread producteur.
unsafe impl Send for ThreadHandle {}

/// Crée un nouveau thread POSIX dans le processus donné.
///
/// # Safety
/// `params.pcb` doit pointer vers un PCB valide et non libéré.
pub fn create_thread(params: &ThreadCreateParams) -> Result<ThreadHandle, ThreadCreateError> {
    // SAFETY: pcb est garanti valide par l'appelant.
    let pcb = unsafe { &*params.pcb };

    // Refus si le processus est en cours de terminaison.
    if pcb.is_exiting() {
        return Err(ThreadCreateError::ProcessExiting);
    }

    // 1. Allouer TID.
    let tid_raw = TID_ALLOCATOR
        .alloc()
        .map_err(|_| ThreadCreateError::TidExhausted)?;
    let tid = Tid(tid_raw);

    // 2. Créer le ProcessThread.
    let cr3 = pcb.cr3.load(Ordering::Relaxed);
    let thread = ProcessThread::new(tid, pcb.pid, cr3, params.attr.policy, params.attr.priority)
        .ok_or_else(|| {
            TID_ALLOCATOR.free(tid_raw);
            ThreadCreateError::OutOfMemory
        })?;

    // 3. Configurer le point d'entrée et le stack utilisateur.
    let thread_ptr = Box::into_raw(thread);
    // SAFETY: thread_ptr valide, juste créé.
    unsafe {
        // Configurer le frame de démarrage sur le stack kernel pour le trampoline.
        let kstack_top = (*thread_ptr).kernel_stack.top_addr();
        // Frame iretq : [rip, cs, rflags, rsp, ss]
        let frame = (kstack_top - 48) as *mut u64;
        *frame.add(0) = params.start_func; // RIP  (point d'entrée)
        *frame.add(1) = 0x1B; // CS   ring 3
        *frame.add(2) = 0x0202; // RFLAGS (IF=1)
        *frame.add(3) = params
            .attr
            .stack_addr
            .wrapping_add(params.attr.stack_size)
            .wrapping_sub(16); // RSP userspace
        *frame.add(4) = 0x23; // SS
                              // Argument dans rdi (convention System V).
                              // Stocké sur la stack avant le frame : sera chargé par le trampoline.
        let rdi_slot = (kstack_top - 56) as *mut u64;
        *rdi_slot = params.arg;

        (*thread_ptr).sched_tcb.kstack_ptr = kstack_top - 56;

        // Adresses userspace.
        (*thread_ptr).addresses = ThreadAddress {
            stack_base: params.attr.stack_addr,
            stack_size: params.attr.stack_size,
            entry_point: params.start_func,
            initial_rsp: params.attr.stack_addr + params.attr.stack_size - 16,
            tls_base: 0,
            pthread_ptr: params.pthread_out,
            sigaltstack_base: 0,
            sigaltstack_size: params.attr.sigaltstack_size,
        };

        // Afficité CPU.
        if params.attr.cpu_affinity >= 0 {
            let cpu = params.attr.cpu_affinity as u32;
            if cpu < MAX_CPUS as u32 {
                (*thread_ptr).sched_tcb.set_cpu_affinity_single(CpuId(cpu));
            }
        }

        // Thread détaché.
        if params.attr.detached {
            (*thread_ptr).detached.store(true, Ordering::Relaxed);
        }
    }

    // 4. Incrémenter le compteur de threads du PCB.
    pcb.inc_threads();

    // 5. Enqueuer dans la run queue.
    {
        let _preempt = PreemptGuard::new();
        if params.target_cpu as usize >= MAX_CPUS {
            pcb.dec_threads();
            // SAFETY: thread_ptr valide.
            unsafe {
                drop(Box::from_raw(thread_ptr));
            }
            TID_ALLOCATOR.free(tid_raw);
            return Err(ThreadCreateError::InvalidCpu);
        }
        // SAFETY: cpu vérifié, thread_ptr valide.
        unsafe {
            let tcb_ptr = NonNull::new_unchecked((*thread_ptr).tcb_ptr());
            run_queue(CpuId(params.target_cpu)).enqueue(tcb_ptr);
        }
    }

    Ok(ThreadHandle {
        tid,
        thread: thread_ptr,
    })
}
