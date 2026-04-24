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

use crate::process::core::pcb::{process_flags, ProcessControlBlock, ProcessState};
use crate::process::core::pid::{Pid, Tid, PID_ALLOCATOR, TID_ALLOCATOR};
use crate::process::core::registry::PROCESS_REGISTRY;
use crate::process::core::tcb::ProcessThread;
use crate::scheduler::core::preempt::{PreemptGuard, MAX_CPUS};
use crate::scheduler::core::runqueue::run_queue;
use crate::scheduler::core::task::ThreadControlBlock;
use crate::scheduler::core::task::{CpuId, ThreadId};
use crate::scheduler::sync::wait_queue::WaitQueue;
use alloc::boxed::Box;
use core::sync::atomic::Ordering;
use spin::Once;

// ─────────────────────────────────────────────────────────────────────────────
// ForkFlags — flags CLONE_* Linux-compatibles
// ─────────────────────────────────────────────────────────────────────────────

/// Flags contrôlant le comportement de fork/clone.
#[derive(Copy, Clone, Default, Debug)]
pub struct ForkFlags(pub u32);

impl ForkFlags {
    /// Partager les fds (CLONE_FILES).
    pub const CLONE_FILES: u32 = 1 << 0;
    /// Partager l'espace d'adressage (CLONE_VM = clone thread).
    pub const CLONE_VM: u32 = 1 << 1;
    /// Partager les handlers de signaux (CLONE_SIGHAND).
    pub const CLONE_SIGHAND: u32 = 1 << 2;
    /// Nouveau namespace PID (CLONE_NEWPID).
    pub const CLONE_NEWPID: u32 = 1 << 3;
    /// Opération vfork (parent bloqué jusqu'au exec).
    pub const VFORK: u32 = 1 << 4;
    /// Thread POSIX (CLONE_THREAD).
    pub const CLONE_THREAD: u32 = 1 << 5;

    pub fn set(self, flag: u32) -> Self {
        Self(self.0 | flag)
    }
    pub fn has(self, flag: u32) -> bool {
        self.0 & flag != 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Trait AddressSpaceCloner — injection de dépendance vers memory/cow/
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat de la duplication CoW de l'espace d'adressage.
pub struct ClonedAddressSpace {
    /// CR3 du nouvel espace d'adressage (fils).
    pub cr3: u64,
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
        src_cr3: u64,
        src_space_ptr: usize,
    ) -> Result<ClonedAddressSpace, AddrSpaceCloneError>;

    /// Flush le TLB d'un espace d'adressage après marquage CoW.
    fn flush_tlb_after_fork(&self, cr3: u64);

    /// Libère un espace d'adressage cloné (appelé sur erreur post-clone).
    ///
    /// CORRECTION P0-01 : évite les fuites mémoire du PML4 en cas d'erreur
    /// dans un chemin d'erreur tardif de do_fork() (RegistryError, InvalidCpu).
    fn free_addr_space(&self, addr_space_ptr: usize);
}

static ADDR_SPACE_CLONER: Once<&'static dyn AddressSpaceCloner> = Once::new();
static VFORK_WAIT_QUEUE: WaitQueue = WaitQueue::new();

/// Enregistre l'implémentation du clonage d'espace d'adressage (memory/ au boot).
pub fn register_addr_space_cloner(cloner: &'static dyn AddressSpaceCloner) {
    ADDR_SPACE_CLONER.call_once(|| cloner);
}

#[inline]
fn vfork_completion_reached(child_pid: Pid) -> bool {
    match PROCESS_REGISTRY.find_by_pid(child_pid) {
        None => true,
        Some(pcb) => {
            let flags = pcb.flags.load(Ordering::Acquire);
            let state = pcb.state();
            state == ProcessState::Zombie
                || state == ProcessState::Dead
                || (flags & (process_flags::EXEC_DONE | process_flags::VFORK_DONE)) != 0
        }
    }
}

pub fn wait_for_vfork_completion(
    child_pid: Pid,
    caller_tcb: &ThreadControlBlock,
) -> Result<(), ()> {
    while !vfork_completion_reached(child_pid) {
        let woke = unsafe { VFORK_WAIT_QUEUE.wait_interruptible(caller_tcb as *const _ as *mut _) };
        if !woke && !vfork_completion_reached(child_pid) {
            return Err(());
        }
    }
    Ok(())
}

#[inline]
pub fn notify_vfork_completion(_child_pid: Pid) {
    VFORK_WAIT_QUEUE.notify_all();
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
    pub parent_pcb: &'a ProcessControlBlock,
    /// Flags de fork (ForkFlags::*).
    pub flags: ForkFlags,
    /// CPU cible pour le thread fils (typiquement CPU courant).
    pub target_cpu: u32,
    /// RIP du fils au retour (après retour syscall, = point de fork).
    pub child_rip: u64,
    /// RSP utilisateur du fils.
    pub child_rsp: u64,
    /// RFLAGS du parent au moment du fork (depuis frame.r11 sauvé par SYSCALL).
    /// CORRECTION P2-02 : propagé au fils avec masquage sécurisé.
    pub parent_rflags: u64,
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

    let parent_cr3 = parent.sched_tcb.cr3_phys;
    let parent_space_ptr = parent_pcb.address_space.load(Ordering::Acquire);

    let cloned_as = cloner
        .clone_cow(parent_cr3, parent_space_ptr)
        .map_err(|_| {
            PID_ALLOCATOR.free(child_pid_raw);
            TID_ALLOCATOR.free(child_tid_raw);
            ForkError::AddressSpaceCloneFailed
        })?;

    if cloned_as.addr_space_ptr != 0 {
        let child_as = unsafe {
            &mut *(cloned_as.addr_space_ptr as *mut crate::memory::virt::UserAddressSpace)
        };
        child_as.pid = child_pid.0 as u64;
    }

    // RÈGLE PROC-08 : Flush TLB parent AVANT retour (pages devenues read-only CoW).
    cloner.flush_tlb_after_fork(parent_cr3);

    // 3. Créer le ProcessThread fils.
    let policy = parent.sched_tcb.policy;
    let priority = parent.sched_tcb.priority;

    let child_thread = ProcessThread::new(child_tid, child_pid, cloned_as.cr3, policy, priority)
        .ok_or_else(|| {
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
        // switch_to_new_thread restaure le stack puis ret vers fork_child_trampoline.
        // fork_child_trampoline : xor eax,eax ; swapgs ; iretq → userspace.
        // SAFETY: fork_child_trampoline est défini dans switch_asm.s, lié statiquement.
        extern "C" {
            fn fork_child_trampoline();
        }

        let kstack_top = (*child_thread_ptr).kernel_stack.top_addr();
        // Frame complet : 12 × u64 = 96 bytes.
        // Layout (indices depuis kernel_rsp = kstack_top - 96) :
        //   [0..48)  = 6 callee-saved regs = 0       ← format switch_to_new_thread
        //   [48]     = fork_child_trampoline          ← adresse ret après 6 pops
        //   [56..96) = iretq frame (RIP, CS, RFLAGS, RSP, SS)
        let frame_ptr = (kstack_top - 96) as *mut u64;
        // Callee-saved registers. switch_to_new_thread les dépile avant le ret.
        *frame_ptr.add(0) = 0; // rbx
        *frame_ptr.add(1) = 0; // rbp
        *frame_ptr.add(2) = 0; // r12
        *frame_ptr.add(3) = 0; // r13
        *frame_ptr.add(4) = 0; // r14
        *frame_ptr.add(5) = 0; // r15
                               // Adresse de retour : fork_child_trampoline (exécuté après les 6 pops).
        *frame_ptr.add(6) = fork_child_trampoline as *const () as u64;
        // iretq frame (RSP pointe ici à l'entrée fork_child_trampoline).
        *frame_ptr.add(7) = ctx.child_rip; // RIP  userspace
        *frame_ptr.add(8) = 0x1B; // CS   ring3 (code64)

        // CORRECTION P2-02 : propager les RFLAGS du parent avec masquage sécurisé.
        // Masque des flags sûrs à hériter (POSIX + sécurité kernel).
        // - Conserver : CF(0), PF(2), AF(4), ZF(6), SF(7), OF(11), DF(10), AC(18), ID(21)
        // - Forcer  : IF=1 (bit 9) — le fils doit accepter les interruptions
        // - Effacer : TF=0 (bit 8) — ne pas tracer le fils si le parent était en trace
        //             NT=0 (bit 14) — Nested Task flag — jamais hérité
        //             RF=0 (bit 16) — Resume Flag — jamais hérité
        //             VM=0 (bit 17) — Virtual 8086 — non supporté
        const RFLAGS_SAFE_MASK: u64 = 0x0000_0000_0020_0CD5; // CF,PF,AF,ZF,SF,DF,OF,AC,ID
        const RFLAGS_FORCE_SET: u64 = 0x0000_0000_0000_0200; // IF=1
        const RFLAGS_FORCE_CLR: u64 = 0x0000_0000_0004_0100; // TF=0, NT=0, RF=0, VM=0

        let child_rflags =
            ((ctx.parent_rflags & RFLAGS_SAFE_MASK) | RFLAGS_FORCE_SET) & !RFLAGS_FORCE_CLR;
        // Garantir que le bit réservé 1 est toujours à 1.
        let child_rflags = child_rflags | 0x0002;

        *frame_ptr.add(9) = child_rflags; // RFLAGS (hérités du parent avec masquage)
        *frame_ptr.add(10) = ctx.child_rsp; // RSP  userspace
        *frame_ptr.add(11) = 0x23; // SS   ring3
        child_tcb.kstack_ptr = kstack_top - 96;
    }

    // Copier les adresses utilisateur.
    // SAFETY: child_thread_ptr valide.
    unsafe {
        (*child_thread_ptr).addresses = parent.addresses;
        (*child_thread_ptr).addresses.tls_base = parent.tls_gs_base.load(Ordering::Relaxed);
    }

    // 4. Créer le PCB fils — hérite des namespaces, credentials, etc.
    let parent_creds = parent_pcb.creds.lock().clone();

    let mut child_pcb = ProcessControlBlock::new(
        child_pid,
        parent_pcb.pid, // ppid = parent pid
        child_pid,      // tgid = child pid (nouveau process group leader thread)
        ThreadId(child_tid_raw as u64),
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
    child_pcb
        .flags
        .fetch_or(process_flags::FORKED, Ordering::Release);
    child_pcb.set_state(ProcessState::Running);

    // Copier les valeurs brk.
    child_pcb.brk_start.store(
        parent_pcb.brk_start.load(Ordering::Relaxed),
        Ordering::Relaxed,
    );
    child_pcb.brk_current.store(
        parent_pcb.brk_current.load(Ordering::Relaxed),
        Ordering::Relaxed,
    );

    // 5. Insérer le fils dans la registry.
    PROCESS_REGISTRY.insert(child_pcb).map_err(|_| {
        // SAFETY: child_thread_ptr valide.
        unsafe {
            drop(Box::from_raw(child_thread_ptr));
        }
        // CORRECTION P0-01 : libérer l'espace d'adressage cloné pour éviter la fuite mémoire
        if let Some(cl) = ADDR_SPACE_CLONER.get() {
            cl.free_addr_space(cloned_as.addr_space_ptr);
        }
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
            unsafe {
                drop(Box::from_raw(child_thread_ptr));
            }
            // CORRECTION P0-01 : libérer l'espace d'adressage cloné pour éviter la fuite mémoire
            if let Some(cl) = ADDR_SPACE_CLONER.get() {
                cl.free_addr_space(cloned_as.addr_space_ptr);
            }
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

    Ok(ForkResult {
        child_pid,
        child_tid,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::core::pcb::Credentials;
    use crate::process::core::registry;
    use std::sync::Once as StdOnce;

    fn init_registry_for_tests() {
        static INIT: StdOnce = StdOnce::new();
        INIT.call_once(|| unsafe {
            registry::init(4096);
        });
    }

    fn cleanup_pid(pid: Pid) {
        let _ = PROCESS_REGISTRY.remove(pid);
    }

    fn insert_test_pcb(pid: Pid) {
        cleanup_pid(pid);
        let pcb = ProcessControlBlock::new(
            pid,
            Pid(1),
            pid,
            ThreadId(pid.0 as u64),
            Credentials::ROOT,
            32,
            0,
            0,
        );
        pcb.set_state(ProcessState::Running);
        PROCESS_REGISTRY.insert(pcb).unwrap();
    }

    #[test]
    fn test_vfork_completion_reached_when_exec_done() {
        init_registry_for_tests();
        let pid = Pid(901);
        insert_test_pcb(pid);
        let pcb = PROCESS_REGISTRY.find_by_pid(pid).unwrap();
        pcb.flags
            .fetch_or(process_flags::EXEC_DONE, Ordering::Release);
        assert!(vfork_completion_reached(pid));
        cleanup_pid(pid);
    }

    #[test]
    fn test_vfork_completion_reached_stress_for_terminal_states() {
        init_registry_for_tests();

        for offset in 0..128u32 {
            let pid = Pid(920 + offset);
            insert_test_pcb(pid);
            let pcb = PROCESS_REGISTRY.find_by_pid(pid).unwrap();
            if offset & 1 == 0 {
                pcb.flags
                    .fetch_or(process_flags::VFORK_DONE, Ordering::Release);
            } else {
                pcb.set_state(ProcessState::Zombie);
            }
            assert!(vfork_completion_reached(pid));
            cleanup_pid(pid);
        }
    }
}
