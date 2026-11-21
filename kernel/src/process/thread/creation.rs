// kernel/src/process/thread/creation.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// POSIX thread creation <500ns (Exo-OS Layer 1.5)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Performance target: creation <500ns measured from clone() syscall.
//
// create_thread() sequence:
//   1. Allocate TID.
//   2. Create ProcessThread (kernel stack + TCB).
//   3. Configure entry point + user stack.
//   4. Configure TLS.
//   5. Register in PCB (inc_threads).
//   6. Enqueue into run queue.
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(target_arch = "x86_64")]
use crate::arch::x86_64::gdt::{GDT_USER_CS64, GDT_USER_DS};
use crate::process::core::pcb::{process_flags, ProcessControlBlock};
use crate::process::core::pid::{Tid, TID_ALLOCATOR};
use crate::process::core::tcb::{ProcessThread, ThreadAddress};
use crate::process::lifecycle::fork::{init_fork_child_kstack_frame, ForkChildRegs};
use crate::scheduler::core::preempt::{PreemptGuard, MAX_CPUS};
use crate::scheduler::core::runqueue::run_queue;
use crate::scheduler::core::task::{CpuId, Priority, SchedPolicy};
use alloc::boxed::Box;
use core::ptr::NonNull;
use core::sync::atomic::Ordering;

#[cfg(target_arch = "x86_64")]
extern "C" {
    fn user_entry_trampoline();
}

/// Thread creation error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadCreateError {
    TidExhausted,
    OutOfMemory,
    TooManyThreads,
    InvalidCpu,
    ProcessExiting,
    StackSetupFailed,
}

/// Attributes of a POSIX thread to create.
#[derive(Clone)]
pub struct ThreadAttr {
    /// Requested user stack size (0 = default).
    pub stack_size: u64,
    /// User-provided stack base address (0 = allocate).
    pub stack_addr: u64,
    /// Scheduling policy.
    pub policy: SchedPolicy,
    /// Priority.
    pub priority: Priority,
    /// Detached thread by default.
    pub detached: bool,
    /// CPU affinity (-1 = no preference).
    pub cpu_affinity: i32,
    /// sigaltstack area size (0 = SIGSTKSZ default 8192).
    pub sigaltstack_size: u64,
}

impl Default for ThreadAttr {
    fn default() -> Self {
        Self {
            stack_size: 8 * 1024 * 1024, // 8 MiB default
            stack_addr: 0,
            policy: SchedPolicy::Normal,
            priority: Priority::NORMAL_DEFAULT,
            detached: false,
            cpu_affinity: -1,
            sigaltstack_size: 0,
        }
    }
}

/// Thread creation parameters.
pub struct ThreadCreateParams {
    /// PCB of process to which thread is added.
    pub pcb: *const ProcessControlBlock,
    /// Thread attributes.
    pub attr: ThreadAttr,
    /// Thread entry point (userspace address).
    pub start_func: u64,
    /// Argument passed to thread (userspace value, rdi).
    pub arg: u64,
    /// Explicit initial userspace RSP (0 = derive from `attr`).
    pub initial_rsp: u64,
    /// Initial userspace FS/TLS base.
    pub tls_base: u64,
    /// Target CPU for enqueue.
    pub target_cpu: u32,
    /// Address of pthread_t / parent_tid structure to populate (`CLONE_PARENT_SETTID`).
    pub pthread_out: u64,
    /// Address of child_tid structure to populate (`CLONE_CHILD_SETTID`).
    pub child_tid_ptr: u64,
    /// Address cleared and woken via futex upon thread exit (`CLONE_CHILD_CLEARTID`).
    pub clear_child_tid: u64,
}

/// Handle to a created thread.
pub struct ThreadHandle {
    pub tid: Tid,
    pub thread: *mut ProcessThread,
}

// SAFETY: transferred from a single producer thread.
unsafe impl Send for ThreadHandle {}

/// Parameters for Linux-ABI `clone()` thread creation (`CLONE_THREAD`),
/// resuming in child at `child_rip` on `child_rsp` with `rax = 0`.
pub struct CloneThreadParams<'a> {
    pub pcb: &'a ProcessControlBlock,
    pub parent: &'a ProcessThread,
    pub child_rip: u64,
    pub child_rsp: u64,
    pub parent_rflags: u64,
    pub regs: ForkChildRegs,
    pub tls_base: u64,
    pub detached: bool,
    pub target_cpu: u32,
    pub ptid_ptr: u64,
    pub ctid_ptr: u64,
    pub clear_child_tid: u64,
}

#[inline]
fn validate_and_publish_thread(
    pcb: &ProcessControlBlock,
    thread_ptr: *mut ProcessThread,
    tid: Tid,
    target_cpu: u32,
    ptid_ptr: u64,
    ctid_ptr: u64,
) -> Result<(), ThreadCreateError> {
    if ptid_ptr != 0 {
        let _ = crate::syscall::validation::write_user_typed::<u32>(ptid_ptr, tid.0);
    }
    if ctid_ptr != 0 {
        let _ = crate::syscall::validation::write_user_typed::<u32>(ctid_ptr, tid.0);
    }

    if !pcb.register_thread_ptr(thread_ptr) {
        unsafe {
            drop(Box::from_raw(thread_ptr));
        }
        TID_ALLOCATOR.free(tid.0);
        return Err(ThreadCreateError::TooManyThreads);
    }

    pcb.inc_threads();

    {
        let _preempt = PreemptGuard::new();
        if target_cpu as usize >= MAX_CPUS {
            pcb.dec_threads();
            pcb.unregister_thread_ptr(thread_ptr);
            unsafe {
                drop(Box::from_raw(thread_ptr));
            }
            TID_ALLOCATOR.free(tid.0);
            return Err(ThreadCreateError::InvalidCpu);
        }
        unsafe {
            let tcb_ptr = NonNull::new_unchecked((*thread_ptr).tcb_ptr());
            run_queue(CpuId(target_cpu)).enqueue(tcb_ptr);
        }
    }

    Ok(())
}

/// Creates a new POSIX thread from standard Linux 5-argument `clone()` syscall frame.
pub fn create_cloned_thread(params: &CloneThreadParams<'_>) -> Result<ThreadHandle, ThreadCreateError> {
    let pcb = params.pcb;
    if pcb.is_exiting() || (pcb.flags.load(Ordering::Acquire) & process_flags::EXECING) != 0 {
        return Err(ThreadCreateError::ProcessExiting);
    }
    if params.child_rsp == 0 {
        return Err(ThreadCreateError::StackSetupFailed);
    }

    let tid_raw = TID_ALLOCATOR
        .alloc()
        .map_err(|_| ThreadCreateError::TidExhausted)?;
    let tid = Tid(tid_raw);
    let cr3 = pcb.cr3.load(Ordering::Relaxed);
    let mut thread = ProcessThread::new(
        tid,
        pcb.pid,
        cr3,
        params.parent.sched_tcb.policy,
        params.parent.sched_tcb.priority,
    )
    .ok_or_else(|| {
        TID_ALLOCATOR.free(tid_raw);
        ThreadCreateError::OutOfMemory
    })?;

    unsafe {
        init_fork_child_kstack_frame(
            &mut thread,
            &params.parent.sched_tcb,
            params.child_rip,
            params.child_rsp,
            params.parent_rflags,
            &params.regs,
        );
        thread
            .tcb_mut()
            .set_kpti_user_cr3(params.parent.sched_tcb.kpti_user_cr3());
        thread.addresses = ThreadAddress {
            stack_base: params.child_rsp.saturating_sub(8 * 1024 * 1024),
            stack_size: 8 * 1024 * 1024,
            entry_point: params.child_rip,
            initial_rsp: params.child_rsp,
            tls_base: params.tls_base,
            entry_arg0: 0,
            pthread_ptr: params.ptid_ptr,
            sigaltstack_base: 0,
            sigaltstack_size: 0,
        };
        thread.sched_tcb.fs_base = params.tls_base;
        thread.sched_tcb.user_gs_base = params.parent.sched_tcb.user_gs_base;
        thread.tls_gs_base.store(params.tls_base, Ordering::Release);
        if params.detached {
            thread.detached.store(true, Ordering::Relaxed);
        }
        if params.clear_child_tid != 0 {
            thread
                .clear_child_tid
                .store(params.clear_child_tid, Ordering::Release);
        }
    }

    let thread_ptr = Box::into_raw(thread);
    validate_and_publish_thread(
        pcb,
        thread_ptr,
        tid,
        params.target_cpu,
        params.ptid_ptr,
        params.ctid_ptr,
    )?;

    Ok(ThreadHandle {
        tid,
        thread: thread_ptr,
    })
}

/// Creates a new POSIX thread in the given process.
///
/// # Safety
/// `params.pcb` must point to a valid and non-freed PCB.
pub fn create_thread(params: &ThreadCreateParams) -> Result<ThreadHandle, ThreadCreateError> {
    // SAFETY: pcb is guaranteed valid by the caller.
    let pcb = unsafe { &*params.pcb };

    // Reject if process is exiting or execing.
    if pcb.is_exiting() || (pcb.flags.load(Ordering::Acquire) & process_flags::EXECING) != 0 {
        return Err(ThreadCreateError::ProcessExiting);
    }

    // 1. Allocate TID.
    let tid_raw = TID_ALLOCATOR
        .alloc()
        .map_err(|_| ThreadCreateError::TidExhausted)?;
    let tid = Tid(tid_raw);

    // 2. Create ProcessThread.
    let cr3 = pcb.cr3.load(Ordering::Relaxed);
    let leader_kpti = {
        let alive = pcb.find_alive_thread();
        if alive.is_null() {
            0
        } else {
            unsafe { (*alive).sched_tcb.kpti_user_cr3() }
        }
    };
    let thread = ProcessThread::new(tid, pcb.pid, cr3, params.attr.policy, params.attr.priority)
        .ok_or_else(|| {
            TID_ALLOCATOR.free(tid_raw);
            ThreadCreateError::OutOfMemory
        })?;

    // 3. Configure entry point and user stack.
    let thread_ptr = Box::into_raw(thread);
    // SAFETY: thread_ptr is valid, newly created.
    unsafe {
        // Configure startup frame on kernel stack for the trampoline.
        let kstack_top = (*thread_ptr).kernel_stack.top_addr();
        let initial_rsp = if params.initial_rsp != 0 {
            params.initial_rsp
        } else {
            params
                .attr
                .stack_addr
                .wrapping_add(params.attr.stack_size)
                .wrapping_sub(16)
        };

        // Expected layout for switch_to_new_thread then user_entry_trampoline:
        // 6 callee-saved registers, a return address, then the iretq frame.
        let kernel_rsp = kstack_top - 96;
        let frame = kernel_rsp as *mut u64;
        *frame.add(0) = 0; // rbx
        *frame.add(1) = 0; // rbp
        *frame.add(2) = params.arg; // r12 -> rdi via user_entry_trampoline
        *frame.add(3) = 0; // r13
        *frame.add(4) = 0; // r14
        *frame.add(5) = 0; // r15
        *frame.add(6) = user_entry_trampoline as *const () as u64;
        *frame.add(7) = params.start_func; // RIP userspace
        *frame.add(8) = GDT_USER_CS64 as u64; // CS ring 3, 64-bit
        *frame.add(9) = 0x0202; // RFLAGS (IF=1)
        *frame.add(10) = initial_rsp; // RSP userspace
        *frame.add(11) = GDT_USER_DS as u64; // SS
        (*thread_ptr).sched_tcb.kstack_ptr = kernel_rsp;
        (*thread_ptr).tcb_mut().set_kpti_user_cr3(leader_kpti);

        // Userspace addresses.
        (*thread_ptr).addresses = ThreadAddress {
            stack_base: params.attr.stack_addr,
            stack_size: params.attr.stack_size,
            entry_point: params.start_func,
            initial_rsp,
            tls_base: params.tls_base,
            entry_arg0: params.arg,
            pthread_ptr: params.pthread_out,
            sigaltstack_base: 0,
            sigaltstack_size: params.attr.sigaltstack_size,
        };
        (*thread_ptr).sched_tcb.fs_base = params.tls_base;
        (*thread_ptr).sched_tcb.user_gs_base = 0;
        (*thread_ptr)
            .tls_gs_base
            .store(params.tls_base, Ordering::Release);

        // CPU affinity.
        if params.attr.cpu_affinity >= 0 {
            let cpu = params.attr.cpu_affinity as u32;
            if cpu < MAX_CPUS as u32 {
                (*thread_ptr).sched_tcb.set_cpu_affinity_single(CpuId(cpu));
            }
        }

        // Detached thread.
        if params.attr.detached {
            (*thread_ptr).detached.store(true, Ordering::Relaxed);
        }

        // Configure CLONE_CHILD_CLEARTID before thread can be scheduled.
        if params.clear_child_tid != 0 {
            (*thread_ptr)
                .clear_child_tid
                .store(params.clear_child_tid, Ordering::Release);
        }
    }

    validate_and_publish_thread(
        pcb,
        thread_ptr,
        tid,
        params.target_cpu,
        params.pthread_out,
        params.child_tid_ptr,
    )?;

    Ok(ThreadHandle {
        tid,
        thread: thread_ptr,
    })
}
