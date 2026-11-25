//! # syscall/dispatch/signals.rs — Signal handling and delivery for dispatch
//!
//! Signal orchestration upon return to user space:
//! - Restore user context for `rt_sigreturn` (SIG-13 / SIG-14)
//! - Check and deliver pending signals before SYSRETQ (RULE SIGNAL-01)

use crate::arch::x86_64::syscall::SyscallFrame;

// ─────────────────────────────────────────────────────────────────────────────
// Special processing for rt_sigreturn (SIG-13 / SIG-14)
// ─────────────────────────────────────────────────────────────────────────────

/// Processes `SYS_RT_SIGRETURN` directly from the arch frame.
///
/// Unlike other syscalls, `rt_sigreturn` must modify userspace registers
/// (RIP, RSP, RFLAGS, RAX…) returned by SYSRETQ. The standard handler
/// has no access to `SyscallFrame` — this special handling allows it.
///
/// ## Protocol (POSIX / SIG-13)
/// 1. Userspace RSP at SYSRETQ time = sig_rsp + 8 (the handler `ret` has
///    consumed `pretcode`).
/// 2. `sig_rsp = frame.user_rsp - 8`.
/// 3. UContext at `sig_rsp + SIGNAL_FRAME_UC_OFFSET`.
/// 4. Magic `0x5349_474E` verified constant-time (LAC-01 / SIG-14).
/// 5. If magic OK: registers + signal_mask restored.
/// 6. Userspace RSP updated in `gs:[0x08]` for SYSRETQ.
///
/// On failure (invalid magic, invalid address): process receives SIGSEGV
/// upon return to userspace (RIP will be 0, which is unmapped).
/// Sanitizes RFLAGS for return to Ring 3 (anti-elevation IOPL/VM/TF, forces IF=1).
#[inline]
pub fn sanitize_user_rflags(rflags: u64) -> u64 {
    const RFLAGS_SAFE_MASK: u64 = 0x0000_0000_0024_0CD5; // CF,PF,AF,ZF,SF,DF,OF,AC,ID
    const RFLAGS_FORCE_SET: u64 = 0x0000_0000_0000_0200; // IF=1 (interrupts enabled in user)
    const RFLAGS_FORCE_CLR: u64 = 0x0000_0000_0003_7100; // TF=0, IOPL=0, NT=0, RF=0, VM=0
    ((rflags & RFLAGS_SAFE_MASK) | RFLAGS_FORCE_SET) & !RFLAGS_FORCE_CLR
}

pub fn handle_sigreturn_inplace(frame: &mut SyscallFrame) {
    use crate::process::signal::handler::{
        extract_and_sanitize_sigframe, restore_fpu_regs_from_bytes, SIGNAL_FRAME_UC_OFFSET,
    };
    use crate::scheduler::core::task::ThreadControlBlock;
    use core::sync::atomic::Ordering;

    // Handler's `ret` popped `pretcode` → userspace RSP = sig_rsp + 8.
    let sig_rsp = frame.rsp.wrapping_sub(8);
    let uc_ptr = sig_rsp + SIGNAL_FRAME_UC_OFFSET;

    let (regs, fpregs) = match extract_and_sanitize_sigframe(uc_ptr) {
        Some(v) => v,
        None => {
            // Invalid magic or corrupted address: trigger SIGSEGV via RIP=0 in Ring3.
            frame.rcx = 0;
            frame.rax = (-crate::syscall::errno::EFAULT) as u64;
            return;
        }
    };

    restore_fpu_regs_from_bytes(&fpregs);

    // Restore general registers into arch frame (read by SYSRETQ).
    frame.rcx = regs.rip;
    frame.r11 = sanitize_user_rflags(regs.rflags);
    frame.rax = regs.rax;
    frame.rdi = regs.rdi;
    frame.rsi = regs.rsi;
    frame.rdx = regs.rdx;
    frame.r8 = regs.r8;
    frame.r9 = regs.r9;
    frame.r10 = regs.r10;
    frame.r12 = regs.r12;
    frame.r13 = regs.r13;
    frame.r14 = regs.r14;
    frame.r15 = regs.r15;
    frame.rbx = regs.rbx;
    frame.rbp = regs.rbp;
    // SAFETY: Active kernel GS, gs:[0x08] = user_rsp slot of PerCpuData.
    unsafe {
        crate::arch::x86_64::smp::percpu::set_user_rsp(regs.rsp);
    }
    frame.rsp = regs.rsp;

    // Restore signal mask, TLS, and handler flag on per-thread state.
    let tcb_ptr = unsafe { crate::arch::x86_64::smp::percpu::read_current_tcb() };
    if tcb_ptr != 0 {
        let safe_mask = crate::process::signal::mask::SigMask::from(regs.signal_mask).0;
        let tcb = unsafe { &mut *(tcb_ptr as *mut ThreadControlBlock) };
        tcb.signal_mask.store(safe_mask, Ordering::Release);
        if crate::memory::core::address::is_user_canonical(regs.fs_base) {
            tcb.fs_base = regs.fs_base;
            #[cfg(target_os = "none")]
            unsafe {
                crate::arch::x86_64::cpu::msr::write_msr(
                    crate::arch::x86_64::cpu::msr::MSR_FS_BASE,
                    regs.fs_base,
                );
            }
        }
        if crate::memory::core::address::is_user_canonical(regs.gs_base) {
            tcb.user_gs_base = regs.gs_base;
            #[cfg(target_os = "none")]
            unsafe {
                crate::arch::x86_64::cpu::msr::write_msr(
                    crate::arch::x86_64::cpu::msr::MSR_KERNEL_GS_BASE,
                    regs.gs_base,
                );
            }
        }

        if let Some(pcb) = crate::process::core::registry::PROCESS_REGISTRY
            .find_by_pid(crate::process::core::pid::Pid(tcb.pid.0))
        {
            let tid = crate::process::core::pid::Tid(tcb.tid as u32);
            let mut thread_ptr = pcb.find_thread_by_tid(tid);
            if thread_ptr.is_null() {
                thread_ptr = pcb.find_alive_thread();
            }
            if !thread_ptr.is_null() {
                let thread = unsafe { &mut *thread_ptr };
                if crate::memory::core::address::is_user_canonical(regs.fs_base) {
                    thread.tls_gs_base.store(regs.fs_base, Ordering::Release);
                }
                thread.in_handler.store(0, Ordering::Release);
                crate::process::signal::delivery::sync_pending_after_unblock(thread, safe_mask);
            }
            if pcb.thread_count.load(Ordering::Acquire) <= 1 {
                let vaddr = pcb.signal_tcb_vaddr.load(Ordering::Acquire);
                if (0x1000..0x0000_7FFF_FFFF_F000).contains(&vaddr) {
                    if let Ok(alias) =
                        crate::syscall::validation::resolve_user_page_kernel_alias(vaddr, true)
                    {
                        let stcb =
                            unsafe { &*(alias as *const crate::process::signal::tcb::SignalTcb) };
                        stcb.set_mask(safe_mask);
                        stcb.in_handler.store(0, Ordering::Release);
                    }
                }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Pending signal verification and delivery
// ─────────────────────────────────────────────────────────────────────────────

/// Checks and delivers pending signals before returning to userspace.
///
/// ## RULE SIGNAL-01 / SIGNAL-02 (DOC1)
/// - `arch/` orchestrates (reads flag, calls process/signal/).
/// - `process/signal/delivery` actually delivers.
/// - `scheduler/` never delivers directly.
///
/// ## Implementation
/// Reads `gs:[0x20]` → TCB pointer → `signal_pending` field.
/// If set → calls `process::signal::delivery::handle_pending_signals()`.
/// Delivery modifies the frame if it installs a userspace handler.
#[inline]
pub fn check_and_deliver_signals(frame: &mut SyscallFrame) {
    // SAFETY: Active kernel GS in this context (SWAPGS in ASM stub).
    // gs:[0x20] contains TCB pointer, potentially null if not yet
    // initialized (kernel case before first fork → no signal possible).
    let tcb_ptr = unsafe { crate::arch::x86_64::smp::percpu::read_current_tcb() };

    if tcb_ptr == 0 {
        // No current thread (boot / idle) → no signal.
        return;
    }

    // SAFETY: tcb_ptr is non-null and points to a valid ThreadControlBlock
    // (constructed by lifecycle/create.rs and maintained by scheduler).
    use crate::scheduler::core::task::ThreadControlBlock;
    let tcb = unsafe { &*(tcb_ptr as *const ThreadControlBlock) };

    // Check flag with Acquire to synchronize with signal_pending write
    // performed by process/signal/ (Ordering::Release applies there).
    if !tcb.has_signal_pending() {
        return;
    }

    // ── arch::SyscallFrame → delivery::SyscallFrame conversion ───────────────
    use crate::process::core::pid::Pid;
    use crate::process::core::registry::PROCESS_REGISTRY;
    use crate::process::signal::delivery::{handle_pending_signals, SyscallFrame as DeliveryFrame};

    let pid = Pid(tcb.pid.0);
    let pcb = match PROCESS_REGISTRY.find_by_pid(pid) {
        Some(p) => p,
        None => return,
    };

    let tid = crate::process::core::pid::Tid(tcb.tid as u32);
    let mut thread_ptr = pcb.find_thread_by_tid(tid);
    if thread_ptr.is_null() {
        thread_ptr = pcb.main_thread_ptr();
    }
    if thread_ptr.is_null() {
        return;
    }

    // SAFETY: thread_ptr maintained by pcb, valid in this context.
    let thread = unsafe { &mut *thread_ptr };

    let mut d_frame = DeliveryFrame {
        user_rip: frame.rcx,    // Return RIP (saved by hw SYSCALL in RCX)
        user_rflags: frame.r11, // RFLAGS (saved by hw SYSCALL in R11)
        user_rsp: frame.rsp,    // userspace RSP
        user_rax: frame.rax,    // syscall return value
        user_rdi: frame.rdi,
        user_rsi: frame.rsi,
        user_rdx: frame.rdx,
        user_rcx: frame.rcx, // userspace RCX = same as return RIP (SYSCALL)
        user_r8: frame.r8,
        user_r9: frame.r9,
        user_r10: frame.r10,
        user_r12: frame.r12,
        user_r13: frame.r13,
        user_r14: frame.r14,
        user_r15: frame.r15,
        user_rbx: frame.rbx,
        user_rbp: frame.rbp,
        user_fs_base: thread.sched_tcb.fs_base,
        user_gs_base: thread.sched_tcb.user_gs_base,
        user_cs: 0x1B, // Ring3 CS (not saved by SYSCALL but required)
        user_ss: 0x23, // Ring3 SS
    };

    handle_pending_signals(thread, &mut d_frame);

    // Propagate potential modifications (e.g. redirected RIP, RSP to sigaltstack).
    frame.rcx = d_frame.user_rip;
    frame.r11 = sanitize_user_rflags(d_frame.user_rflags);
    frame.rax = d_frame.user_rax;
    frame.rdi = d_frame.user_rdi;
    frame.rsi = d_frame.user_rsi;
    frame.rdx = d_frame.user_rdx;
    frame.r8 = d_frame.user_r8;
    frame.r9 = d_frame.user_r9;
    frame.r10 = d_frame.user_r10;
    frame.r12 = d_frame.user_r12;
    frame.r13 = d_frame.user_r13;
    frame.r14 = d_frame.user_r14;
    frame.r15 = d_frame.user_r15;
    frame.rbx = d_frame.user_rbx;
    frame.rbp = d_frame.user_rbp;

    // Userspace RSP: update gs:[0x08] if modified (sigaltstack / setup_signal_frame).
    if d_frame.user_rsp != frame.rsp {
        let new_rsp = d_frame.user_rsp;
        // SAFETY: Active kernel GS, gs:[0x08] = user_rsp slot of PerCpuData.
        unsafe {
            crate::arch::x86_64::smp::percpu::set_user_rsp(new_rsp);
        }
        frame.rsp = new_rsp;
    }
}
