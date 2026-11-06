// kernel/src/process/lifecycle/create.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Process and kernel thread creation (Exo-OS Layer 1.5)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Entry points:
//   create_process() — creates a complete process (PCB + main thread)
//   create_kthread() — creates a pure kernel thread (pid=1, KTHREAD flag)
//
// Sequence for create_process():
//   1. Allocate a PID + TID.
//   2. Create ProcessThread (kernel stack + scheduler TCB).
//   3. Create ProcessControlBlock.
//   4. Register in PROCESS_REGISTRY.
//   5. Enqueue TCB in the scheduler run queue.
//   6. Return PID.
// ═══════════════════════════════════════════════════════════════════════════════

use crate::process::core::pid::{Pid, PidAllocError, Tid, PID_ALLOCATOR, TID_ALLOCATOR};
use alloc::boxed::Box;
use core::ptr::NonNull;
use core::sync::atomic::Ordering;

// kthread start trampoline — defined in scheduler/asm/switch_asm.s.
// Called during first context_switch to a new kthread.
// On entry: r12 = entry_fn, r13 = arg. Places arg in rdi then jmp *r12.
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
use crate::scheduler::smp::topology::nr_cpus;

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
// Creation errors
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateError {
    /// No more PIDs available.
    PidExhausted,
    /// No more TIDs available.
    TidExhausted,
    /// Memory allocation failed (kernel stack or PCB).
    OutOfMemory,
    /// Registration error in registry.
    RegistryError,
    /// Invalid target CPU.
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
// CreateParams — process creation parameters
// ─────────────────────────────────────────────────────────────────────────────

/// User process creation parameters.
pub struct CreateParams {
    /// Parent process PID (0 = init).
    pub ppid: Pid,
    /// Credentials of the new process.
    pub creds: Credentials,
    /// Initial CR3 (empty address space — filled by execve).
    pub cr3: u64,
    /// Opaque pointer to initial address space (can be 0).
    pub addr_space: usize,
    /// Main thread scheduling policy.
    pub policy: SchedPolicy,
    /// Initial priority.
    pub priority: Priority,
    /// Target CPU for initial enqueue.
    pub target_cpu: u32,
    /// Open FDs limit.
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

fn least_loaded_cpu() -> u32 {
    let n = nr_cpus().clamp(1, MAX_CPUS);
    let mut best_cpu = 0u32;
    let mut best_load = usize::MAX;
    let mut cpu = 0usize;
    while cpu < n {
        // SAFETY: cpu < nr_cpus() <= MAX_CPUS and run queues are initialized
        // before process creation is allowed to enqueue user threads.
        let load = unsafe { run_queue(CpuId(cpu as u32)) }.nr_running_usize();
        if load < best_load {
            best_load = load;
            best_cpu = cpu as u32;
        }
        cpu += 1;
    }
    best_cpu
}

/// Creation handle — groups created objects to pass them atomically.
pub struct ProcessHandle {
    /// Created process PID.
    pub pid: Pid,
    /// Main thread TID.
    pub tid: Tid,
    /// Raw pointer to ProcessThread (managed by lifecycle).
    pub thread: *mut ProcessThread,
}

// SAFETY: ProcessHandle transferred between threads only during creation.
unsafe impl Send for ProcessHandle {}

// ─────────────────────────────────────────────────────────────────────────────
// create_process — full creation of a user process
// ─────────────────────────────────────────────────────────────────────────────

/// Creates a new process with a main thread.
///
/// Sequence:
///   1. Allocate PID + TID.
///   2. Create ProcessThread (kernel stack + scheduler TCB).
///   3. Create ProcessControlBlock.
///   4. Insert into PROCESS_REGISTRY.
///   5. Push TCB into the run queue.
///
/// # Safety
/// Called from context of a kernel thread with active preemption.
pub fn create_process(params: &CreateParams) -> Result<ProcessHandle, CreateError> {
    // 1. Allocate PID and TID.
    let pid_raw = PID_ALLOCATOR.alloc()?;
    let tid_raw = TID_ALLOCATOR.alloc().map_err(|_| {
        // Free already allocated PID before returning error.
        PID_ALLOCATOR.free(pid_raw);
        CreateError::TidExhausted
    })?;

    let pid = Pid(pid_raw);
    let tid = Tid(tid_raw);

    // 2. Create ProcessThread (kernel stack allocated here).
    let thread = ProcessThread::new(tid, pid, params.cr3, params.policy, params.priority)
        .ok_or_else(|| {
            PID_ALLOCATOR.free(pid_raw);
            TID_ALLOCATOR.free(tid_raw);
            CreateError::OutOfMemory
        })?;

    // Record POSIX TID in TCB.
    let thread_ptr = Box::into_raw(thread);

    // 3. Create PCB.
    let pcb = ProcessControlBlock::try_new(
        pid,
        params.ppid,
        pid, // tgid = pid for main thread
        ThreadId(tid_raw as u64),
        params.creds,
        params.fd_limit,
        params.cr3,
        params.addr_space,
    )
    .ok_or_else(|| {
        // SAFETY: thread_ptr was created by Box::into_raw just above.
        unsafe {
            drop(Box::from_raw(thread_ptr));
        }
        PID_ALLOCATOR.free(pid_raw);
        TID_ALLOCATOR.free(tid_raw);
        CreateError::OutOfMemory
    })?;
    pcb.set_main_thread_ptr(thread_ptr);

    // 4. Insert into registry.
    // NOTE: If insert fails, drop of `pcb` calls `cleanup_all_threads()` which frees
    // `thread_ptr` and `tid_raw`. Therefore, only `pid_raw` must be freed here.
    PROCESS_REGISTRY.insert(pcb).map_err(|_| {
        PID_ALLOCATOR.free(pid_raw);
        CreateError::RegistryError
    })?;
    sync_process_kernel_half(params.cr3);

    // 5. Register TCB into target CPU run queue.
    {
        let _preempt = PreemptGuard::new();
        let cpu_id = if params.target_cpu == 0 {
            least_loaded_cpu()
        } else {
            params.target_cpu
        };
        if cpu_id as usize >= MAX_CPUS {
            // Invalid CPU — clean up and return error.
            // Removing PCB from registry and dropping it frees thread_ptr and tid_raw.
            let _ = PROCESS_REGISTRY.remove(pid);
            PID_ALLOCATOR.free(pid_raw);
            return Err(CreateError::InvalidCpu);
        }
        // SAFETY: cpu_id verified, thread_ptr valid, lifetime managed by lifecycle.
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

/// Creates real PID 1 from an already loaded ELF image.
///
/// `pid::init()` reserves PID 1 in bitmap: this path therefore does not allocate a
/// PID, it explicitly publishes the `init` PCB in registry then enqueues its
/// main thread. The first switch to this thread goes through
/// `user_entry_trampoline`, which prepares minimal ABI registers before ring3
/// IRET.
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

    const PAGE_SIZE_U64: u64 = crate::memory::core::PAGE_SIZE as u64;
    const USER_STACK_SIZE: u64 = crate::memory::core::layout::USER_STACK_BOOTSTRAP_SIZE as u64;
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
            entry_arg0: elf.entry_arg0,
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
        *frame.add(2) = elf.entry_arg0; // r12 -> rdi via user_entry_trampoline
        *frame.add(3) = 0; // r13
        *frame.add(4) = 0; // r14
        *frame.add(5) = 0; // r15
        *frame.add(6) = user_entry_trampoline as *const () as u64;
        *frame.add(7) = elf.entry_point; // Userspace RIP
        *frame.add(8) = GDT_USER_CS64 as u64; // CS ring3 64-bit
        *frame.add(9) = 0x0202; // RFLAGS: reserved bit + IF
        *frame.add(10) = elf.initial_stack_top; // Userspace RSP
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
    pcb.set_name_bytes(b"init_server");
    pcb.set_main_thread_ptr(thread_ptr);
    pcb.signal_tcb_vaddr
        .store(elf.signal_tcb_vaddr, Ordering::Release);
    pcb.brk_start.store(elf.brk_start, Ordering::Release);
    pcb.brk_current.store(elf.brk_start, Ordering::Release);
    pcb.flags.fetch_or(
        process_flags::EXEC_DONE | process_flags::VFORK_DONE,
        Ordering::Release,
    );
    {
        const TTY_HANDLE: u64 = crate::syscall::fs_bridge::TTY_PTS0_HANDLE as u64;
        pcb.files
            .lock()
            .install_std_fds(TTY_HANDLE, TTY_HANDLE, TTY_HANDLE);
    }
    pcb.set_state(ProcessState::Running);

    // FIX-SEC-T0.2/E: init (PID 1) = FS trust root. Grant an admin
    // capability on the root ExoFS object (FS_ROOT_OBJECT_ID) -> authorizes
    // global operations (gc/quota/snapshot/import) and admin. Servers
    // obtain their caps by inheritance at fork (inherit_from) or delegation.
    {
        use crate::fs::exofs::core::rights::{ALL_RIGHTS, RIGHT_ADMIN};
        use crate::security::capability::{CapObjectType, ObjectId, Rights};
        let _ = pcb.cap_table.grant(
            ObjectId::from_raw(crate::fs::exofs::syscall::captable::FS_ROOT_OBJECT_ID),
            Rights::from_bits_truncate(ALL_RIGHTS | RIGHT_ADMIN),
            CapObjectType::FileInode,
        );
    }

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
            run_queue(CpuId(least_loaded_cpu())).enqueue(tcb_ptr);
        }
    }

    Ok(ProcessHandle {
        pid,
        tid,
        thread: thread_ptr,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// create_kthread — creation of a dedicated kernel thread
// ─────────────────────────────────────────────────────────────────────────────

/// Parameters for a kthread.
pub struct KthreadParams {
    /// Name of the kthread (for debugging).
    pub name: &'static str,
    /// Entry function of the kthread.
    pub entry: fn(usize) -> !,
    /// Argument passed to `entry`.
    pub arg: usize,
    /// Target CPU (0 = BSP).
    pub target_cpu: u32,
    /// Priority (Normal by default).
    pub priority: Priority,
}

/// Creates a kernel thread (kthread) and enqueues it into the run queue.
///
/// kthreads:
///   - Belong to the kernel domain (PID 0 on scheduler side).
///   - Never have a user address space.
///   - Have the KTHREAD flag set in TCB.
///
/// # Safety
/// The `arg` argument and `entry` pointer must remain valid
/// throughout the lifetime of the kthread.
pub fn create_kthread(params: &KthreadParams) -> Result<Tid, CreateError> {
    // Allocate a TID only (kthread does not consume an extra PID).
    let tid_raw = TID_ALLOCATOR
        .alloc()
        .map_err(|_| CreateError::TidExhausted)?;
    let tid = Tid(tid_raw);

    // Create ProcessThread with cr3=0 (shared kernel space).
    let thread = ProcessThread::new_kthread(tid, 0).ok_or_else(|| {
        TID_ALLOCATOR.free(tid_raw);
        CreateError::OutOfMemory
    })?;

    let thread_ptr = Box::into_raw(thread);

    // Configure entry point in kernel stack.
    // SAFETY: thread_ptr valid, kernel_stack allocated inside.
    unsafe {
        (*thread_ptr).sched_tcb.priority = params.priority;

        // Frame expected by switch_to_new_thread during FIRST switch to this kthread.
        // switch_to_new_thread restores in this order (WITHOUT MXCSR/FCW):
        //   popq %rbx           → rbx     [rsp+ 0]
        //   popq %rbp           → rbp     [rsp+ 8]
        //   popq %r12           → r12     [rsp+16]  ← entry_fn
        //   popq %r13           → r13     [rsp+24]  ← arg
        //   popq %r14           → r14     [rsp+32]
        //   popq %r15           → r15     [rsp+40]
        //   ret                 → rip     [rsp+48]  ← kthread_trampoline
        //
        // kthread_trampoline does: mov r13, rdi ; jmp *r12
        // (arg → rdi, then jumps to entry_fn(arg))
        //
        // NOTE: context_switch_asm uses a 72-byte frame WITH MXCSR+FCW.
        //       switch_to_new_thread uses a 56-byte frame WITHOUT MXCSR+FCW.
        //       create_kthread() must use the switch_to_new_thread format (first activation).
        let stack_top = (*thread_ptr).kernel_stack.top_addr();
        const FRAME: u64 = 7 * 8; // 56 bytes — switch_to_new_thread format
        let kernel_rsp = stack_top - FRAME;
        let frame = kernel_rsp as *mut u64;
        *frame.add(0) = 0; // rbx
        *frame.add(1) = 0; // rbp
        *frame.add(2) = params.entry as usize as u64; // r12 → entry_fn
        *frame.add(3) = params.arg as u64; // r13 → arg
        *frame.add(4) = 0; // r14
        *frame.add(5) = 0; // r15
        *frame.add(6) = kthread_trampoline as *const () as u64; // return address → trampoline
        (*thread_ptr).sched_tcb.kstack_ptr = kernel_rsp;
    }
    // Register in run queue.
    {
        let _preempt = PreemptGuard::new();
        if params.target_cpu as usize >= MAX_CPUS {
            // SAFETY: thread_ptr created via Box::into_raw() above, not yet enqueued.
            unsafe {
                drop(Box::from_raw(thread_ptr));
            }
            TID_ALLOCATOR.free(tid_raw);
            return Err(CreateError::InvalidCpu);
        }
        // SAFETY: valid cpu, valid thread_ptr.
        unsafe {
            let tcb_ptr = NonNull::new_unchecked((*thread_ptr).tcb_ptr());
            run_queue(CpuId(params.target_cpu)).enqueue(tcb_ptr);
        }
    }

    Ok(tid)
}
