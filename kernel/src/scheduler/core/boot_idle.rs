// kernel/src/scheduler/core/boot_idle.rs
//
// Per-CPU bootstrap idle TCB.
//
// This module provides a static idle TCB for each logical CPU.
// It serves as the canonical "current thread" during BSP/AP phases where the CPU
// already executes its native idle loop (`hlt`) before a real user
// thread or kthread is scheduled on this CPU.

use super::preempt::MAX_CPUS;
use super::runqueue::run_queue;
use super::switch::CURRENT_THREAD_PER_CPU;
use super::task::{
    CpuId, Priority, ProcessId, SchedPolicy, TaskState, ThreadControlBlock, ThreadId,
};
use core::mem::MaybeUninit;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, Ordering};

const BOOT_IDLE_TID_BASE: u64 = 0x8000_0000;

static mut BOOT_IDLE_TCBS: [MaybeUninit<ThreadControlBlock>; MAX_CPUS] = {
    // SAFETY: array of MaybeUninit explicitly uninitialized.
    unsafe { MaybeUninit::uninit().assume_init() }
};
static BOOT_IDLE_CLAIMED: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];
static BOOT_IDLE_INIT: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];

unsafe fn boot_idle_slot(cpu_id: usize) -> &'static mut MaybeUninit<ThreadControlBlock> {
    // SAFETY: caller bounds cpu_id to MAX_CPUS and access is per unique CPU.
    unsafe { &mut BOOT_IDLE_TCBS[cpu_id] }
}

/// Guarantees that a static idle TCB exists for `cpu_id`.
///
/// # Safety
/// Must be called with a valid `cpu_id`. The caller guarantees that a given CPU
/// does not initialize its slot concurrently with another context.
pub unsafe fn ensure_boot_idle_tcb(
    cpu_id: u32,
    kernel_stack_top: u64,
) -> Option<NonNull<ThreadControlBlock>> {
    let idx = cpu_id as usize;
    if idx >= MAX_CPUS {
        return None;
    }

    if !BOOT_IDLE_INIT[idx].load(Ordering::Acquire) {
        if BOOT_IDLE_CLAIMED[idx]
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            while !BOOT_IDLE_INIT[idx].load(Ordering::Acquire) {
                core::hint::spin_loop();
            }
        } else {
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

            // SAFETY: unique slot per CPU, initialized once in practice.
            unsafe {
                boot_idle_slot(idx).write(idle);
            }
            BOOT_IDLE_INIT[idx].store(true, Ordering::Release);
        }
    }

    // SAFETY: slot is initialized right above or was already initialized.
    let ptr = unsafe { boot_idle_slot(idx).assume_init_mut() as *mut ThreadControlBlock };
    Some(unsafe { NonNull::new_unchecked(ptr) })
}

/// Publishes bootstrap idle TCB as current thread of the calling CPU.
///
/// # Safety
/// Per-CPU GS must already be initialized for this CPU.
pub unsafe fn publish_current_boot_idle(
    cpu_id: u32,
    kernel_stack_top: u64,
) -> Option<NonNull<ThreadControlBlock>> {
    let idle = unsafe { ensure_boot_idle_tcb(cpu_id, kernel_stack_top)? };
    // SAFETY: this TCB represents the current context of this CPU.
    unsafe {
        idle.as_ref().set_state(TaskState::Running);
        crate::arch::x86_64::smp::percpu::set_current_tcb(idle.as_ptr());
        crate::arch::x86_64::smp::percpu::set_kernel_rsp(kernel_stack_top);
    }
    CURRENT_THREAD_PER_CPU[cpu_id as usize].store(idle.as_ptr() as usize, Ordering::Release);
    Some(idle)
}

/// Returns the bootstrap idle TCB already published for `cpu_id`, if it exists.
///
/// Initializes nothing new: this helper only serves to reconnect a
/// run queue that lost its `idle_thread` while the canonical idle TCB
/// already exists for this CPU.
pub fn published_boot_idle(cpu_id: u32) -> Option<NonNull<ThreadControlBlock>> {
    let idx = cpu_id as usize;
    if idx >= MAX_CPUS || !BOOT_IDLE_INIT[idx].load(Ordering::Acquire) {
        return None;
    }

    // SAFETY: BOOT_IDLE_INIT[idx] publishes complete slot initialization.
    let ptr = unsafe { boot_idle_slot(idx).assume_init_ref() as *const ThreadControlBlock };
    Some(unsafe { NonNull::new_unchecked(ptr as *mut ThreadControlBlock) })
}

/// Binds already-published idle TCBs to per-CPU run queues after `scheduler::init()`.
///
/// # Safety
/// Run queues for `nr_cpus` must already have been initialized.
pub unsafe fn bind_boot_idle_threads(nr_cpus: usize) {
    for idx in 0..nr_cpus.min(MAX_CPUS) {
        let Some(idle) = published_boot_idle(idx as u32) else {
            continue;
        };
        let rq = unsafe { run_queue(CpuId(idx as u32)) };
        rq.set_idle_thread(idle);
        if rq.current.is_none() {
            rq.current = Some(idle);
        }
    }
}
