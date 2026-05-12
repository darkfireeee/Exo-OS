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

use crate::process::core::pid::{Pid, PidAllocError, Tid, PID_ALLOCATOR, TID_ALLOCATOR};
use alloc::boxed::Box;
use core::ptr::NonNull;
use core::sync::atomic::Ordering;

// Trampoline de démarrage kthread — défini dans scheduler/asm/switch_asm.s.
// Appelé lors du premier context_switch vers un nouveau kthread.
// À l'entrée : r12 = entry_fn, r13 = arg. Place arg dans rdi puis jmp *r12.
extern "C" {
    fn kthread_trampoline();
    fn user_entry_trampoline();
}
#[cfg(target_arch = "x86_64")]
use crate::arch::x86_64::gdt::{GDT_USER_CS64, GDT_USER_DS};
use crate::process::core::pcb::{process_flags, Credentials, ProcessControlBlock, ProcessState};
use crate::process::core::registry::PROCESS_REGISTRY;
use crate::process::core::tcb::{ProcessThread, ThreadAddress};
use crate::process::lifecycle::exec::ElfLoadResult;
use crate::scheduler::core::preempt::{PreemptGuard, MAX_CPUS};
use crate::scheduler::core::runqueue::run_queue;
use crate::scheduler::core::task::{CpuId, Priority, SchedPolicy, TaskState, ThreadId};

#[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
fn debug_hex64(value: u64) {
    let mut buf = [0u8; 16];
    let mut shift = 60u32;
    for byte in &mut buf {
        let nibble = ((value >> shift) & 0x0f) as u8;
        *byte = if nibble < 10 {
            b'0' + nibble
        } else {
            b'a' + (nibble - 10)
        };
        shift = shift.saturating_sub(4);
    }
    crate::arch::x86_64::terminal::debug_write(&buf);
}

#[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
fn debug_init_as(label: &[u8], cr3: u64, addr_space: usize) {
    let out = crate::arch::x86_64::terminal::debug_write;
    out(label);
    out(b" cr3=0x");
    debug_hex64(cr3);
    out(b" as=0x");
    debug_hex64(addr_space as u64);
    out(b"\n");
}

#[inline]
fn sync_process_kernel_half(cr3: u64) {
    if cr3 == 0 {
        return;
    }
    unsafe {
        crate::memory::virt::address_space::KERNEL_AS
            .sync_kernel_half_into(crate::memory::core::PhysAddr::new(cr3));
    }
}

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
            PidAllocError::Exhausted => CreateError::PidExhausted,
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
    pub ppid: Pid,
    /// Credentials du nouveau processus.
    pub creds: Credentials,
    /// CR3 initial (espace d'adressage vide — rempli par execve).
    pub cr3: u64,
    /// Pointeur opaque vers l'espace d'adressage initial (peut être 0).
    pub addr_space: usize,
    /// Politique d'ordonnancement du thread principal.
    pub policy: SchedPolicy,
    /// Priorité initiale.
    pub priority: Priority,
    /// CPU cible pour l'enfilement initial.
    pub target_cpu: u32,
    /// Limite de FDs ouverts.
    pub fd_limit: usize,
}

impl Default for CreateParams {
    fn default() -> Self {
        Self {
            ppid: Pid::INIT,
            creds: Credentials::new(1000, 1000),
            cr3: 0,
            addr_space: 0,
            policy: SchedPolicy::Normal,
            priority: Priority::NORMAL_DEFAULT,
            target_cpu: 0,
            fd_limit: 1024,
        }
    }
}

/// Handle de création — regroupe les objets créés pour les passer de façon atomique.
pub struct ProcessHandle {
    /// PID du processus créé.
    pub pid: Pid,
    /// TID du thread principal.
    pub tid: Tid,
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
    let pcb = ProcessControlBlock::try_new(
        pid,
        params.ppid,
        pid, // tgid = pid pour le thread principal
        ThreadId(tid_raw as u64),
        params.creds,
        params.fd_limit,
        params.cr3,
        params.addr_space,
    )
    .ok_or_else(|| {
        // SAFETY: thread_ptr a été créé par Box::into_raw juste au-dessus.
        unsafe {
            drop(Box::from_raw(thread_ptr));
        }
        PID_ALLOCATOR.free(pid_raw);
        TID_ALLOCATOR.free(tid_raw);
        CreateError::OutOfMemory
    })?;
    pcb.set_main_thread_ptr(thread_ptr);

    // 4. Insérer dans la registry.
    PROCESS_REGISTRY.insert(pcb).map_err(|_| {
        // SAFETY: thread_ptr a été créé par Box::into_raw juste au-dessus.
        unsafe {
            drop(Box::from_raw(thread_ptr));
        }
        PID_ALLOCATOR.free(pid_raw);
        TID_ALLOCATOR.free(tid_raw);
        CreateError::RegistryError
    })?;
    sync_process_kernel_half(params.cr3);

    // 5. Enregistrer le TCB dans la run queue du CPU cible.
    {
        let _preempt = PreemptGuard::new();
        let cpu_id = params.target_cpu;
        if cpu_id as usize >= MAX_CPUS {
            // CPU invalide — nettoyer et retourner erreur.
            let _ = PROCESS_REGISTRY.remove(pid);
            // SAFETY: thread_ptr créé par Box::into_raw(), non passé à la runqueue; Box::from_raw seul reclaim valide.
            unsafe {
                drop(Box::from_raw(thread_ptr));
            }
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

    Ok(ProcessHandle {
        pid,
        tid,
        thread: thread_ptr,
    })
}

/// Cree le vrai PID 1 depuis une image ELF deja chargee.
///
/// `pid::init()` reserve PID 1 dans le bitmap: ce chemin n'alloue donc pas de
/// PID, il publie explicitement le PCB `init` dans la registry puis enfile son
/// thread principal. Le premier switch vers ce thread passe par
/// `user_entry_trampoline`, qui prepare les registres ABI minimaux avant l'IRET
/// ring3.
pub fn create_init_process_from_elf(elf: ElfLoadResult) -> Result<ProcessHandle, CreateError> {
    if PROCESS_REGISTRY.find_by_pid(Pid::INIT).is_some() {
        return Err(CreateError::RegistryError);
    }
    #[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
    debug_init_as(b"init_create: elf", elf.cr3, elf.addr_space_ptr);

    let tid_raw = TID_ALLOCATOR
        .alloc()
        .map_err(|_| CreateError::TidExhausted)?;
    let tid = Tid(tid_raw);
    let pid = Pid::INIT;

    let thread = ProcessThread::new(
        tid,
        pid,
        elf.cr3,
        SchedPolicy::Normal,
        Priority::NORMAL_DEFAULT,
    )
    .ok_or_else(|| {
        TID_ALLOCATOR.free(tid_raw);
        CreateError::OutOfMemory
    })?;

    let thread_ptr = Box::into_raw(thread);

    const USER_STACK_PAGES: u64 = 8;
    const PAGE_SIZE_U64: u64 = crate::memory::core::PAGE_SIZE as u64;
    const USER_STACK_SIZE: u64 = USER_STACK_PAGES * PAGE_SIZE_U64;
    let stack_top = elf.initial_stack_top;
    let stack_base = stack_top.saturating_sub(USER_STACK_SIZE) & !(PAGE_SIZE_U64 - 1);
    let stack_size = stack_top.saturating_sub(stack_base);

    unsafe {
        let thread = &mut *thread_ptr;
        thread.sched_tcb.cr3_phys = elf.cr3;
        thread.sched_tcb.fs_base = elf.tls_base;
        thread.sched_tcb.user_gs_base = 0;
        thread.sched_tcb.signal_mask.store(0, Ordering::Release);
        thread.sched_tcb.set_state(TaskState::Runnable);
        thread.addresses = ThreadAddress {
            stack_base,
            stack_size,
            entry_point: elf.entry_point,
            initial_rsp: elf.initial_stack_top,
            tls_base: elf.tls_base,
            pthread_ptr: 0,
            sigaltstack_base: 0,
            sigaltstack_size: 0,
        };
        thread.tls_gs_base.store(elf.tls_base, Ordering::Release);
        thread.tls_size = elf.tls_size;

        let kstack_top = thread.kernel_stack.top_addr();
        let kernel_rsp = kstack_top - 96;
        let frame = kernel_rsp as *mut u64;
        *frame.add(0) = 0; // rbx
        *frame.add(1) = 0; // rbp
        *frame.add(2) = 0; // r12
        *frame.add(3) = 0; // r13
        *frame.add(4) = 0; // r14
        *frame.add(5) = 0; // r15
        *frame.add(6) = user_entry_trampoline as *const () as u64;
        *frame.add(7) = elf.entry_point; // RIP userspace
        *frame.add(8) = GDT_USER_CS64 as u64; // CS ring3 64-bit
        *frame.add(9) = 0x0202; // RFLAGS: reserved bit + IF
        *frame.add(10) = elf.initial_stack_top; // RSP userspace
        *frame.add(11) = GDT_USER_DS as u64; // SS ring3
        thread.sched_tcb.kstack_ptr = kernel_rsp;
    }

    let pcb = ProcessControlBlock::try_new(
        pid,
        Pid::IDLE,
        pid,
        ThreadId(tid_raw as u64),
        Credentials::ROOT,
        1024,
        elf.cr3,
        elf.addr_space_ptr,
    )
    .ok_or_else(|| {
        unsafe {
            drop(Box::from_raw(thread_ptr));
        }
        TID_ALLOCATOR.free(tid_raw);
        CreateError::OutOfMemory
    })?;
    #[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
    debug_init_as(
        b"init_create: pcb",
        pcb.cr3.load(Ordering::Relaxed),
        pcb.address_space.load(Ordering::Relaxed),
    );
    pcb.set_main_thread_ptr(thread_ptr);
    pcb.brk_start.store(elf.brk_start, Ordering::Release);
    pcb.brk_current.store(elf.brk_start, Ordering::Release);
    pcb.flags.fetch_or(
        process_flags::EXEC_DONE | process_flags::VFORK_DONE,
        Ordering::Release,
    );
    {
        const BOOT_TTY_HANDLE: u64 = 1;
        pcb.files
            .lock()
            .install_std_fds(BOOT_TTY_HANDLE, BOOT_TTY_HANDLE, BOOT_TTY_HANDLE);
    }
    pcb.set_state(ProcessState::Running);

    PROCESS_REGISTRY.insert(pcb).map_err(|_| {
        unsafe {
            drop(Box::from_raw(thread_ptr));
        }
        TID_ALLOCATOR.free(tid_raw);
        CreateError::RegistryError
    })?;
    sync_process_kernel_half(elf.cr3);

    {
        let _preempt = PreemptGuard::new();
        unsafe {
            let tcb_ptr = NonNull::new_unchecked((*thread_ptr).tcb_ptr());
            run_queue(CpuId(0)).enqueue(tcb_ptr);
        }
    }

    Ok(ProcessHandle {
        pid,
        tid,
        thread: thread_ptr,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// create_kthread — création d'un thread kernel dédié
// ─────────────────────────────────────────────────────────────────────────────

/// Paramètres d'un kthread.
pub struct KthreadParams {
    /// Nom du kthread (pour le debugging).
    pub name: &'static str,
    /// Fonction d'entrée du kthread.
    pub entry: fn(usize) -> !,
    /// Argument passé à `entry`.
    pub arg: usize,
    /// CPU cible (0 = BSP).
    pub target_cpu: u32,
    /// Priorité (Normal par défaut).
    pub priority: Priority,
}

/// Crée un thread kernel (kthread) et l'enfile dans la run queue.
///
/// Les kthreads :
///   - Appartiennent au domaine noyau (PID 0 côté scheduler).
///   - N'ont jamais d'espace d'adressage utilisateur.
///   - Ont le flag KTHREAD positionné dans le TCB.
///
/// # Safety
/// L'argument `arg` et le pointeur `entry` doivent rester valides
/// pendant toute la durée de vie du kthread.
pub fn create_kthread(params: &KthreadParams) -> Result<Tid, CreateError> {
    // Allouer un TID uniquement (kthread ne consomme pas de PID extra).
    let tid_raw = TID_ALLOCATOR
        .alloc()
        .map_err(|_| CreateError::TidExhausted)?;
    let tid = Tid(tid_raw);

    // Créer le ProcessThread avec cr3=0 (espace kernel partagé).
    let thread = ProcessThread::new_kthread(tid, 0).ok_or_else(|| {
        TID_ALLOCATOR.free(tid_raw);
        CreateError::OutOfMemory
    })?;

    let thread_ptr = Box::into_raw(thread);

    // Configurer le point d'entrée dans la stack kernel.
    // SAFETY: thread_ptr valide, kernel_stack alloué dedans.
    unsafe {
        (*thread_ptr).sched_tcb.priority = params.priority;

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
        *frame.add(0) = 0; // rbx
        *frame.add(1) = 0; // rbp
        *frame.add(2) = params.entry as u64; // r12 → entry_fn
        *frame.add(3) = params.arg as u64; // r13 → arg
        *frame.add(4) = 0; // r14
        *frame.add(5) = 0; // r15
        *frame.add(6) = kthread_trampoline as *const () as u64; // return address → trampoline
        (*thread_ptr).sched_tcb.kstack_ptr = kernel_rsp;
    }
    // Enregistrer dans la run queue.
    {
        let _preempt = PreemptGuard::new();
        if params.target_cpu as usize >= MAX_CPUS {
            // SAFETY: thread_ptr créé via Box::into_raw() ci-dessus, pas encore enfilé.
            unsafe {
                drop(Box::from_raw(thread_ptr));
            }
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
