// kernel/src/scheduler/core/boot_idle.rs
//
// TCB idle de bootstrap par CPU.
//
// Ce module fournit un TCB idle statique pour chaque CPU logique.
// Il sert de "thread courant" canonique pendant les phases BSP/AP où le CPU
// exécute déjà sa boucle idle native (`hlt`) mais avant qu'un vrai thread
// utilisateur ou kthread soit planifié sur ce CPU.

use super::preempt::MAX_CPUS;
use super::runqueue::run_queue;
use super::switch::CURRENT_THREAD_PER_CPU;
use super::task::{CpuId, Priority, ProcessId, SchedPolicy, TaskState, ThreadControlBlock, ThreadId};
use core::mem::MaybeUninit;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, Ordering};

const BOOT_IDLE_TID_BASE: u64 = 0x8000_0000;

static mut BOOT_IDLE_TCBS: [MaybeUninit<ThreadControlBlock>; MAX_CPUS] = {
    // SAFETY: tableau de MaybeUninit explicitement non initialisé.
    unsafe { MaybeUninit::uninit().assume_init() }
};
static BOOT_IDLE_INIT: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];

unsafe fn boot_idle_slot(cpu_id: usize) -> &'static mut MaybeUninit<ThreadControlBlock> {
    // SAFETY: l'appelant borne cpu_id à MAX_CPUS et l'accès est par CPU unique.
    unsafe { &mut BOOT_IDLE_TCBS[cpu_id] }
}

/// Garantit qu'un TCB idle statique existe pour `cpu_id`.
///
/// # Safety
/// Doit être appelé avec un `cpu_id` valide. L'appelant garantit qu'un même CPU
/// n'initialise pas son slot concurremment avec un autre contexte.
pub unsafe fn ensure_boot_idle_tcb(
    cpu_id: u32,
    kernel_stack_top: u64,
) -> Option<NonNull<ThreadControlBlock>> {
    let idx = cpu_id as usize;
    if idx >= MAX_CPUS {
        return None;
    }

    if !BOOT_IDLE_INIT[idx].load(Ordering::Acquire) {
        let mut idle = ThreadControlBlock::new(
            ThreadId(BOOT_IDLE_TID_BASE + cpu_id as u64),
            ProcessId(0),
            SchedPolicy::Idle,
            Priority::IDLE,
            crate::arch::x86_64::read_cr3(),
            kernel_stack_top,
        );
        idle.assign_cpu(CpuId(cpu_id));
        idle.set_cpu_affinity_single(CpuId(cpu_id));
        crate::scheduler::policies::mark_idle_thread(&mut idle);

        // SAFETY: slot unique par CPU, initialisé une seule fois en pratique.
        unsafe {
            boot_idle_slot(idx).write(idle);
        }
        BOOT_IDLE_INIT[idx].store(true, Ordering::Release);
    }

    // SAFETY: le slot est initialisé juste au-dessus ou l'était déjà.
    let ptr = unsafe { boot_idle_slot(idx).assume_init_mut() as *mut ThreadControlBlock };
    Some(unsafe { NonNull::new_unchecked(ptr) })
}

/// Publie le TCB idle de bootstrap comme thread courant du CPU appelant.
///
/// # Safety
/// GS per-CPU doit déjà être initialisé pour ce CPU.
pub unsafe fn publish_current_boot_idle(
    cpu_id: u32,
    kernel_stack_top: u64,
) -> Option<NonNull<ThreadControlBlock>> {
    let idle = unsafe { ensure_boot_idle_tcb(cpu_id, kernel_stack_top)? };
    // SAFETY: ce TCB représente le contexte courant de ce CPU.
    unsafe {
        idle.as_ref().set_state(TaskState::Running);
        crate::arch::x86_64::smp::percpu::set_current_tcb(idle.as_ptr());
        crate::arch::x86_64::smp::percpu::set_kernel_rsp(kernel_stack_top);
    }
    CURRENT_THREAD_PER_CPU[cpu_id as usize].store(idle.as_ptr() as usize, Ordering::Release);
    Some(idle)
}

/// Relie les TCB idle déjà publiés aux run queues per-CPU après `scheduler::init()`.
///
/// # Safety
/// Les run queues pour `nr_cpus` doivent déjà avoir été initialisées.
pub unsafe fn bind_boot_idle_threads(nr_cpus: usize) {
    for idx in 0..nr_cpus.min(MAX_CPUS) {
        if !BOOT_IDLE_INIT[idx].load(Ordering::Acquire) {
            continue;
        }

        // SAFETY: le slot est marqué initialisé, la run queue de ce CPU existe.
        let idle = unsafe {
            NonNull::new_unchecked(boot_idle_slot(idx).assume_init_mut() as *mut ThreadControlBlock)
        };
        let rq = unsafe { run_queue(CpuId(idx as u32)) };
        rq.set_idle_thread(idle);
        if rq.current.is_none() {
            rq.current = Some(idle);
        }
    }
}
