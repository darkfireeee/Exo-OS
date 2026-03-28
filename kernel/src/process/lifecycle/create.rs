// kernel/src/process/lifecycle/create.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Création de processus et de threads kernel (Exo-OS Couche 1.5)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Entrées :
//   create_process() — crée un processus complet (PCB + thread principal)
//   create_kthread()  — crée un thread kernel pur (pid=1, KTHREAD flag)
//
// Séquence create_process() :
//   1. Allouer un PID + TID.
//   2. Créer le ProcessThread (stack kernel + TCB scheduler).
//   3. Créer le ProcessControlBlock.
//   4. Enregistrer dans PROCESS_REGISTRY.
//   5. Enregistrer le TCB dans la run queue du scheduler.
//   6. Retourner le PID.
// ═══════════════════════════════════════════════════════════════════════════════


use alloc::boxed::Box;
use core::ptr::NonNull;
use crate::process::core::pid::{Pid, Tid, PID_ALLOCATOR, TID_ALLOCATOR, PidAllocError};

// Trampoline de démarrage kthread — défini dans scheduler/asm/switch_asm.s.
// Appelé lors du premier context_switch vers un nouveau kthread.
// À l'entrée : r12 = entry_fn, r13 = arg. Place arg dans rdi puis jmp *r12.
extern "C" {
    fn kthread_trampoline();
}
use crate::process::core::pcb::{ProcessControlBlock, Credentials};
use crate::process::core::tcb::ProcessThread;
use crate::process::core::registry::PROCESS_REGISTRY;
use crate::scheduler::core::task::{SchedPolicy, Priority, ThreadId, CpuId};
use crate::scheduler::core::runqueue::run_queue;
use crate::scheduler::core::preempt::{PreemptGuard, MAX_CPUS};

// ─────────────────────────────────────────────────────────────────────────────
// Erreurs de création
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateError {
    /// Plus de PIDs disponibles.
    PidExhausted,
    /// Plus de TIDs disponibles.
    TidExhausted,
    /// Allocation mémoire échouée (stack kernel ou PCB).
    OutOfMemory,
    /// Erreur d'enregistrement dans la registry.
    RegistryError,
    /// CPU cible invalide.
    InvalidCpu,
}

impl From<PidAllocError> for CreateError {
    fn from(e: PidAllocError) -> Self {
        match e {
            PidAllocError::Exhausted   => CreateError::PidExhausted,
            PidAllocError::AlreadyUsed => CreateError::PidExhausted,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CreateParams — paramètres de création d'un processus
// ─────────────────────────────────────────────────────────────────────────────

/// Paramètres de création d'un processus utilisateur.
pub struct CreateParams {
    /// PID du processus parent (0 = init).
    pub ppid:        Pid,
    /// Credentials du nouveau processus.
    pub creds:       Credentials,
    /// CR3 initial (espace d'adressage vide — rempli par execve).
    pub cr3:         u64,
    /// Pointeur opaque vers l'espace d'adressage initial (peut être 0).
    pub addr_space:  usize,
    /// Politique d'ordonnancement du thread principal.
    pub policy:      SchedPolicy,
    /// Priorité initiale.
    pub priority:    Priority,
    /// CPU cible pour l'enfilement initial.
    pub target_cpu:  u32,
    /// Limite de FDs ouverts.
    pub fd_limit:    usize,
}

impl Default for CreateParams {
    fn default() -> Self {
        Self {
            ppid:       Pid::INIT,
            creds:      Credentials::new(1000, 1000),
            cr3:        0,
            addr_space: 0,
            policy:     SchedPolicy::Normal,
            priority:   Priority::NORMAL_DEFAULT,
            target_cpu: 0,
            fd_limit:   1024,
        }
    }
}

/// Handle de création — regroupe les objets créés pour les passer de façon atomique.
pub struct ProcessHandle {
    /// PID du processus créé.
    pub pid:    Pid,
    /// TID du thread principal.
    pub tid:    Tid,
    /// Pointeur raw vers le ProcessThread (géré par lifecycle).
    pub thread: *mut ProcessThread,
}

// SAFETY: ProcessHandle transféré entre threads uniquement lors de la création.
unsafe impl Send for ProcessHandle {}

// ─────────────────────────────────────────────────────────────────────────────
// create_process — création complète d'un processus utilisateur
// ─────────────────────────────────────────────────────────────────────────────

/// Crée un nouveau processus avec un thread principal.
///
/// Séquence :
///   1. Allouer PID + TID.
///   2. Créer ProcessThread (stack kernel + TCB scheduler).
///   3. Créer ProcessControlBlock.
///   4. Insérer dans PROCESS_REGISTRY.
///   5. Pousser le TCB dans la run queue.
///
/// # Safety
/// Appelé depuis le contexte d'un thread kernel avec préemption active.
pub fn create_process(params: &CreateParams) -> Result<ProcessHandle, CreateError> {
    // 1. Allouer PID et TID.
    let pid_raw = PID_ALLOCATOR.alloc()?;
    let tid_raw = TID_ALLOCATOR.alloc().map_err(|_| {
        // Libérer le PID déjà alloué avant de retourner l'erreur.
        PID_ALLOCATOR.free(pid_raw);
        CreateError::TidExhausted
    })?;

    let pid = Pid(pid_raw);
    let tid = Tid(tid_raw);

    // 2. Créer le ProcessThread (stack kernel alloué ici).
    let thread = ProcessThread::new(tid, pid, params.cr3, params.policy, params.priority)
        .ok_or_else(|| {
            PID_ALLOCATOR.free(pid_raw);
            TID_ALLOCATOR.free(tid_raw);
            CreateError::OutOfMemory
        })?;

    // Enregistrer le TID POSIX dans le TCB.
    let thread_ptr = Box::into_raw(thread);

    // 3. Créer le PCB.
    let pcb = ProcessControlBlock::new(
        pid,
        params.ppid,
        pid,  // tgid = pid pour le thread principal
        ThreadId(tid_raw as u64),
        params.creds,
        params.fd_limit,
        params.cr3,
        params.addr_space,
    );

    // 4. Insérer dans la registry.
    PROCESS_REGISTRY.insert(pcb).map_err(|_| {
        // SAFETY: thread_ptr a été créé par Box::into_raw juste au-dessus.
        unsafe { drop(Box::from_raw(thread_ptr)); }
        PID_ALLOCATOR.free(pid_raw);
        TID_ALLOCATOR.free(tid_raw);
        CreateError::RegistryError
    })?;

    // 5. Enregistrer le TCB dans la run queue du CPU cible.
    {
        let _preempt = PreemptGuard::new();
        let cpu_id = params.target_cpu;
        if cpu_id as usize >= MAX_CPUS {
            // CPU invalide — nettoyer et retourner erreur.
            let _ = PROCESS_REGISTRY.remove(pid);
            // SAFETY: thread_ptr créé par Box::into_raw(), non passé à la runqueue; Box::from_raw seul reclaim valide.
            unsafe { drop(Box::from_raw(thread_ptr)); }
            PID_ALLOCATOR.free(pid_raw);
            TID_ALLOCATOR.free(tid_raw);
            return Err(CreateError::InvalidCpu);
        }
        // SAFETY: cpu_id vérifié, thread_ptr valide, durée de vie gérée par lifecycle.
        unsafe {
            let tcb_ptr = NonNull::new_unchecked((*thread_ptr).tcb_ptr());
            run_queue(CpuId(cpu_id)).enqueue(tcb_ptr);
        }
    }

    Ok(ProcessHandle { pid, tid, thread: thread_ptr })
}

// ─────────────────────────────────────────────────────────────────────────────
// create_kthread — création d'un thread kernel dédié
// ─────────────────────────────────────────────────────────────────────────────

/// Paramètres d'un kthread.
pub struct KthreadParams {
    /// Nom du kthread (pour le debugging).
    pub name:       &'static str,
    /// Fonction d'entrée du kthread.
    pub entry:      fn(usize) -> !,
    /// Argument passé à `entry`.
    pub arg:        usize,
    /// CPU cible (0 = BSP).
    pub target_cpu: u32,
    /// Priorité (Normal par défaut).
    pub priority:   Priority,
}

/// Crée un thread kernel (kthread) et l'enfile dans la run queue.
///
/// Les kthreads :
///   - Appartiennent au processus système (PID 1).
///   - N'ont jamais d'espace d'adressage utilisateur.
///   - Ont le flag KTHREAD positionné dans le TCB.
///
/// # Safety
/// L'argument `arg` et le pointeur `entry` doivent rester valides
/// pendant toute la durée de vie du kthread.
pub fn create_kthread(params: &KthreadParams) -> Result<Tid, CreateError> {
    // Allouer un TID uniquement (kthread ne consomme pas de PID extra).
    let tid_raw = TID_ALLOCATOR.alloc().map_err(|_| CreateError::TidExhausted)?;
    let tid = Tid(tid_raw);

    // Créer le ProcessThread avec cr3=0 (espace kernel partagé).
    let thread = ProcessThread::new_kthread(tid, 0)
        .ok_or_else(|| {
            TID_ALLOCATOR.free(tid_raw);
            CreateError::OutOfMemory
        })?;

    let thread_ptr = Box::into_raw(thread);

    // Configurer le point d'entrée dans la stack kernel.
    // SAFETY: thread_ptr valide, kernel_stack alloué dedans.
    unsafe {
        // Frame attendu par switch_to_new_thread lors du PREMIER switch vers ce kthread.
        // switch_to_new_thread restaure dans cet ordre (SANS MXCSR/FCW) :
        //   popq %rbx           → rbx     [rsp+ 0]
        //   popq %rbp           → rbp     [rsp+ 8]
        //   popq %r12           → r12     [rsp+16]  ← entry_fn
        //   popq %r13           → r13     [rsp+24]  ← arg
        //   popq %r14           → r14     [rsp+32]
        //   popq %r15           → r15     [rsp+40]
        //   ret                 → rip     [rsp+48]  ← kthread_trampoline
        //
        // kthread_trampoline fait : mov r13, rdi ; jmp *r12
        // (arg → rdi, puis saute à entry_fn(arg))
        //
        // NOTE: context_switch_asm utilise un frame de 72 octets AVEC MXCSR+FCW.
        //       switch_to_new_thread utilise un frame de 56 octets SANS MXCSR+FCW.
        //       create_kthread() doit utiliser le format switch_to_new_thread (première activation).
        let stack_top = (*thread_ptr).kernel_stack.top_addr();
        const FRAME: u64 = 7 * 8; // 56 bytes — format switch_to_new_thread
        let kernel_rsp = stack_top - FRAME;
        let frame = kernel_rsp as *mut u64;
        *frame.add(0) = 0;                             // rbx
        *frame.add(1) = 0;                             // rbp
        *frame.add(2) = params.entry as u64;           // r12 → entry_fn
        *frame.add(3) = params.arg as u64;             // r13 → arg
        *frame.add(4) = 0;                             // r14
        *frame.add(5) = 0;                             // r15
        *frame.add(6) = kthread_trampoline as *const () as u64;     // return address → trampoline
        (*thread_ptr).sched_tcb.kstack_ptr = kernel_rsp;
    }

    // Enregistrer dans la run queue.
    {
        let _preempt = PreemptGuard::new();
        if params.target_cpu as usize >= MAX_CPUS {
            // SAFETY: thread_ptr créé via Box::into_raw() ci-dessus, pas encore enfilé.
            unsafe { drop(Box::from_raw(thread_ptr)); }
            TID_ALLOCATOR.free(tid_raw);
            return Err(CreateError::InvalidCpu);
        }
        // SAFETY: cpu valide, thread_ptr valide.
        unsafe {
            let tcb_ptr = NonNull::new_unchecked((*thread_ptr).tcb_ptr());
            run_queue(CpuId(params.target_cpu)).enqueue(tcb_ptr);
        }
    }

    Ok(tid)
}
