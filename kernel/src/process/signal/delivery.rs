// kernel/src/process/signal/delivery.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// POSIX signal delivery — Exo-OS Layer 1.5
// RULE SIGNAL-01: handle_pending_signals called ONLY on syscall return.
// ═══════════════════════════════════════════════════════════════════════════════

use super::default::{default_action, SigAction, SigActionKind, Signal};
use super::queue::SigInfo;
use crate::process::core::pcb::ProcessState;
use crate::process::core::pid::Pid;
use crate::process::core::registry::PROCESS_REGISTRY;
use crate::scheduler::core::task::ThreadControlBlock;
use core::sync::atomic::Ordering;

/// Signals supported by TCB's `AtomicU64` mask.
///
/// Bit 0 remains reserved for invalid signal 0; standard signals occupy
/// 1..31 and real-time signals 32..63 inclusive. A hypothetical signal 64
/// cannot be represented without widening the mask.
pub const MAX_SIGNAL_NUMBER: u8 = 63;
pub const RT_SIGNAL_MIN: u8 = 32;
pub const RT_SIGNAL_MAX: u8 = MAX_SIGNAL_NUMBER;
const RT_SIGNAL_MASK_SHIFT: u8 = RT_SIGNAL_MIN - 1;

// ─────────────────────────────────────────────────────────────────────────────
// Errors
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendError {
    /// Non-existent target PID.
    NoSuchProcess,
    /// Invalid signal number.
    InvalidSignal,
    /// Permission denied (EPERM).
    PermissionDenied,
}

// ─────────────────────────────────────────────────────────────────────────────
// Signal sending from kernel (kill(2), raise, SIGCHLD...)
// ─────────────────────────────────────────────────────────────────────────────

/// FIX-SIGWAKE (bisect hang boot): wakes up a sleeping thread that receives a
/// signal.
///
/// Prior to this fix, `raise_signal_pending()` only set the
/// `signal_pending` + NEED_RESCHED bits on the **target** TCB. If the target is
/// sleeping (nanosleep, futex, wait IPC), this NEED_RESCHED is invisible:
/// no one consults it as long as the thread is not running. The signal
/// therefore remained pending indefinitely and an interrupted `nanosleep` never
/// returned (lost wake). Linux does the equivalent via
/// `signal_wake_up()` → `wake_up_state(TASK_INTERRUPTIBLE)`.
///
/// Here: transition Sleeping → Runnable + enqueue into thread's CPU
/// runqueue. No double-enqueue possible (enqueue checks try_mark_queued).
fn wake_thread_for_signal(sched: &ThreadControlBlock) {
    use crate::scheduler::core::preempt::{IrqGuard, MAX_CPUS};
    use crate::scheduler::core::runqueue::run_queue;
    use crate::scheduler::core::task::{CpuId, TaskState};
    use core::ptr::NonNull;

    let cpu_raw = sched.cpu_id.load(Ordering::Relaxed) as usize;
    if cpu_raw >= MAX_CPUS {
        return;
    }
    let _irq = IrqGuard::new();
    if !sched.try_transition(TaskState::Sleeping, TaskState::Runnable) {
        // Already Runnable/Running (signal will be delivered at next syscall
        // return) — nothing to do.
        return;
    }
    // SAFETY: cpu_raw is bounded by MAX_CPUS and comes from CPU assigned to TCB;
    // IRQs and preemption are disabled by IrqGuard during access.
    let rq = unsafe { run_queue(CpuId(cpu_raw as u32)) };
    // `sched` is the TCB of a living thread from process registry; TCB
    // lives in static pool and is not freed during the call.
    let nn = NonNull::from(sched);
    rq.enqueue(nn);
    // Preempt current if same CPU for rapid delivery (same logic
    // as sleep_timer_wake).
    let current = crate::scheduler::core::switch::current_thread_raw();
    if !current.is_null() {
        // SAFETY: valid current TCB published by scheduler.
        let cur = unsafe { &*current };
        if cur.current_cpu().0 as usize == cpu_raw {
            cur.request_preemption();
        }
    }
}

/// Sends a signal to a target process by PID.
/// Called by do_exit() (SIGCHLD), kill(2), etc.
///
/// Algorithm:
/// 1. Look up PCB in PROCESS_REGISTRY.
/// 2. For standard signal: set bit in pending_signals of TCB
///    of "main" thread (thread 0) of process.
/// 3. For RT signal: push into thread's RTSigQueue.
/// 4. Request preemption via raise_signal_pending().
pub fn send_signal_to_pid(pid: Pid, sig: Signal) -> Result<(), SendError> {
    send_signal_number_to_pid(pid, sig.number())
}

/// Sends a numerical signal to a target process by PID.
///
/// Unlike [`send_signal_to_pid`], this path also accepts real-time signals
/// 32..63, which do not have a variant in the standard POSIX signal enum.
pub fn send_signal_number_to_pid(pid: Pid, sig_n: u8) -> Result<(), SendError> {
    if sig_n == 0 || sig_n > MAX_SIGNAL_NUMBER {
        return Err(SendError::InvalidSignal);
    }
    let pcb = PROCESS_REGISTRY
        .find_by_pid(pid)
        .ok_or(SendError::NoSuchProcess)?;

    // A zombie process can no longer receive signals (except ignored SIGCHLD).
    let state = pcb.state();
    if state == ProcessState::Zombie || state == ProcessState::Dead {
        return Ok(());
    }

    // Retrieve an alive thread from the target process.
    let thread_ptr = pcb.find_alive_thread();
    if thread_ptr.is_null() {
        return Err(SendError::NoSuchProcess);
    }

    // SAFETY: thread_ptr is valid as long as PCB is alive; under PCB
    // spinlock (write_lock) -- here read-only access to pointer is sufficient.
    let thread = unsafe { &*thread_ptr };

    // Enqueue signal.
    if sig_n < RT_SIGNAL_MIN {
        thread.sig_queue.enqueue(sig_n);
    } else {
        let info = SigInfo::kernel(sig_n);
        thread.rt_sig_queue.enqueue(sig_n, info);
    }

    // Notify scheduler: signal_pending = true (PROC-04 via raise_signal_pending).
    thread.raise_signal_pending();
    // FIX-SIGWAKE: wake up target if sleeping, otherwise signal is
    // never seen (sleeping thread does not go through syscall return).
    wake_thread_for_signal(&thread.sched_tcb);
    Ok(())
}

/// Sends a signal directly to a known scheduler TCB.
/// Used when we already have the TCB pointer (e.g. exception delivery).
pub fn send_signal_to_tcb(
    thread: &crate::process::core::tcb::ProcessThread,
    sig: u8,
    info: SigInfo,
) {
    if sig == 0 || sig > MAX_SIGNAL_NUMBER {
        return;
    }
    if sig < RT_SIGNAL_MIN {
        thread.sig_queue.enqueue(sig);
    } else {
        thread.rt_sig_queue.enqueue(sig, info);
    }
    thread.raise_signal_pending();
    // FIX-SIGWAKE: wake up sleeping target (cf. send_signal_number_to_pid).
    wake_thread_for_signal(&thread.sched_tcb);
}

// ─────────────────────────────────────────────────────────────────────────────
// handle_pending_signals — RULE SIGNAL-01: called only on syscall return
// ─────────────────────────────────────────────────────────────────────────────

/// Syscall return context (pointers to saved registers).
/// Populated by asm syscall-entry and passed here to allow handler
/// to modify user registers (rip, rsp, rax...).
#[repr(C)]
pub struct SyscallFrame {
    pub user_rsp: u64,
    pub user_rip: u64,
    pub user_rflags: u64,
    pub user_rax: u64, // syscall return value
    pub user_rdi: u64,
    pub user_rsi: u64,
    pub user_rdx: u64,
    pub user_rcx: u64,
    pub user_r8: u64,
    pub user_r9: u64,
    pub user_r10: u64,
    pub user_r12: u64,
    pub user_r13: u64,
    pub user_r14: u64,
    pub user_r15: u64,
    pub user_rbx: u64,
    pub user_rbp: u64,
    pub user_fs_base: u64,
    pub user_gs_base: u64,
    pub user_cs: u64,
    pub user_ss: u64,
}

/// Processes all pending unblocked signals.
/// **Called ONLY on return from a syscall** (RULE SIGNAL-01).
///
/// Algorithm for each dequeued signal:
/// 1. Check mask -> skip if blocked.
/// 2. Read SigAction in PCB table.
/// 3. Dispatch according to kind: User | Ignore | Term | Core | Stop | Cont.
/// 4. SA_RESETHAND: reset handler after delivery.
pub fn handle_pending_signals(
    thread: &mut crate::process::core::tcb::ProcessThread,
    frame: &mut SyscallFrame,
) {
    use crate::process::core::pid::Pid;

    // No signal_pending? Fast exit.
    if !thread.sched_tcb.has_signal_pending() {
        return;
    }

    // SIG-08 / SIG-24: Acquire fence before reading signal state / SignalTcb.
    core::sync::atomic::fence(Ordering::Acquire);

    let pid = Pid(thread.sched_tcb.pid.0);

    // Read handler table from PCB.
    let pcb = match PROCESS_REGISTRY.find_by_pid(pid) {
        Some(p) => p,
        None => {
            clear_signal_pending(thread);
            return;
        }
    };

    const NON_BLOCKABLE: u64 =
        (1u64 << (Signal::SIGKILL as u8 - 1)) | (1u64 << (Signal::SIGSTOP as u8 - 1));
    const USER_SPACE_TOP: u64 = 0x0000_7FFF_FFFF_F000;
    let sig_tcb_vaddr = pcb.signal_tcb_vaddr.load(Ordering::Acquire);
    let sig_tcb_opt = if (0x1000..USER_SPACE_TOP).contains(&sig_tcb_vaddr) {
        // SAFETY: handle_pending_signals executes in CR3 of current process (syscall/exception return).
        Some(unsafe { &*(sig_tcb_vaddr as *const super::tcb::SignalTcb) })
    } else {
        None
    };

    let mut mask = thread.sched_tcb.signal_mask.load(Ordering::Acquire);
    if let Some(stcb) = sig_tcb_opt {
        let user_blocked = stcb.get_mask() & !NON_BLOCKABLE;
        mask = (mask | user_blocked) & !NON_BLOCKABLE;
        let q_pending = thread.sig_queue.pending.load(Ordering::Acquire);
        if q_pending != 0 {
            stcb.pending.fetch_or(q_pending, Ordering::AcqRel);
        }
    }

    // Delivery loop: process until nothing remains.
    loop {
        // Dequeue from standard queue.
        let maybe = thread.sig_queue.dequeue(mask);
        let (sig_n, info) = if let Some(pair) = maybe {
            pair
        } else {
            // Try RT queue.
            let rt_mask = rt_mask_from_signal_mask(mask);
            if let Some(pair) = thread.rt_sig_queue.dequeue(rt_mask) {
                pair
            } else {
                break; // Nothing else.
            }
        };

        if let Some(stcb) = sig_tcb_opt {
            stcb.consume(sig_n);
        }

        let mut action = {
            let handlers = pcb.sig_handlers.lock();
            handlers.get(sig_n)
        };
        if let Some(stcb) = sig_tcb_opt {
            if let Some(entry) = stcb.get_action(sig_n) {
                if entry.is_ign() && sig_n != Signal::SIGKILL as u8 && sig_n != Signal::SIGSTOP as u8 {
                    action = SigAction::IGN;
                } else if entry.is_user() && sig_n != Signal::SIGKILL as u8 && sig_n != Signal::SIGSTOP as u8 {
                    action = SigAction {
                        kind: SigActionKind::User,
                        handler: entry.handler_vaddr,
                        flags: entry.flags,
                        mask: entry.mask,
                        restorer: entry.restorer,
                    };
                }
            }
        }

        deliver_one(thread, frame, sig_n, info, action, pcb);

        // SA_RESETHAND: handler → SIG_DFL after first delivery.
        if action.flags & SigAction::SA_RESETHAND != 0 {
            let def = default_action(sig_n);
            let mut handlers = pcb.sig_handlers.lock();
            handlers.set(sig_n, def);
            if let Some(stcb) = sig_tcb_opt {
                if let Some(idx) = sig_n.checked_sub(1).map(|i| i as usize) {
                    if idx < super::tcb::NSIG {
                        // SAFETY: writing handler slot of current thread on syscall return.
                        unsafe {
                            let slot_ptr = &stcb.handlers[idx]
                                as *const super::tcb::SigactionEntry
                                as *mut super::tcb::SigactionEntry;
                            slot_ptr.write_volatile(super::tcb::SigactionEntry::default_dfl());
                        }
                    }
                }
            }
        }

        // If a user handler was just installed on return frame,
        // deliver a single signal per return to Ring 3 (next one delivered on sigreturn).
        if action.kind == SigActionKind::User {
            break;
        }
    }

    // Clear flag if nothing pending remains.
    let remaining_std = thread.sig_queue.pending.load(Ordering::Acquire) & !mask;
    let remaining_rt = (thread.rt_sig_queue.pending_mask.load(Ordering::Acquire) as u32)
        & !rt_mask_from_signal_mask(mask);
    if remaining_std == 0 && remaining_rt == 0 {
        clear_signal_pending(thread);
    }
}

#[inline]
fn rt_mask_from_signal_mask(mask: u64) -> u32 {
    (mask >> RT_SIGNAL_MASK_SHIFT) as u32
}

#[cfg(test)]
mod tests {
    use super::{rt_mask_from_signal_mask, RT_SIGNAL_MAX, RT_SIGNAL_MIN};

    #[test]
    fn rt_mask_maps_signal_32_to_first_rt_slot() {
        let signal_32_mask = 1u64 << (RT_SIGNAL_MIN - 1);

        assert_eq!(rt_mask_from_signal_mask(signal_32_mask), 1);
    }

    #[test]
    fn rt_mask_maps_signal_63_to_last_rt_slot() {
        let signal_63_mask = 1u64 << (RT_SIGNAL_MAX - 1);

        assert_eq!(rt_mask_from_signal_mask(signal_63_mask), 1u32 << 31);
    }
}

/// Delivers a single signal.
fn deliver_one(
    thread: &mut crate::process::core::tcb::ProcessThread,
    frame: &mut SyscallFrame,
    sig_n: u8,
    info: SigInfo,
    action: SigAction,
    pcb: &crate::process::core::pcb::ProcessControlBlock,
) {
    use super::handler::setup_signal_frame;
    use crate::process::lifecycle::exit::do_exit;

    match action.kind {
        SigActionKind::Ignore => {
            // Nothing to do.
        }
        SigActionKind::User => {
            // Build a user frame to execute the handler.
            // RULE SIGNAL-01: setup_signal_frame modifies frame->user_rip / user_rsp.
            setup_signal_frame(thread, frame, sig_n, &info, &action);
        }
        SigActionKind::Stop => {
            // Put process into Stopped state + notify parent.
            pcb.set_state(crate::process::core::pcb::ProcessState::Stopped);
            // Send SIGCHLD to parent (POSIX requirement).
            let ppid = pcb.ppid();
            if ppid.0 != 0 {
                let _ = send_signal_to_pid(ppid, Signal::SIGCHLD);
            }
            // Block thread until SIGCONT (POSIX SIGSTOP semantics).
            // Thread will be woken up by deliver_one(SIGCONT) via PCB::set_state(Running).
            thread
                .sched_tcb
                .set_state(crate::scheduler::core::task::TaskState::Sleeping);
            // SAFETY: sched_tcb is the current TCB; sole holder on this CPU (preemption disabled).
            unsafe {
                let cpu_id = thread.sched_tcb.current_cpu();
                let rq = crate::scheduler::core::runqueue::run_queue(cpu_id);
                crate::scheduler::schedule_block(rq, &mut *thread.sched_tcb);
            }
        }
        SigActionKind::Cont => {
            // Wake up process if it was stopped.
            let state = pcb.state();
            if state == crate::process::core::pcb::ProcessState::Stopped {
                pcb.set_state(crate::process::core::pcb::ProcessState::Running);
            }
        }
        SigActionKind::Term | SigActionKind::Core => {
            // Terminate process by signal (bit 31 = signal, bit 7 = core dump, bits 0..6 = signal).
            let is_core = matches!(action.kind, SigActionKind::Core);
            let exit_status: u32 =
                0x8000_0000 | ((sig_n as u32) & 0x7F) | if is_core { 0x80 } else { 0 };
            do_exit(thread, pcb, exit_status);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Internal helpers
// ─────────────────────────────────────────────────────────────────────────────

#[inline(always)]
fn clear_signal_pending(thread: &crate::process::core::tcb::ProcessThread) {
    thread.sched_tcb.clear_signal_pending();
}

// ─────────────────────────────────────────────────────────────────────────────
// Bridge arch/ → process/: signal delivery on exception return
// ─────────────────────────────────────────────────────────────────────────────

/// Exact mirror of `arch::x86_64::exceptions::ExceptionFrame` (repr(C)).
/// Used to access without circular dependency from this module.
/// INVARIANT: any change in ExceptionFrame MUST be reflected here.
#[repr(C)]
struct ExcFrame {
    r15: u64,
    r14: u64,
    r13: u64,
    r12: u64,
    r11: u64,
    r10: u64,
    r9: u64,
    r8: u64,
    rbp: u64,
    rdi: u64,
    rsi: u64,
    rdx: u64,
    rcx: u64,
    rbx: u64,
    rax: u64,
    error_code: u64,
    rip: u64,
    cs: u64,
    rflags: u64,
    rsp: u64,
    ss: u64,
}

/// C ABI bridge for signal delivery on exception return.
///
/// Called from `arch::x86_64::exceptions::exception_return_to_user()` via
/// `extern "C"`. Current TCB is at `GS:[0x20]`, exception frame is
/// passed directly.
///
/// # Contract RULE SIGNAL-01
/// - Only executes on return to Ring 3 (from_userspace() checked by caller).
/// - `handle_pending_signals` is never called from scheduler (RULE SWITCH-02).
///
/// # NOTE multi-thread
/// At this stage (single-threaded kernel per process), `main_thread_ptr()` IS the
/// current thread. In future multi-threading, a per-CPU register `current_thread_ptr`
/// must replace this passage through PCB.
///
/// # Safety
/// `tcb_ptr` and `excframe_ptr` must point to valid and non-shared structures:
/// - `tcb_ptr`: pointer to current `ThreadControlBlock` (GS:[0x20]).
/// - `excframe_ptr`: pointer to `ExceptionFrame` on kernel stack.
#[no_mangle]
pub unsafe extern "C" fn proc_signal_on_exception_return(tcb_ptr: *mut u8, excframe_ptr: *mut u8) {
    if tcb_ptr.is_null() || excframe_ptr.is_null() {
        return;
    }

    // Fast read of signal_pending flag — avoids PROCESS_REGISTRY lookup
    // if no signal is pending.
    let sched_tcb = &*(tcb_ptr as *const ThreadControlBlock);
    if !sched_tcb.has_signal_pending() {
        return;
    }

    // Look up PCB by pid (needed for handlers and mask).
    let pid = Pid(sched_tcb.pid.0);
    let pcb = match PROCESS_REGISTRY.find_by_pid(pid) {
        Some(p) => p,
        None => {
            // Unknown process — clear orphan flag and exit.
            sched_tcb.clear_signal_pending();
            return;
        }
    };

    // Retrieve main ProcessThread.
    // SAFETY: At this stage process is single-threaded; main_thread_ptr() == current thread.
    let thread_ptr = pcb.main_thread_ptr();
    if thread_ptr.is_null() {
        return;
    }
    let thread = &mut *thread_ptr;

    // Build a SyscallFrame from ExceptionFrame to reuse
    // handle_pending_signals() (which normalizes userspace frames).
    let exc = &*(excframe_ptr as *const ExcFrame);
    let mut frame = SyscallFrame {
        user_rsp: exc.rsp,
        user_rip: exc.rip,
        user_rflags: exc.rflags,
        user_rax: exc.rax,
        user_rdi: exc.rdi,
        user_rsi: exc.rsi,
        user_rdx: exc.rdx,
        user_rcx: exc.rcx,
        user_r8: exc.r8,
        user_r9: exc.r9,
        user_r10: exc.r10,
        user_r12: exc.r12,
        user_r13: exc.r13,
        user_r14: exc.r14,
        user_r15: exc.r15,
        user_rbx: exc.rbx,
        user_rbp: exc.rbp,
        user_fs_base: thread.sched_tcb.fs_base,
        user_gs_base: thread.sched_tcb.user_gs_base,
        user_cs: exc.cs,
        user_ss: exc.ss,
    };

    // Effective delivery — modifies frame if a user handler is installed.
    handle_pending_signals(thread, &mut frame);

    // Reflect potentially modified registers (e.g. RIP redirected to
    // signal handler, RSP to sigaltstack) in actual frame.
    let exc_mut = &mut *(excframe_ptr as *mut ExcFrame);
    exc_mut.rsp = frame.user_rsp;
    exc_mut.rip = frame.user_rip;
    exc_mut.rflags = frame.user_rflags;
    exc_mut.rax = frame.user_rax;
    exc_mut.rdi = frame.user_rdi;
    exc_mut.rsi = frame.user_rsi;
    exc_mut.rdx = frame.user_rdx;
    exc_mut.rcx = frame.user_rcx;
    exc_mut.r8 = frame.user_r8;
    exc_mut.r9 = frame.user_r9;
    exc_mut.r10 = frame.user_r10;
    exc_mut.r12 = frame.user_r12;
    exc_mut.r13 = frame.user_r13;
    exc_mut.r14 = frame.user_r14;
    exc_mut.r15 = frame.user_r15;
    exc_mut.rbx = frame.user_rbx;
    exc_mut.rbp = frame.user_rbp;
    exc_mut.cs = frame.user_cs;
    exc_mut.ss = frame.user_ss;
}
