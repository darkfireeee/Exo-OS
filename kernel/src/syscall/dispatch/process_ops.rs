//! # syscall/dispatch/process_ops.rs — Special handling for fork, vfork and execve
//!
//! These system calls require direct access and modification of the hardware
//! `SyscallFrame` (return RIP, userspace RSP, argument pointers, CR3 reload).

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use crate::arch::x86_64::syscall::SyscallFrame;
use super::stats::{exec_trace, syscall_trace};

// ─────────────────────────────────────────────────────────────────────────────
// Special processing for fork / vfork (SYS_FORK = 57, SYS_VFORK = 58)
// ─────────────────────────────────────────────────────────────────────────────

/// Processes `SYS_FORK` or `SYS_VFORK` directly from the arch frame.
///
/// Fork needs `frame.rcx` (return RIP = bifurcation point) and
/// `frame.rsp` (userspace RSP). These values do not pass in standard syscall
/// arguments (rdi..r9), so the normal handler cannot read them.
///
/// ## Protocol (POSIX fork)
/// 1. Reads current TCB via `gs:[0x20]` → PID → PCB.
/// 2. Retrieves `ProcessThread` via `pcb.main_thread_ptr()`.
/// 3. Constructs `ForkContext` from complete syscall frame.
/// 4. Calls `do_fork()` — creates child PCB/TCB, CoW, TLB flush, enqueues RunQueue.
/// 5. Returns `child_pid` to parent (child starts via `fork_child_trampoline`).
#[derive(Copy, Clone, Default)]
struct CloneExtras {
    child_rsp_override: Option<u64>,
    tls_override: Option<u64>,
    ptid_ptr: u64,
    ctid_ptr: u64,
    clear_child_tid: u64,
}

#[inline]
fn resolve_caller_thread_ptr(
    pcb: &crate::process::core::pcb::ProcessControlBlock,
    tcb: &crate::scheduler::core::task::ThreadControlBlock,
) -> *mut crate::process::core::tcb::ProcessThread {
    let tid = crate::process::core::pid::Tid(tcb.tid as u32);
    let mut ptr = pcb.find_thread_by_tid(tid);
    if ptr.is_null() {
        ptr = pcb.find_alive_thread();
    }
    ptr
}

pub fn handle_fork_like_inplace(
    frame: &SyscallFrame,
    fork_flags: crate::process::lifecycle::fork::ForkFlags,
) -> i64 {
    handle_fork_like_with_clone(frame, fork_flags, CloneExtras::default())
}

fn handle_fork_like_with_clone(
    frame: &SyscallFrame,
    fork_flags: crate::process::lifecycle::fork::ForkFlags,
    extras: CloneExtras,
) -> i64 {
    use crate::process::core::pid::Pid;
    use crate::process::core::registry::PROCESS_REGISTRY;
    use crate::process::lifecycle::fork::{
        do_fork, wait_for_vfork_completion, ForkContext, ForkError,
    };
    use crate::scheduler::core::task::ThreadControlBlock;
    use crate::syscall::errno::{EAGAIN, EFAULT, EINTR, EINVAL, ENOMEM};

    // Read current TCB.
    // SAFETY: Active kernel GS, gs:[0x20] = TCB pointer (PerCpuData).
    let tcb_ptr: u64;
    unsafe {
        core::arch::asm!("mov {}, gs:[0x20]", out(reg) tcb_ptr, options(nostack, nomem));
    }
    if tcb_ptr == 0 {
        return EAGAIN;
    }

    // SAFETY: non-null tcb_ptr maintained by scheduler.
    let tcb = unsafe { &*(tcb_ptr as *const ThreadControlBlock) };
    let pid = Pid(tcb.pid.0);

    let pcb = match PROCESS_REGISTRY.find_by_pid(pid) {
        Some(p) => p,
        None => return EAGAIN,
    };

    let thread_ptr = resolve_caller_thread_ptr(pcb, tcb);
    if thread_ptr.is_null() {
        return EAGAIN;
    }

    // SAFETY: thread_ptr maintained by pcb, valid as long as pcb is in PROCESS_REGISTRY.
    let thread = unsafe { &*thread_ptr };

    let ctx = ForkContext {
        parent_thread: thread,
        parent_pcb: pcb,
        flags: fork_flags,
        target_cpu: tcb.current_cpu().0,
        child_rip: frame.rcx,     // Return RIP saved by hw SYSCALL
        child_rsp: extras.child_rsp_override.unwrap_or(frame.rsp),
        parent_rflags: frame.r11, // RFLAGS saved by hw SYSCALL — CORRECTION P2-02
        user_rbx: frame.rbx,
        user_rbp: frame.rbp,
        user_r12: frame.r12,
        user_r13: frame.r13,
        user_r14: frame.r14,
        user_r15: frame.r15,
        user_rdi: frame.rdi,
        user_rsi: frame.rsi,
        user_rdx: frame.rdx,
        user_r10: frame.r10,
        user_r8: frame.r8,
        user_r9: frame.r9,
        tls_override: extras.tls_override,
        ptid_ptr: extras.ptid_ptr,
        ctid_ptr: extras.ctid_ptr,
        clear_child_tid: extras.clear_child_tid,
    };

    match do_fork(&ctx) {
        Ok(result) => {
            syscall_trace(b"sys_fork: do_fork ok\n");
            if fork_flags.has(crate::process::lifecycle::fork::ForkFlags::VFORK) {
                // SAFETY: tcb_ptr points to current TCB; `do_fork` finished and
                // no immutable reference to TCB is reused after this point.
                let tcb_mut = unsafe { &mut *(tcb_ptr as *mut ThreadControlBlock) };
                if wait_for_vfork_completion(result.child_pid, tcb_mut).is_err() {
                    return EINTR;
                }
            }
            result.child_pid.0 as i64
        }
        Err(e) => {
            syscall_trace(b"sys_fork: do_fork err\n");
            match e {
                ForkError::PidExhausted
                | ForkError::TidExhausted
                | ForkError::RegistryError
                | ForkError::InvalidCpu => EAGAIN,
                ForkError::OutOfMemory | ForkError::AddressSpaceCloneFailed => ENOMEM,
                ForkError::NoAddrCloner => EFAULT,
                ForkError::UnsupportedFlag => EINVAL,
            }
        }
    }
}

/// Processes `SYS_CLONE (56)` using standard Linux x86_64 5-argument ABI:
/// `(flags = rdi, child_stack = rsi, ptid = rdx, ctid = r10, tls = r8)`.
pub fn handle_clone_inplace(frame: &SyscallFrame) -> i64 {
    use crate::process::core::pid::Pid;
    use crate::process::core::registry::PROCESS_REGISTRY;
    use crate::process::lifecycle::fork::{ForkChildRegs, ForkFlags};
    use crate::process::thread::creation::{
        create_cloned_thread, CloneThreadParams, ThreadCreateError,
    };
    use crate::scheduler::core::task::ThreadControlBlock;
    use crate::syscall::errno::{EAGAIN, EFAULT, EINVAL, ENOMEM};
    use crate::syscall::validation::UserPtr;

    const CLONE_VM: u64 = 0x0000_0100;
    const CLONE_FS: u64 = 0x0000_0200;
    const CLONE_FILES: u64 = 0x0000_0400;
    const CLONE_SIGHAND: u64 = 0x0000_0800;
    const CLONE_VFORK: u64 = 0x0000_4000;
    const CLONE_THREAD: u64 = 0x0001_0000;
    const CLONE_SYSVSEM: u64 = 0x0004_0000;
    const CLONE_SETTLS: u64 = 0x0008_0000;
    const CLONE_PARENT_SETTID: u64 = 0x0010_0000;
    const CLONE_CHILD_CLEARTID: u64 = 0x0020_0000;
    const CLONE_DETACHED: u64 = 0x0040_0000;
    const CLONE_CHILD_SETTID: u64 = 0x0100_0000;
    const CLONE_NEWPID: u64 = 0x2000_0000;
    const THREAD_CORE: u64 = CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND | CLONE_THREAD;
    const SUPPORTED_THREAD_FLAGS: u64 = THREAD_CORE
        | CLONE_SYSVSEM
        | CLONE_SETTLS
        | CLONE_PARENT_SETTID
        | CLONE_CHILD_CLEARTID
        | CLONE_DETACHED
        | CLONE_CHILD_SETTID;

    let flags = frame.rdi;
    let child_stack = frame.rsi;
    let ptid = frame.rdx;
    let ctid = frame.r10;
    let tls = frame.r8;

    let set_ptid = if (flags & CLONE_PARENT_SETTID) != 0 { ptid } else { 0 };
    let set_ctid = if (flags & CLONE_CHILD_SETTID) != 0 { ctid } else { 0 };
    let clear_ctid = if (flags & CLONE_CHILD_CLEARTID) != 0 { ctid } else { 0 };

    if set_ptid != 0 && UserPtr::<u32>::from_raw(set_ptid).validate().is_err() {
        return EFAULT;
    }
    if (set_ctid != 0 || clear_ctid != 0) && UserPtr::<u32>::from_raw(ctid).validate().is_err() {
        return EFAULT;
    }

    if (flags & CLONE_THREAD) != 0 {
        if (flags & THREAD_CORE) != THREAD_CORE || (flags & !SUPPORTED_THREAD_FLAGS) != 0 {
            return EINVAL;
        }
        if child_stack == 0 || child_stack >= crate::syscall::USER_ADDR_MAX {
            return EINVAL;
        }
        let tls_base = if (flags & CLONE_SETTLS) != 0 {
            if tls != 0 && !crate::memory::core::address::is_user_canonical(tls) {
                return EINVAL;
            }
            tls
        } else {
            0
        };

        let tcb_ptr: u64;
        unsafe {
            core::arch::asm!("mov {}, gs:[0x20]", out(reg) tcb_ptr, options(nostack, nomem));
        }
        if tcb_ptr == 0 {
            return EAGAIN;
        }
        let tcb = unsafe { &*(tcb_ptr as *const ThreadControlBlock) };
        let Some(pcb) = PROCESS_REGISTRY.find_by_pid(Pid(tcb.pid.0)) else {
            return EAGAIN;
        };
        let parent_ptr = resolve_caller_thread_ptr(pcb, tcb);
        if parent_ptr.is_null() {
            return EAGAIN;
        }
        let parent = unsafe { &*parent_ptr };
        let effective_tls = if (flags & CLONE_SETTLS) != 0 {
            tls_base
        } else {
            parent.sched_tcb.fs_base
        };

        let params = CloneThreadParams {
            pcb,
            parent,
            child_rip: frame.rcx,
            child_rsp: child_stack,
            parent_rflags: frame.r11,
            regs: ForkChildRegs {
                rbx: frame.rbx,
                rbp: frame.rbp,
                r12: frame.r12,
                r13: frame.r13,
                r14: frame.r14,
                r15: frame.r15,
                rdi: frame.rdi,
                rsi: frame.rsi,
                rdx: frame.rdx,
                r10: frame.r10,
                r8: frame.r8,
                r9: frame.r9,
            },
            tls_base: effective_tls,
            detached: (flags & CLONE_DETACHED) != 0,
            target_cpu: tcb.current_cpu().0,
            ptid_ptr: set_ptid,
            ctid_ptr: set_ctid,
            clear_child_tid: clear_ctid,
        };

        return match create_cloned_thread(&params) {
            Ok(handle) => handle.tid.0 as i64,
            Err(ThreadCreateError::OutOfMemory) => ENOMEM,
            Err(ThreadCreateError::TidExhausted | ThreadCreateError::TooManyThreads) => EAGAIN,
            Err(ThreadCreateError::ProcessExiting) => EAGAIN,
            Err(_) => EINVAL,
        };
    }

    // Process-level clone (fork / vfork / CLONE_VM / CLONE_FILES).
    let mut fork_flags = ForkFlags::default();
    if (flags & CLONE_VM) != 0 {
        if child_stack == 0 && (flags & CLONE_VFORK) == 0 {
            return EINVAL;
        }
        fork_flags = fork_flags.set(ForkFlags::CLONE_VM);
    }
    if (flags & CLONE_FILES) != 0 {
        fork_flags = fork_flags.set(ForkFlags::CLONE_FILES);
    }
    if (flags & CLONE_SIGHAND) != 0 {
        fork_flags = fork_flags.set(ForkFlags::CLONE_SIGHAND);
    }
    if (flags & CLONE_VFORK) != 0 {
        fork_flags = fork_flags.set(ForkFlags::VFORK);
    }
    if (flags & CLONE_NEWPID) != 0 {
        fork_flags = fork_flags.set(ForkFlags::CLONE_NEWPID);
    }

    let extras = CloneExtras {
        child_rsp_override: if child_stack != 0 {
            Some(child_stack)
        } else {
            None
        },
        tls_override: if (flags & CLONE_SETTLS) != 0 {
            if tls != 0 && !crate::memory::core::address::is_user_canonical(tls) {
                return EINVAL;
            }
            Some(tls)
        } else {
            None
        },
        ptid_ptr: set_ptid,
        ctid_ptr: set_ctid,
        clear_child_tid: clear_ctid,
    };

    handle_fork_like_with_clone(frame, fork_flags, extras)
}

// ─────────────────────────────────────────────────────────────────────────────
// Special processing for execve (SYS_EXECVE = 59)
// ─────────────────────────────────────────────────────────────────────────────

/// Copies a null-terminated array of strings from user space.
///
/// `argv_ptr` points to an array of `char*` pointers terminated by NULL.
/// Returns `Some(Vec<String>)` or `None` if an address is invalid.
///
/// EXEC-01: only function authorized to read argv/envp from userspace.
pub fn copy_userspace_argv(
    argv_ptr: u64,
    max_args: usize,
) -> Option<Vec<String>> {
    use crate::syscall::validation::{copy_from_user, UserStr, USER_ADDR_MAX};

    if argv_ptr == 0 {
        return Some(Vec::new());
    }
    if argv_ptr >= USER_ADDR_MAX {
        return None;
    }

    let mut result: Vec<String> = Vec::new();
    let mut saw_null = false;

    for i in 0..max_args {
        // Read the i-th pointer (u64) from the array.
        let ptr_addr = match argv_ptr.checked_add(i as u64 * 8) {
            Some(a) if a <= USER_ADDR_MAX.saturating_sub(8) => a,
            _ => return None,
        };

        let mut raw = [0u8; core::mem::size_of::<u64>()];
        copy_from_user(raw.as_mut_ptr(), ptr_addr as *const u8, raw.len()).ok()?;
        let str_ptr = u64::from_ne_bytes(raw);

        if str_ptr == 0 {
            saw_null = true;
            break;
        } // NULL terminator of the array
        if str_ptr >= USER_ADDR_MAX {
            return None;
        }

        let user_str = UserStr::from_user(str_ptr, 4096).ok()?;

        // Permissive UTF-8 conversion (replaces invalid bytes with U+FFFD).
        result.push(String::from_utf8_lossy(user_str.as_bytes()).into_owned());
    }

    if saw_null {
        Some(result)
    } else {
        None
    }
}

/// Processes `SYS_EXECVE` directly from the arch frame.
///
/// On success, `do_execve()` replaces the process image but returns
/// normally in Rust. This handler then updates the arch frame so that
/// SYSRETQ jumps directly to the new ELF entry point.
///
/// ## Protocol
/// 1. Reads path from userspace via `frame.rdi`.
/// 2. Retrieves TCB → PCB → ProcessThread.
/// 3. Calls `do_execve()`.
/// 4. Success: `frame.rcx = entry_point`, `frame.rsp = initial_rsp`, `frame.rax = 0`.
/// 5. Failure: `frame.rax = -errno`.
///
/// ## Note: argv/envp
/// The `argv` and `envp` arrays are copied before image replacement via
/// centralized userspace primitives (`copy_from_user` / `UserStr`).
pub fn handle_execve_inplace(frame: &mut SyscallFrame) {
    use crate::process::core::pid::Pid;
    use crate::process::core::registry::PROCESS_REGISTRY;
    use crate::process::lifecycle::exec::{do_execve, ElfLoadError, ExecError};
    use crate::scheduler::core::task::ThreadControlBlock;
    use crate::syscall::errno::*;
    use crate::syscall::validation::read_user_path;

    exec_trace(b"execve: enter\n");

    // Read path from userspace.
    let user_path = match read_user_path(frame.rdi) {
        Ok(p) => p,
        Err(e) => {
            exec_trace(b"execve: path fault\n");
            frame.rax = e.to_errno() as u64;
            return;
        }
    };
    let path = match user_path.as_str() {
        Ok(s) => s,
        Err(_) => {
            exec_trace(b"execve: path utf8\n");
            frame.rax = EFAULT as u64;
            return;
        }
    };
    // DIAG E9: labeled print of execve path (unambiguous, greppable in trace mode).
    #[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
    {
        crate::arch::x86_64::terminal::debug_write(b"\n<EXEC ");
        crate::arch::x86_64::terminal::debug_write(path.as_bytes());
        crate::arch::x86_64::terminal::debug_write(b">");
    }

    // Read current TCB.
    // SAFETY: Active kernel GS.
    let tcb_ptr: u64;
    unsafe {
        core::arch::asm!("mov {}, gs:[0x20]", out(reg) tcb_ptr, options(nostack, nomem));
    }
    if tcb_ptr == 0 {
        exec_trace(b"execve: no tcb\n");
        frame.rax = EFAULT as u64;
        return;
    }

    // SAFETY: tcb_ptr maintained by scheduler.
    let tcb = unsafe { &*(tcb_ptr as *const ThreadControlBlock) };
    let pid = Pid(tcb.pid.0);

    let pcb = match PROCESS_REGISTRY.find_by_pid(pid) {
        Some(p) => p,
        None => {
            exec_trace(b"execve: no pcb\n");
            frame.rax = EFAULT as u64;
            return;
        }
    };

    let thread_ptr = resolve_caller_thread_ptr(pcb, tcb);
    if thread_ptr.is_null() {
        exec_trace(b"execve: no thread\n");
        frame.rax = EAGAIN as u64;
        return;
    }

    // SAFETY: thread_ptr maintained by pcb.
    let thread = unsafe { &mut *thread_ptr };

    // ARGV-01 (EXEC-01): copy argv/envp from userspace before do_execve().
    // frame.rsi = argv_ptr (null-terminated array of char*)
    // frame.rdx = envp_ptr (null-terminated array of char*)
    let argv_strings = match copy_userspace_argv(frame.rsi, 1024) {
        Some(v) => v,
        None => {
            exec_trace(b"execve: argv fault\n");
            frame.rax = EFAULT as u64;
            return;
        }
    };
    let argv_refs: Vec<&str> = argv_strings.iter().map(|s| s.as_str()).collect();

    let envp_strings = match copy_userspace_argv(frame.rdx, 4096) {
        Some(v) => v,
        None => {
            exec_trace(b"execve: env fault\n");
            frame.rax = EFAULT as u64;
            return;
        }
    };
    let envp_refs: Vec<&str> = envp_strings.iter().map(|s| s.as_str()).collect();

    exec_trace(b"execve: load\n");
    match do_execve(thread, pcb, &path, &argv_refs, &envp_refs) {
        Ok(()) => {
            exec_trace(b"execve: ok\n=OK\n");
            // Success: read new entry point from updated ProcessThread.
            let new_rip = thread.addresses.entry_point;
            let new_rsp = thread.addresses.initial_rsp;

            // Update frame for SYSRETQ.
            frame.rbx = 0;
            frame.rbp = 0;
            frame.r12 = 0;
            frame.r13 = 0;
            frame.r14 = 0;
            frame.r15 = 0;
            frame.rdi = thread.addresses.entry_arg0;
            frame.rsi = 0;
            frame.rdx = 0;
            frame.r8 = 0;
            frame.r9 = 0;
            frame.r10 = 0;
            frame.rcx = new_rip; // RIP → new ELF entry point
            frame.rsp = new_rsp; // RSP → new userspace stack
            frame.r11 = 0x0202; // RFLAGS: IF=1, reserved bit=1
            frame.rax = 0; // "success" (not returned — SYSRETQ jumps)

            // SYSRETQ reads gs:[0x08] to restore userspace RSP (ASM stub).
            // SAFETY: Active kernel GS, gs:[0x08] = user_rsp slot of PerCpuData.
            unsafe {
                core::arch::asm!(
                    "mov qword ptr gs:[0x08], {rsp}",
                    rsp = in(reg) new_rsp,
                    options(nostack, nomem)
                );
            }

            // FIX-EXEC-CR3: execve replaces address space WITHOUT context
            // switch. The syscall return stub (syscall.rs) reloads CR3 from
            // the per-CPU slot gs:[0x48] (user CR3), which is only updated by
            // `set_current_cr3()` during a context switch. Without this
            // republication, SYSRETQ returns with the OLD CR3: the process
            // resumes at new entry RIP but in old address space
            // (init and servers are all linked to the same virtual base),
            // executing old image code → fork/execve loop without
            // ever loading the new image.
            // DIAG E9: 'K' = execve CR3 fix path executed.
            #[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
            unsafe { core::arch::asm!("out 0xE9, al", in("al") b'K', options(nomem, nostack)) };
            let new_kernel_cr3 = thread.sched_tcb.cr3_phys;
            let new_user_cr3 = thread.sched_tcb.kpti_user_cr3();
            if crate::arch::x86_64::spectre::kpti::kpti_enabled() {
                crate::arch::x86_64::spectre::kpti::set_current_cr3(new_kernel_cr3, new_user_cr3);
            } else {
                // KPTI disabled: gs:[0x40] and gs:[0x48] MUST remain 0.
                // Calling set_current_cr3() with KPTI off wrote new_kernel_cr3
                // into gs:[0x40] and 0 into gs:[0x48]; as context_switch() does
                // not update gs:[0x40] when KPTI is off, any subsequent
                // syscall/IRQ/exception entry of another process (PID 1)
                // reloaded CR3 from this execve from gs:[0x40] and returned
                // to Ring 3 (gs:[0x48]==0) on the wrong address space (#25).
                crate::arch::x86_64::smp::percpu::set_kpti_cr3_slots(0, 0);
                // SAFETY: new_kernel_cr3 = PML4 of new image, kernel half
                // synchronized by do_execve(); Ring 0.
                unsafe {
                    core::arch::asm!("mov cr3, {}", in(reg) new_kernel_cr3, options(nostack, nomem));
                }
            }
        }
        Err(e) => {
            exec_trace(b"execve: err\n");
            exec_trace(match e {
                ExecError::ElfLoadFailed(ElfLoadError::NotFound) => b"=ER:NotFound\n" as &[u8],
                ExecError::ElfLoadFailed(ElfLoadError::InvalidElf) => b"=ER:InvalidElf\n",
                ExecError::ElfLoadFailed(ElfLoadError::PermissionDenied) => b"=ER:Perm\n",
                ExecError::ElfLoadFailed(ElfLoadError::OutOfMemory) => b"=ER:ElfOOM\n",
                ExecError::ElfLoadFailed(ElfLoadError::UnsupportedArch) => b"=ER:Arch\n",
                ExecError::ElfLoadFailed(ElfLoadError::InterpreterNotFound) => b"=ER:NoInterp\n",
                ExecError::OutOfMemory => b"=ER:OOM\n",
                ExecError::ThreadGroupNotSingle => b"=ER:NotSingle\n",
                ExecError::NoLoader => b"=ER:NoLoader\n",
                _ => b"=ER:Other\n",
            });
            let errno: i64 = match e {
                ExecError::ElfLoadFailed(ElfLoadError::NotFound) => ENOENT,
                ExecError::ElfLoadFailed(ElfLoadError::PermissionDenied)
                | ExecError::PermissionDenied => EACCES,
                ExecError::ElfLoadFailed(ElfLoadError::OutOfMemory) => ENOMEM,
                ExecError::ElfLoadFailed(ElfLoadError::InvalidElf) => EINVAL,
                ExecError::ArgListTooLong => E2BIG,
                ExecError::NameTooLong => EINVAL,
                ExecError::OutOfMemory => ENOMEM,
                ExecError::ThreadGroupNotSingle => EBUSY,
                ExecError::NoLoader => ENOSYS,
                _ => ENOSYS,
            };
            frame.rax = errno as u64;
        }
    }
}
