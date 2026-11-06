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
use crate::process::core::pcb::ProcessControlBlock;
use crate::process::core::pid::{Tid, TID_ALLOCATOR};
use crate::process::core::tcb::{ProcessThread, ThreadAddress};
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
            sigaltstack_size: 8192,
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
    /// Address of pthread_t structure to populate.
    pub pthread_out: u64,
}

/// Handle to a created thread.
pub struct ThreadHandle {
    pub tid: Tid,
    pub thread: *mut ProcessThread,
}

// SAFETY: transferred from a single producer thread.
unsafe impl Send for ThreadHandle {}

/// Creates a new POSIX thread in the given process.
///
/// # Safety
/// `params.pcb` must point to a valid and non-freed PCB.
pub fn create_thread(params: &ThreadCreateParams) -> Result<ThreadHandle, ThreadCreateError> {
    // SAFETY: pcb is guaranteed valid by the caller.
    let pcb = unsafe { &*params.pcb };

    // Reject if process is exiting.
    if pcb.is_exiting() {
        return Err(ThreadCreateError::ProcessExiting);
    }

    // 1. Allocate TID.
    let tid_raw = TID_ALLOCATOR
        .alloc()
        .map_err(|_| ThreadCreateError::TidExhausted)?;
    let tid = Tid(tid_raw);

    // 2. Create ProcessThread.
    let cr3 = pcb.cr3.load(Ordering::Relaxed);
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
    }

    if !pcb.register_thread_ptr(thread_ptr) {
        // SAFETY: thread_ptr was just allocated and is not published.
        unsafe {
            drop(Box::from_raw(thread_ptr));
        }
        TID_ALLOCATOR.free(tid_raw);
        return Err(ThreadCreateError::TooManyThreads);
    }

    // 4. Increment PCB thread counter.
    pcb.inc_threads();

    // 5. Enqueue into run queue.
    {
        let _preempt = PreemptGuard::new();
        if params.target_cpu as usize >= MAX_CPUS {
            pcb.dec_threads();
            pcb.unregister_thread_ptr(thread_ptr);
            // SAFETY: thread_ptr is valid.
            unsafe {
                drop(Box::from_raw(thread_ptr));
            }
            TID_ALLOCATOR.free(tid_raw);
            return Err(ThreadCreateError::InvalidCpu);
        }
        // SAFETY: cpu verified, thread_ptr is valid.
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
