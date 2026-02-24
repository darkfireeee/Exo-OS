// kernel/src/process/lifecycle/fork.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// fork() — Duplication de processus Copy-on-Write (Exo-OS Couche 1.5)
// ═══════════════════════════════════════════════════════════════════════════════
//
// fork() sémantique POSIX :
//   • Copie le processus courant avec CoW sur les pages utilisateur.
//   • Hérite des fds, signaux masqués, credentials, namespaces.
//   • Le fils reçoit PID=nouveau / retour=0 ; le parent reçoit PID fils.
//
// RÈGLE PROC-08 : flush TLB parent AVANT retour de fork().
//
// Couche mémoire :
//   La duplication de l'espace d'adressage est déléguée à memory/cow/
//   via le trait abstrait AddressSpaceCloner (injection de dépendance).
// ═══════════════════════════════════════════════════════════════════════════════

#![allow(dead_code)]

use core::sync::atomic::Ordering;
use alloc::boxed::Box;
use spin::Once;
use crate::process::core::pid::{Pid, Tid, PID_ALLOCATOR, TID_ALLOCATOR};
use crate::process::core::pcb::{ProcessControlBlock, ProcessState, process_flags};
use crate::process::core::tcb::{ProcessThread, ThreadAddress};
use crate::process::core::registry::PROCESS_REGISTRY;
use crate::scheduler::core::task::{SchedPolicy, Priority, ThreadId, ProcessId, CpuId};
use crate::scheduler::core::runqueue::run_queue;
use crate::scheduler::core::preempt::{PreemptGuard, MAX_CPUS};

// ─────────────────────────────────────────────────────────────────────────────
// ForkFlags — flags CLONE_* Linux-compatibles
// ─────────────────────────────────────────────────────────────────────────────

/// Flags contrôlant le comportement de fork/clone.
#[derive(Copy, Clone, Default, Debug)]
pub struct ForkFlags(pub u32);

impl ForkFlags {
    /// Partager les fds (CLONE_FILES).
    pub const CLONE_FILES:     u32 = 1 << 0;
    /// Partager l'espace d'adressage (CLONE_VM = clone thread).
    pub const CLONE_VM:        u32 = 1 << 1;
    /// Partager les handlers de signaux (CLONE_SIGHAND).
    pub const CLONE_SIGHAND:   u32 = 1 << 2;
    /// Nouveau namespace PID (CLONE_NEWPID).
    pub const CLONE_NEWPID:    u32 = 1 << 3;
    /// Opération vfork (parent bloqué jusqu'au exec).
    pub const VFORK:           u32 = 1 << 4;
    /// Thread POSIX (CLONE_THREAD).
    pub const CLONE_THREAD:    u32 = 1 << 5;

    pub fn set(self, flag: u32) -> Self { Self(self.0 | flag) }
    pub fn has(self, flag: u32) -> bool { self.0 & flag != 0 }
}

// ─────────────────────────────────────────────────────────────────────────────
// Trait AddressSpaceCloner — injection de dépendance vers memory/cow/
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat de la duplication CoW de l'espace d'adressage.
pub struct ClonedAddressSpace {
    /// CR3 du nouvel espace d'adressage (fils).
    pub cr3:           u64,
    /// Pointeur opaque vers le UserAddressSpace fils.
    pub addr_space_ptr: usize,
}

/// Erreur de clonage de l'espace d'adressage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddrSpaceCloneError {
    OutOfMemory,
    InvalidSource,
}

/// Trait injecté par memory/ pour dupliquer un espace d'adressage en CoW.
pub trait AddressSpaceCloner: Send + Sync {
    /// Clone l'espace d'adressage référencé par `src_cr3`.
    fn clone_cow(
        &self,
        src_cr3:       u64,
        src_space_ptr: usize,
    ) -> Result<ClonedAddressSpace, AddrSpaceCloneError>;

    /// Flush le TLB d'un espace d'adressage après marquage CoW.
    fn flush_tlb_after_fork(&self, cr3: u64);
}

static ADDR_SPACE_CLONER: Once<&'static dyn AddressSpaceCloner> = Once::new();

/// Enregistre l'implémentation du clonage d'espace d'adressage (memory/ au boot).
pub fn register_addr_space_cloner(cloner: &'static dyn AddressSpaceCloner) {
    ADDR_SPACE_CLONER.call_once(|| cloner);
}

// ─────────────────────────────────────────────────────────────────────────────
// ForkError
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForkError {
    PidExhausted,
    TidExhausted,
    OutOfMemory,
    AddressSpaceCloneFailed,
    RegistryError,
    NoAddrCloner,
    InvalidCpu,
}

// ─────────────────────────────────────────────────────────────────────────────
// do_fork — implémentation principale
// ─────────────────────────────────────────────────────────────────────────────

/// Paramètres en entrée de do_fork().
pub struct ForkContext<'a> {
    /// ProcessThread du processus parent.
    pub parent_thread: &'a ProcessThread,
    /// PCB du processus parent.
    pub parent_pcb:    &'a ProcessControlBlock,
    /// Flags de fork (ForkFlags::*).
    pub flags:         ForkFlags,
    /// CPU cible pour le thread fils (typiquement CPU courant).
    pub target_cpu:    u32,
    /// RIP du fils au retour (après retour syscall, = point de fork).
    pub child_rip:     u64,
    /// RSP utilisateur du fils.
    pub child_rsp:     u64,
}

/// Résultat de fork() pour le parent.
pub struct ForkResult {
    /// PID du fils créé.
    pub child_pid: Pid,
    /// TID du thread principal du fils.
    pub child_tid: Tid,
}

/// Effectue un fork() POSIX : duplique le processus courant.
///
/// # RÈGLE PROC-08
/// Le TLB parent est flushé AVANT le retour pour invalider les PTEs qui
/// viennent d'être marquées read-only (CoW).
///
/// # Safety
/// `ctx.parent_thread` doit pointer vers le thread courant du processus.
pub fn do_fork(ctx: &ForkContext<'_>) -> Result<ForkResult, ForkError> {
    let parent = ctx.parent_thread;
    let parent_pcb = ctx.parent_pcb;

    // 1. Allouer PID + TID fils.
    let child_pid_raw = PID_ALLOCATOR.alloc().map_err(|_| ForkError::PidExhausted)?;
    let child_tid_raw = TID_ALLOCATOR.alloc().map_err(|_| {
        PID_ALLOCATOR.free(child_pid_raw);
        ForkError::TidExhausted
    })?;

    let child_pid = Pid(child_pid_raw);
    let child_tid = Tid(child_tid_raw);

    // 2. Cloner l'espace d'adressage (CoW).
    let cloner = ADDR_SPACE_CLONER.get().ok_or_else(|| {
        PID_ALLOCATOR.free(child_pid_raw);
        TID_ALLOCATOR.free(child_tid_raw);
        ForkError::NoAddrCloner
    })?;

    let parent_cr3       = parent.sched_tcb.cr3;
    let parent_space_ptr = parent_pcb.address_space.load(Ordering::Acquire);

    let cloned_as = cloner.clone_cow(parent_cr3, parent_space_ptr)
        .map_err(|_| {
            PID_ALLOCATOR.free(child_pid_raw);
            TID_ALLOCATOR.free(child_tid_raw);
            ForkError::AddressSpaceCloneFailed
        })?;

    // RÈGLE PROC-08 : Flush TLB parent AVANT retour (pages devenues read-only CoW).
    cloner.flush_tlb_after_fork(parent_cr3);

    // 3. Créer le ProcessThread fils.
    let policy   = parent.sched_tcb.policy;
    let priority = parent.sched_tcb.priority;

    let child_thread = ProcessThread::new(
        child_tid, child_pid, cloned_as.cr3, policy, priority,
    ).ok_or_else(|| {
        PID_ALLOCATOR.free(child_pid_raw);
        TID_ALLOCATOR.free(child_tid_raw);
        ForkError::OutOfMemory
    })?;

    let child_thread_ptr = Box::into_raw(child_thread);

    // Configurer le point de retour du fils (RIP + RSP au retour de fork).
    // SAFETY: child_thread_ptr valide.
    unsafe {
        let child_tcb = (*child_thread_ptr).tcb_mut();
        // Le fils retourne 0 via la convention de syscall (rax=0 dans le frame).
        // Ici on fixe le kernel RSP pour démarrer dans le trampoline fork_child_return.
        // L'architecture gère le retour userspace via iretq.
        let kstack_top = (*child_thread_ptr).kernel_stack.top_addr();
        // Frame minimal sur le stack kernel fils.
        let frame_ptr = (kstack_top - 48) as *mut u64;
        // [rip, cs, rflags, rsp, ss] — frame iretq.
        *frame_ptr.add(0) = ctx.child_rip;     // RIP  fils
        *frame_ptr.add(1) = 0x1B;              // CS   userspace (ring 3)
        *frame_ptr.add(2) = 0x0202;            // RFLAGS (IF=1, reserved=1)
        *frame_ptr.add(3) = ctx.child_rsp;     // RSP  userspace fils
        *frame_ptr.add(4) = 0x23;              // SS   userspace
        child_tcb.kernel_rsp = (kstack_top - 48) - 8; // retour via fork_child_trampoline
    }

    // Copier les adresses utilisateur.
    // SAFETY: child_thread_ptr valide.
    unsafe {
        (*child_thread_ptr).addresses = parent.addresses;
        (*child_thread_ptr).addresses.tls_base = parent.tls_gs_base.load(Ordering::Relaxed);
    }

    // 4. Créer le PCB fils — hérite des namespaces, credentials, etc.
    let parent_creds = parent_pcb.creds.lock()
        .clone();

    let mut child_pcb = ProcessControlBlock::new(
        child_pid,
        parent_pcb.pid,      // ppid = parent pid
        child_pid,           // tgid = child pid (nouveau process group leader thread)
        ThreadId(child_tid_raw),
        parent_creds,
        {
        let f = parent_pcb.files.lock();
            f.open_fd_count().max(1024)
        },
        cloned_as.cr3,
        cloned_as.addr_space_ptr,
    );

    // Copier les fds si !CLONE_FILES.
    if !ctx.flags.has(ForkFlags::CLONE_FILES) {
        let cloned_files = {
            let f = parent_pcb.files.lock();
            f.clone_for_fork()
        };
        *child_pcb.files.lock() = cloned_files;
    }

    // Copier les namespaces.
    child_pcb.pid_ns.clone_from(&parent_pcb.pid_ns);
    child_pcb.mnt_ns.clone_from(&parent_pcb.mnt_ns);
    child_pcb.net_ns.clone_from(&parent_pcb.net_ns);
    child_pcb.uts_ns.clone_from(&parent_pcb.uts_ns);
    child_pcb.user_ns.clone_from(&parent_pcb.user_ns);

    // Marquer FORKED.
    child_pcb.flags.fetch_or(process_flags::FORKED, Ordering::Release);
    child_pcb.set_state(ProcessState::Running);

    // Copier les valeurs brk.
    child_pcb.brk_start.store(parent_pcb.brk_start.load(Ordering::Relaxed), Ordering::Relaxed);
    child_pcb.brk_current.store(parent_pcb.brk_current.load(Ordering::Relaxed), Ordering::Relaxed);

    // 5. Insérer le fils dans la registry.
    PROCESS_REGISTRY.insert(child_pcb).map_err(|_| {
        // SAFETY: child_thread_ptr valide.
        unsafe { drop(Box::from_raw(child_thread_ptr)); }
        PID_ALLOCATOR.free(child_pid_raw);
        TID_ALLOCATOR.free(child_tid_raw);
        ForkError::RegistryError
    })?;

    // 6. Enqueue le fils dans la run queue.
    {
        let _preempt = PreemptGuard::new();
        if ctx.target_cpu as usize >= MAX_CPUS {
            let _ = PROCESS_REGISTRY.remove(child_pid);
            // SAFETY: child_thread_ptr créé via Box::into_raw() ci-dessus, pas encore enfilé.
            unsafe { drop(Box::from_raw(child_thread_ptr)); }
            PID_ALLOCATOR.free(child_pid_raw);
            TID_ALLOCATOR.free(child_tid_raw);
            return Err(ForkError::InvalidCpu);
        }
        // SAFETY: cpu vérifié, child_thread_ptr valide.
        unsafe {
            let tcb_ptr = core::ptr::NonNull::new_unchecked((*child_thread_ptr).tcb_ptr());
            run_queue(CpuId(ctx.target_cpu)).enqueue(tcb_ptr);
        }
    }

    Ok(ForkResult { child_pid, child_tid })
}
