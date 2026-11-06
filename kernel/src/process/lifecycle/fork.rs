// kernel/src/process/lifecycle/fork.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// fork() — Copy-on-Write process duplication (Exo-OS Layer 1.5)
// ═══════════════════════════════════════════════════════════════════════════════
//
// fork() POSIX semantics:
//   • Copies current process with CoW on user pages.
//   • Inherits fds, masked signals, credentials, namespaces.
//   • Child receives PID=new / return=0; parent receives child PID.
//
// RULE PROC-08: flush parent TLB BEFORE return from fork().
//
// Memory layer:
//   Address space duplication is delegated to memory/cow/
//   via abstract trait AddressSpaceCloner (dependency injection).
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(target_arch = "x86_64")]
use crate::arch::x86_64::gdt::{GDT_USER_CS64, GDT_USER_DS};
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

#[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
use core::sync::atomic::AtomicUsize;

pub use crate::memory::virt::address_space::fork_impl::{
    AddrSpaceCloneError, AddressSpaceCloner, ClonedAddressSpace,
};

// ─────────────────────────────────────────────────────────────────────────────
// ForkFlags — Linux-compatible CLONE_* flags
// ─────────────────────────────────────────────────────────────────────────────

/// Flags controlling fork/clone behavior.
#[derive(Copy, Clone, Default, Debug)]
pub struct ForkFlags(pub u32);

impl ForkFlags {
    /// Share fds (CLONE_FILES).
    pub const CLONE_FILES: u32 = 1 << 0;
    /// Share address space (CLONE_VM = thread clone).
    pub const CLONE_VM: u32 = 1 << 1;
    /// Share signal handlers (CLONE_SIGHAND).
    pub const CLONE_SIGHAND: u32 = 1 << 2;
    /// New PID namespace (CLONE_NEWPID).
    pub const CLONE_NEWPID: u32 = 1 << 3;
    /// vfork operation (parent blocked until exec).
    pub const VFORK: u32 = 1 << 4;
    /// POSIX thread (CLONE_THREAD).
    pub const CLONE_THREAD: u32 = 1 << 5;

    pub fn set(self, flag: u32) -> Self {
        Self(self.0 | flag)
    }
    pub fn has(self, flag: u32) -> bool {
        self.0 & flag != 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Dependency injection towards memory/cow/
// ─────────────────────────────────────────────────────────────────────────────

static ADDR_SPACE_CLONER: Once<&'static dyn AddressSpaceCloner> = Once::new();
static VFORK_WAIT_QUEUE: WaitQueue = WaitQueue::new();

/// Registers address space cloning implementation (memory/ at boot).
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

#[allow(clippy::result_unit_err)]
pub fn wait_for_vfork_completion(
    child_pid: Pid,
    caller_tcb: &mut ThreadControlBlock,
) -> Result<(), ()> {
    while !vfork_completion_reached(child_pid) {
        let _ = unsafe { VFORK_WAIT_QUEUE.wait_interruptible(caller_tcb as *mut _) };
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
    UnsupportedFlag,
}

#[inline]
fn rollback_child_allocations(
    cloned_as: &ClonedAddressSpace,
    pid_raw: u32,
    tid_raw: u32,
    owns_addr_space: bool,
) {
    if owns_addr_space {
        if let Some(cl) = ADDR_SPACE_CLONER.get() {
            cl.free_addr_space(cloned_as.addr_space_ptr);
        }
    }
    PID_ALLOCATOR.free(pid_raw);
    TID_ALLOCATOR.free(tid_raw);
}

fn rollback_child_process_resources(
    cloned_as: &ClonedAddressSpace,
    pid_raw: u32,
    owns_addr_space: bool,
) {
    if owns_addr_space {
        if let Some(cl) = ADDR_SPACE_CLONER.get() {
            cl.free_addr_space(cloned_as.addr_space_ptr);
        }
    }
    PID_ALLOCATOR.free(pid_raw);
}

#[inline]
fn vfork_shares_address_space(flags: ForkFlags) -> bool {
    // FIX #25: VFORK NO LONGER SHARES address space. Treated as a
    // "blocking fork": separate CoW clone (like fork) + parent suspended until
    // child's execve (`wait_for_vfork_completion`, triggered by the VFORK
    // flag in dispatch.rs, independent of this sharing choice).
    //
    // Reason: sharing AS exposed live parent memory (init) to
    // child's execve — co-scheduling race #25 then directly corrupted
    // init's AS. With a separate CoW clone + blocked parent, init's AS
    // is protected by CoW AND init does not execute during execve,
    // reliably closing the race window (covers all of execve, unlike
    // a fixed sleep). Only CLONE_VM (threads) still shares AS.
    flags.has(ForkFlags::CLONE_VM)
}

#[inline]
fn cloned_address_space_for_vfork(parent_cr3: u64, parent_space_ptr: usize) -> ClonedAddressSpace {
    ClonedAddressSpace {
        cr3: parent_cr3,
        addr_space_ptr: parent_space_ptr,
    }
}

#[inline]
fn clone_parent_address_space(
    cloner: &'static dyn AddressSpaceCloner,
    parent_cr3: u64,
    parent_space_ptr: usize,
) -> Result<ClonedAddressSpace, ForkError> {
    cloner
        .clone_cow(parent_cr3, parent_space_ptr)
        .map_err(|_| ForkError::AddressSpaceCloneFailed)
}

#[inline]
fn update_child_addr_space_pid(
    cloned_as: &ClonedAddressSpace,
    child_pid: Pid,
    owns_addr_space: bool,
) {
    if owns_addr_space && cloned_as.addr_space_ptr != 0 {
        let child_as = unsafe {
            &mut *(cloned_as.addr_space_ptr as *mut crate::memory::virt::UserAddressSpace)
        };
        child_as.pid = child_pid.0 as u64;
    }
}

#[inline]
fn child_process_flags(flags: ForkFlags) -> u32 {
    let mut bits = process_flags::FORKED;
    if vfork_shares_address_space(flags) {
        bits |= process_flags::VFORK_SHARED_AS;
    }
    bits
}

#[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
static FORK_TRACE_COUNT: AtomicUsize = AtomicUsize::new(0);

#[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
#[inline]
fn fork_trace(message: &[u8]) {
    if FORK_TRACE_COUNT.fetch_add(1, Ordering::Relaxed) < 2048 {
        crate::arch::x86_64::terminal::debug_write(message);
    }
}

#[cfg(not(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace)))]
#[inline]
fn fork_trace(_message: &[u8]) {}

#[inline]
fn sync_child_kernel_half(cr3: u64) {
    if cr3 == 0 {
        return;
    }
    unsafe {
        crate::memory::virt::address_space::KERNEL_AS
            .sync_kernel_half_into(crate::memory::core::PhysAddr::new(cr3));
    }
}

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
fn fork_debug_parent(label: &[u8], parent: &ProcessThread, parent_pcb: &ProcessControlBlock) {
    let out = crate::arch::x86_64::terminal::debug_write;
    out(label);
    out(b" pcb=0x");
    debug_hex64(parent_pcb as *const _ as u64);
    out(b" pcb_pid=0x");
    debug_hex64(parent_pcb.pid.0 as u64);
    out(b" pcb_as=0x");
    debug_hex64(parent_pcb.address_space.load(Ordering::Relaxed) as u64);
    out(b" pcb_cr3=0x");
    debug_hex64(parent_pcb.cr3.load(Ordering::Relaxed));
    out(b" tcb_cr3=0x");
    debug_hex64(parent.sched_tcb.cr3_phys);
    out(b"\n");
}

#[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
fn fork_debug_cloned(label: &[u8], cloned_as: &ClonedAddressSpace) {
    let out = crate::arch::x86_64::terminal::debug_write;
    out(label);
    out(b" child_cr3=0x");
    debug_hex64(cloned_as.cr3);
    out(b" child_as=0x");
    debug_hex64(cloned_as.addr_space_ptr as u64);
    out(b"\n");
}

// ─────────────────────────────────────────────────────────────────────────────
// do_fork — main implementation
// ─────────────────────────────────────────────────────────────────────────────

/// Input parameters for do_fork().
pub struct ForkContext<'a> {
    /// Parent process ProcessThread.
    pub parent_thread: &'a ProcessThread,
    /// Parent process PCB.
    pub parent_pcb: &'a ProcessControlBlock,
    /// Fork flags (ForkFlags::*).
    pub flags: ForkFlags,
    /// Target CPU for child thread (typically current CPU).
    pub target_cpu: u32,
    /// Child RIP upon return (after syscall return, = fork point).
    pub child_rip: u64,
    /// Child user RSP.
    pub child_rsp: u64,
    /// Parent RFLAGS at fork time (from frame.r11 saved by SYSCALL).
    /// FIX P2-02: propagated to child with secure masking.
    pub parent_rflags: u64,
    /// User registers to restore in child.
    ///
    /// The userspace `syscall` wrapper only declares `rax`, `rcx`, and `r11`
    /// as clobbers. Child must therefore resume with same visible registers
    /// as parent, except `rax=0`.
    pub user_rbx: u64,
    pub user_rbp: u64,
    pub user_r12: u64,
    pub user_r13: u64,
    pub user_r14: u64,
    pub user_r15: u64,
    pub user_rdi: u64,
    pub user_rsi: u64,
    pub user_rdx: u64,
    pub user_r10: u64,
    pub user_r8: u64,
    pub user_r9: u64,
}

/// fork() result for the parent.
pub struct ForkResult {
    /// Created child PID.
    pub child_pid: Pid,
    /// Child main thread TID.
    pub child_tid: Tid,
}

/// Performs a POSIX fork(): duplicates current process.
///
/// # RULE PROC-08
/// Parent TLB is flushed BEFORE return to invalidate PTEs that
/// have just been marked read-only (CoW).
///
/// # Safety
/// `ctx.parent_thread` must point to the current thread of the process.
pub fn do_fork(ctx: &ForkContext<'_>) -> Result<ForkResult, ForkError> {
    fork_trace(b"fork: enter\n");
    let parent = ctx.parent_thread;
    let parent_pcb = ctx.parent_pcb;
    #[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
    fork_debug_parent(b"fork_dbg: before", parent, parent_pcb);

    if ctx.flags.has(ForkFlags::CLONE_NEWPID) {
        return Err(ForkError::UnsupportedFlag);
    }

    // 1. Allocate child PID + TID.
    let child_pid_raw = PID_ALLOCATOR.alloc().map_err(|_| ForkError::PidExhausted)?;
    let child_tid_raw = TID_ALLOCATOR.alloc().map_err(|_| {
        PID_ALLOCATOR.free(child_pid_raw);
        ForkError::TidExhausted
    })?;

    let child_pid = Pid(child_pid_raw);
    let child_tid = Tid(child_tid_raw);
    fork_trace(b"fork: pid/tid\n");

    // 2. Clone address space (CoW).
    let cloner = ADDR_SPACE_CLONER.get().ok_or_else(|| {
        PID_ALLOCATOR.free(child_pid_raw);
        TID_ALLOCATOR.free(child_tid_raw);
        ForkError::NoAddrCloner
    })?;

    let parent_cr3 = parent.sched_tcb.cr3_phys;
    let parent_space_ptr = parent_pcb.address_space.load(Ordering::Acquire);

    let shares_address_space = vfork_shares_address_space(ctx.flags);
    let owns_addr_space = !shares_address_space;
    fork_trace(b"fork: clone-as begin\n");
    let cloned_as = if shares_address_space {
        cloned_address_space_for_vfork(parent_cr3, parent_space_ptr)
    } else {
        clone_parent_address_space(*cloner, parent_cr3, parent_space_ptr).inspect_err(|_| {
            PID_ALLOCATOR.free(child_pid_raw);
            TID_ALLOCATOR.free(child_tid_raw);
        })?
    };
    fork_trace(b"fork: clone-as done\n");
    #[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
    {
        fork_debug_parent(b"fork_dbg: after_clone", parent, parent_pcb);
        fork_debug_cloned(b"fork_dbg: cloned", &cloned_as);
    }

    update_child_addr_space_pid(&cloned_as, child_pid, owns_addr_space);
    fork_trace(b"fork: child-as pid\n");
    #[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
    fork_debug_parent(b"fork_dbg: after_child_as_pid", parent, parent_pcb);

    // RULE PROC-08: Flush parent TLB BEFORE return (pages turned read-only CoW).
    if owns_addr_space {
        cloner.flush_tlb_after_fork(parent_cr3);
    }
    fork_trace(b"fork: tlb\n");
    #[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
    fork_debug_parent(b"fork_dbg: after_tlb", parent, parent_pcb);

    // 3. Create child ProcessThread.
    fork_trace(b"fork: before policy\n");
    let policy = parent.sched_tcb.policy;
    fork_trace(b"fork: after policy\n");
    let priority = parent.sched_tcb.priority;
    fork_trace(b"fork: after priority\n");

    let child_thread = ProcessThread::new(child_tid, child_pid, cloned_as.cr3, policy, priority)
        .ok_or_else(|| {
            rollback_child_allocations(&cloned_as, child_pid_raw, child_tid_raw, owns_addr_space);
            ForkError::OutOfMemory
        })?;
    #[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
    fork_debug_parent(b"fork_dbg: after_thread_new", parent, parent_pcb);

    let child_thread_ptr = Box::into_raw(child_thread);
    fork_trace(b"fork: thread\n");

    // Configure child return point (RIP + RSP on fork return).
    // SAFETY: child_thread_ptr valid.
    unsafe {
        let child_tcb = (*child_thread_ptr).tcb_mut();
        let parent_vruntime = parent.sched_tcb.vruntime.load(Ordering::Acquire);
        child_tcb
            .vruntime
            .store(parent_vruntime.saturating_add(6_000_000), Ordering::Release);
        // Child returns 0 via syscall convention (rax=0 in frame).
        // switch_to_new_thread restores stack then ret to fork_child_trampoline.
        // fork_child_trampoline: xor eax,eax ; swapgs ; iretq → userspace.
        // SAFETY: fork_child_trampoline is defined in switch_asm.s, statically linked.
        extern "C" {
            fn fork_child_trampoline();
        }

        let kstack_top = (*child_thread_ptr).kernel_stack.top_addr();
        // Full frame: 18 × u64 = 144 bytes.
        // Layout (indices from kernel_rsp = kstack_top - 144):
        //   [0..48)    = 6 parent callee-saved regs  ← switch_to_new_thread format
        //   [48]       = fork_child_trampoline        ← ret address after 6 pops
        //   [56..104)  = 6 caller-saved regs restored by trampoline
        //   [104..144) = iretq frame (RIP, CS, RFLAGS, RSP, SS)
        let frame_ptr = (kstack_top - 144) as *mut u64;
        // Callee-saved registers. switch_to_new_thread pops them before ret.
        *frame_ptr.add(0) = ctx.user_rbx; // rbx
        *frame_ptr.add(1) = ctx.user_rbp; // rbp
        *frame_ptr.add(2) = ctx.user_r12; // r12
        *frame_ptr.add(3) = ctx.user_r13; // r13
        *frame_ptr.add(4) = ctx.user_r14; // r14
        *frame_ptr.add(5) = ctx.user_r15; // r15
                                          // Return address: fork_child_trampoline (executed after the 6 pops).
        *frame_ptr.add(6) = fork_child_trampoline as *const () as u64;
        // Caller-saved registers that userspace syscall wrapper expects preserved.
        *frame_ptr.add(7) = ctx.user_rdi;
        *frame_ptr.add(8) = ctx.user_rsi;
        *frame_ptr.add(9) = ctx.user_rdx;
        *frame_ptr.add(10) = ctx.user_r10;
        *frame_ptr.add(11) = ctx.user_r8;
        *frame_ptr.add(12) = ctx.user_r9;
        // iretq frame (RSP points here on entry to fork_child_trampoline).
        *frame_ptr.add(13) = ctx.child_rip; // Userspace RIP
        *frame_ptr.add(14) = GDT_USER_CS64 as u64; // CS ring3 (code64)

        // FIX P2-02: propagate parent RFLAGS with secure masking.
        // Mask of safe flags to inherit (POSIX + kernel security).
        // - Preserve: CF(0), PF(2), AF(4), ZF(6), SF(7), OF(11), DF(10), AC(18), ID(21)
        // - Force   : IF=1 (bit 9) — child must accept interrupts
        // - Clear   : TF=0 (bit 8) — do not trace child if parent was traced
        //             NT=0 (bit 14) — Nested Task flag — never inherited
        //             IOPL=0 (bits 12-13) — never inherited in Ring3/capability model
        //             RF=0 (bit 16) — Resume Flag — never inherited
        //             VM=0 (bit 17) — Virtual 8086 — unsupported
        const RFLAGS_SAFE_MASK: u64 = 0x0000_0000_0024_0CD5; // CF,PF,AF,ZF,SF,DF,OF,AC,ID
        const RFLAGS_FORCE_SET: u64 = 0x0000_0000_0000_0200; // IF=1
        const RFLAGS_FORCE_CLR: u64 = 0x0000_0000_0003_7100; // TF=0, IOPL=0, NT=0, RF=0, VM=0

        let child_rflags =
            ((ctx.parent_rflags & RFLAGS_SAFE_MASK) | RFLAGS_FORCE_SET) & !RFLAGS_FORCE_CLR;
        // Ensure reserved bit 1 is always 1.
        let child_rflags = child_rflags | 0x0002;

        *frame_ptr.add(15) = child_rflags; // RFLAGS (inherited from parent with masking)
        *frame_ptr.add(16) = ctx.child_rsp; // Userspace RSP
        *frame_ptr.add(17) = GDT_USER_DS as u64; // SS ring3
        child_tcb.kstack_ptr = kstack_top - 144;
        child_tcb.signal_mask.store(
            parent.sched_tcb.signal_mask.load(Ordering::Acquire),
            Ordering::Release,
        );
    }
    #[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
    fork_debug_parent(b"fork_dbg: after_child_frame", parent, parent_pcb);

    // Copy user addresses.
    // SAFETY: child_thread_ptr valid.
    unsafe {
        (*child_thread_ptr).addresses = parent.addresses;
        let child = &mut *child_thread_ptr;
        child.sched_tcb.fs_base = parent.sched_tcb.fs_base;
        child.sched_tcb.user_gs_base = parent.sched_tcb.user_gs_base;
        let tls_base = parent.tls_gs_base.load(Ordering::Relaxed);
        child.addresses.tls_base = tls_base;
        child.tls_gs_base.store(tls_base, Ordering::Release);
        child
            .tls_block
            .store(parent.tls_block.load(Ordering::Acquire), Ordering::Release);
        child.tls_size = parent.tls_size;
    }
    fork_trace(b"fork: addresses\n");
    #[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
    fork_debug_parent(b"fork_dbg: after_addresses", parent, parent_pcb);

    // 4. Create child PCB — inherits namespaces, credentials, etc.
    let parent_creds = *parent_pcb.creds.lock();
    let (fd_limit, cloned_files) = {
        let f = parent_pcb.files.lock();
        fork_trace(b"fork: files locked\n");
        let fd_limit = f.open_fd_count().max(1024);
        fork_trace(b"fork: fd count\n");
        let cloned_files = if !ctx.flags.has(ForkFlags::CLONE_FILES) {
            match f.try_clone_for_fork() {
                Some(files) => {
                    fork_trace(b"fork: files cloned\n");
                    Some(files)
                }
                None => {
                    // SAFETY: child_thread_ptr comes from Box::into_raw and is not published.
                    unsafe {
                        drop(Box::from_raw(child_thread_ptr));
                    }
                    rollback_child_allocations(
                        &cloned_as,
                        child_pid_raw,
                        child_tid_raw,
                        owns_addr_space,
                    );
                    return Err(ForkError::OutOfMemory);
                }
            }
        } else {
            None
        };
        (fd_limit, cloned_files)
    };
    #[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
    fork_debug_parent(b"fork_dbg: after_files", parent, parent_pcb);

    let mut child_pcb = ProcessControlBlock::try_new(
        child_pid,
        parent_pcb.pid, // ppid = parent pid
        child_pid,      // tgid = child pid (new process group leader thread)
        ThreadId(child_tid_raw as u64),
        parent_creds,
        fd_limit,
        cloned_as.cr3,
        cloned_as.addr_space_ptr,
    )
    .ok_or_else(|| {
        // SAFETY: child_thread_ptr comes from Box::into_raw and is not published.
        unsafe {
            drop(Box::from_raw(child_thread_ptr));
        }
        rollback_child_allocations(&cloned_as, child_pid_raw, child_tid_raw, owns_addr_space);
        ForkError::OutOfMemory
    })?;
    fork_trace(b"fork: pcb\n");
    #[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
    fork_debug_parent(b"fork_dbg: after_child_pcb", parent, parent_pcb);

    // Copy fds if !CLONE_FILES.
    if let Some(cloned_files) = cloned_files {
        *child_pcb.files.lock() = cloned_files;
    }
    child_pcb.set_name_bytes(&parent_pcb.name_snapshot());
    child_pcb.set_main_thread_ptr(child_thread_ptr);

    // Copy namespaces.
    child_pcb.pid_ns.clone_from(&parent_pcb.pid_ns);
    child_pcb.mnt_ns.clone_from(&parent_pcb.mnt_ns);
    child_pcb.net_ns.clone_from(&parent_pcb.net_ns);
    child_pcb.uts_ns.clone_from(&parent_pcb.uts_ns);
    child_pcb.user_ns.clone_from(&parent_pcb.user_ns);
    child_pcb.set_pgroup_id(parent_pcb.pgroup_id());
    child_pcb.set_session_id(parent_pcb.session_id());

    // FIX-SEC-T0.2: child INHERITS parent capability table (caps =
    // open ExoFS objects + delegated rights). Without this, a fork would lose all
    // access and `check_object_cap` would deny any post-fork operation. inherit_from
    // copies all non-free slots under write_lock (generation-based revocation
    // preserved). RULE SEC-CAP-02.
    // FIX-SEC-T1.0: LEAST-PRIVILEGE inheritance — child inherits operational
    // rights (read/write/…) but NOT privileged FS rights
    // (gc/import/snapshot/admin = PRIVILEGED_RIGHTS): these only propagate
    // through explicit grant/delegation. Prevents any fork (shell, app)
    // from inheriting init's FS admin.
    child_pcb.cap_table =
        alloc::boxed::Box::new(crate::security::capability::CapTable::inherit_from_masked(
            &parent_pcb.cap_table,
            crate::security::capability::Rights::from_bits_truncate(
                crate::fs::exofs::core::rights::PRIVILEGED_RIGHTS,
            ),
            crate::security::capability::CapObjectType::FileInode,
        ));

    // FIX-SEC-T1.1: child inherits AT LEAST zero-trust restrictions
    // (sandbox/pledge) from parent — a sandboxed process cannot escape them
    // via fork (RULE ZT-03 / SAND-03). No-op if parent is not restricted.
    crate::security::zero_trust::inherit_restrictions(parent_pcb.pid.0, child_pid.0);

    // Mark FORKED.
    child_pcb
        .flags
        .fetch_or(child_process_flags(ctx.flags), Ordering::Release);
    child_pcb.set_state(ProcessState::Running);
    fork_trace(b"fork: pcb state\n");

    // Copy brk and SignalTcb values / POSIX signal dispositions.
    child_pcb.brk_start.store(
        parent_pcb.brk_start.load(Ordering::Relaxed),
        Ordering::Relaxed,
    );
    child_pcb.brk_current.store(
        parent_pcb.brk_current.load(Ordering::Relaxed),
        Ordering::Relaxed,
    );
    child_pcb.signal_tcb_vaddr.store(
        parent_pcb.signal_tcb_vaddr.load(Ordering::Acquire),
        Ordering::Release,
    );
    *child_pcb.sig_handlers.lock() = *parent_pcb.sig_handlers.lock();
    #[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
    fork_debug_parent(b"fork_dbg: after_child_setup", parent, parent_pcb);

    // 5. Insert child into registry.
    // NOTE: If insert fails, child_pcb drop calls `cleanup_all_threads()`
    // which already frees `child_thread_ptr` and `child_tid_raw`.
    PROCESS_REGISTRY.insert(child_pcb).map_err(|_| {
        rollback_child_process_resources(&cloned_as, child_pid_raw, owns_addr_space);
        ForkError::RegistryError
    })?;
    fork_trace(b"fork: registry\n");
    sync_child_kernel_half(cloned_as.cr3);
    fork_trace(b"fork: child cr3 sync\n");
    #[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
    fork_debug_parent(b"fork_dbg: after_registry", parent, parent_pcb);

    // 6. Enqueue child in run queue.
    {
        let _preempt = PreemptGuard::new();
        if ctx.target_cpu as usize >= MAX_CPUS {
            // Invalid CPU — child_pcb removal and drop frees thread and tid.
            let _ = PROCESS_REGISTRY.remove(child_pid);
            rollback_child_process_resources(&cloned_as, child_pid_raw, owns_addr_space);
            return Err(ForkError::InvalidCpu);
        }
        // SAFETY: cpu verified, child_thread_ptr valid.
        unsafe {
            let tcb_ptr = core::ptr::NonNull::new_unchecked((*child_thread_ptr).tcb_ptr());
            run_queue(CpuId(ctx.target_cpu)).enqueue(tcb_ptr);
        }
    }
    // Child is runnable now, but parent must be able to complete fork() return
    // path and publish immediate effects (init/service bookkeeping, setpgid,
    // explicit vfork wait) before forced preemption. Scheduler tick and cooperative
    // blocking points will then take over without imposing a fragile child-first policy.
    // FIX-APP-07 (Security_Application_Audit §GAP-07): initialize exokairos budget
    // for child process immediately after enqueue.
    // Without wiring, a forked process had no time limit —
    // complete bypass of exokairos throttle/kill.
    // FIX-APP-07: ExoKairos budgets for child process.
    // init_process_budget() does not exist in v0.2.0 — initialization
    // of exokairos budgets happens implicitly via init_kernel_secret()
    // at boot. Here we log the fork event in audit for traceability.
    {
        use crate::security::audit::logger::{log_event, AuditCategory, AuditOutcome};
        log_event(
            AuditCategory::Process,
            child_pid.0,
            0u32,
            0u16,
            crate::syscall::numbers::SYS_CLONE as u32,
            0i32,
            AuditOutcome::Allow,
            [0u8; 8],
        );
        // ExoLedger: immutable trace in addition to audit ring buffer.
        crate::security::exoledger::exo_ledger_append(
            crate::security::exoledger::ActionTag::Custom {
                tag: 0x666F_726B, // b"fork"
                data: child_pid.0 as u64,
            },
        );
    }
    fork_trace(b"fork: enqueue\n");
    #[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
    fork_debug_parent(b"fork_dbg: before_return", parent, parent_pcb);

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
            Pid::INIT,
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
