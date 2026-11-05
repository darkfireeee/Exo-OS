#![allow(unused_imports, dead_code)]
use super::*;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::arch::x86_64::{apic, halt_cpu, idt, memory_iface, paging, read_cr2, read_cr3, smp};

// ── C ABI Bridges to Scheduler (RULE FPU-02 + RULE IPI-01 DOC3) ─────────
//
// arch/ cannot import scheduler/ directly (to avoid Rust crate-level dependency
// cycle). The functions below are exported by the
// scheduler via `#[no_mangle] pub unsafe extern "C"` and resolved at link
// time.
//
// sched_fpu_handle_nm   — scheduler/fpu/lazy.rs     — #NM handler (lazy FPU)
// sched_ipi_reschedule  — scheduler/timer/tick.rs   — reschedule IPI
extern "C" {
    /// Handles the #NM (Device Not Available) exception for current TCB.
    /// Clears CR0.TS and restores FPU context via xrstor.
    /// `tcb_ptr` = GS:[0x20] (current_tcb). If null → simple clts.
    pub fn sched_fpu_handle_nm(tcb_ptr: *mut u8);
    /// Sets NEED_RESCHED on current thread following a reschedule IPI.
    /// `tcb_ptr` = GS:[0x20] (current_tcb). If null → ignored.
    pub fn sched_ipi_reschedule(tcb_ptr: *mut u8);
    /// Scheduler timer tick: advances quanta, decides preemption.
    /// `cpu_id` = GS:[0x10], `current` = GS:[0x20].
    pub fn scheduler_tick(cpu_id: u32, current: *mut u8);
    /// Bridge arch/ → process/: signal delivery upon Ring 3 exception return.
    /// `tcb_ptr` = GS:[0x20], `excframe` = current &mut ExceptionFrame.
    pub fn proc_signal_on_exception_return(tcb_ptr: *mut u8, excframe: *mut u8);
}

// ── Exception Frame ─────────────────────────────────────────────────────────

/// Registers saved by CPU + stub ASM during an exception
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct ExceptionFrame {
    // Registers saved by ASM stub (caller-saved)
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rbp: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,

    // Pushed by stub or CPU depending on the exception
    pub error_code: u64,

    // Pushed automatically by CPU
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64, // Present only if privilege level changed
    pub ss: u64,  // Same
}

impl ExceptionFrame {
    /// Returns `true` if exception came from Ring 3 (userspace)
    #[inline(always)]
    pub fn from_userspace(&self) -> bool {
        (self.cs & 3) == 3
    }

    /// Returns `true` if exception came from kernel (Ring 0)
    #[inline(always)]
    pub fn from_kernel(&self) -> bool {
        (self.cs & 3) == 0
    }
}


// ── Exception Rust Handlers ────────────────────────────────────────────────

#[inline(always)]
pub fn current_tcb_raw_checked() -> u64 {
    // SAFETY: read first verifies MSR_GS_BASE points to kernel per-CPU
    // area before touching `gs:[0x20]`.
    unsafe { crate::arch::x86_64::smp::percpu::try_read_current_tcb().unwrap_or(0) }
}

#[inline(always)]
pub fn current_cpu_id_checked() -> u32 {
    crate::arch::x86_64::smp::percpu::try_current_cpu_id().unwrap_or(0)
}

/// Return to userspace after exception from Ring 3
///
/// ## RULE SIGNAL-01 (DOC1)
/// Orchestration point for signals after preemption or exception.
/// Checks `signal_pending` in TCB and orchestrates delivery.
/// Actual delivery is delegated to `process::signal::delivery`.
pub fn exception_return_to_user_inner(frame: &mut ExceptionFrame, allow_resched: bool) {
    let tcb_ptr = current_tcb_raw_checked();
    if tcb_ptr == 0 {
        return;
    }
    // SAFETY: proc_signal_on_exception_return thread-safe; excluded by implicit cli in handler.
    unsafe {
        proc_signal_on_exception_return(
            tcb_ptr as *mut u8,
            frame as *mut ExceptionFrame as *mut u8,
        );
    }

    if allow_resched {
        // Timer/IPI IRQs only assert NEED_RESCHED. Ring 3 return is the
        // stable point where switching to another thread is possible, keeping the
        // exception frame on the preempted thread's kernel stack.
        // SAFETY: called at IRQ return, stable preemption point; the exception
        // frame stays on the preempted thread's kernel stack during switch.
        unsafe {
            let _ = crate::scheduler::core::switch::schedule_current_if_needed();
        }
    }
}

/// Normal return from a Ring 3 exception.
#[inline]
pub fn exception_return_to_user(frame: &mut ExceptionFrame) {
    exception_return_to_user_inner(frame, true);
}

/// Returns from a recovered fault without switching threads between PTE
/// fixup and replay of the faulting instruction.
///
/// A CoW stack write may have faulted in the middle of a `call`. Rescheduling
/// here lets the child progress while the parent still owns the exception
/// frame that must replay this `call`; return address remained null
/// in dump #25. Signals continue to be delivered, only preemption
/// is deferred until next safe kernel entry point.
#[inline]
pub fn exception_return_to_user_after_recovered_fault(frame: &mut ExceptionFrame) {
    exception_return_to_user_inner(frame, false);
}

#[inline]
pub fn queue_signal_for_current(sig: crate::process::signal::Signal) {
    let tcb_raw = current_tcb_raw_checked();
    // DIAG-V14-SIGTRACE (bisect #GP1 livelock, run 13/14): bit-for-bit identical #GP1
    // from one run to another, without ever a single <SG pid=... k=...>
    // (printed by deliver_one before any action), proves that delivery
    if tcb_raw == 0 {
        return;
    }
    // SAFETY: verified tcb_raw != 0; points to current thread TCB published
    // by scheduler for this CPU, alive during handler.
    let tcb = unsafe { &*(tcb_raw as *const crate::scheduler::core::task::ThreadControlBlock) };
    let pid = crate::process::core::pid::Pid(tcb.pid.0);
    let _ = crate::process::signal::delivery::send_signal_to_pid(pid, sig);
}

/// DIAG-UD1/GP1 (bisect init termination): complete dump of fatal userspace
/// exception — same format as <SEGV> DIAG-25 marker from do_page_fault:
/// pid, rip, rsp, ripb (8 IN-MEMORY bytes at RIP: leading `0f 0b` = genuine
/// compiled `ud2`; random bytes = corrupted indirect jump #25), key
/// registers, and 8 user stack words (return address chain).
///
/// Guarded volatile reads: only if address is in plausible user range —
/// unguarded read would re-fault in kernel and corrupt the dump.
pub fn diag_user_exception_dump(tag: &[u8], frame: &ExceptionFrame) {
    let out = crate::arch::x86_64::terminal::debug_write;
    let hex = |val: u64| {
        let lut = b"0123456789abcdef";
        let mut hb = [0u8; 16];
        let mut x = val;
        let mut k = 16;
        while k > 0 {
            k -= 1;
            hb[k] = lut[(x & 0xf) as usize];
            x >>= 4;
        }
        hb
    };

    out(tag);
    // Decimal PID of current thread.
    let tcb_raw = current_tcb_raw_checked();
    let mut v = if tcb_raw != 0 {
        // SAFETY: verified tcb_raw != 0; current TCB published by scheduler.
        unsafe {
            (*(tcb_raw as *const crate::scheduler::core::task::ThreadControlBlock))
                .pid
                .0 as u64
        }
    } else {
        0
    };
    if v == 0 {
        out(b"0");
    } else {
        let mut d = [0u8; 20];
        let mut i = d.len();
        while v != 0 && i > 0 {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
        }
        out(&d[i..]);
    }

    out(b" rip=");
    out(&hex(frame.rip));
    out(b" rsp=");
    out(&hex(frame.rsp));
    // Bytes at RIP: protected by fixup to avoid nested #PF (T1-025 FIX).
    let mut rb = 0u64;
    let cpu = crate::arch::x86_64::smp::percpu::current_cpu_id() as usize;
    if frame.rip >= 0x1000 && frame.rip < 0x0000_8000_0000_0000 {
        let mut buf = [0u8; 8];
        if crate::syscall::fixup::copy_from_user_safe(buf.as_mut_ptr(), frame.rip as *const u8, 8, cpu) {
            rb = u64::from_le_bytes(buf);
        }
    }
    out(b" ripb=");
    out(&hex(rb));
    out(b" rax=");
    out(&hex(frame.rax));
    out(b" rcx=");
    out(&hex(frame.rcx));
    out(b" rdx=");
    out(&hex(frame.rdx));
    out(b" rsi=");
    out(&hex(frame.rsi));
    out(b" rdi=");
    out(&hex(frame.rdi));
    out(b" rbp=");
    out(&hex(frame.rbp));
    // DIAG-V8 (SEGV run 9): r8-r15 were missing from dump — faulting
    // instruction BASE register ([rbx-0x82]/[rbp-0x82]/[r8..15+disp]) is
    // likely among them. Without them, impossible to distinguish between "dereferenced
    // syscall return -11" and "pointer reloaded from zeroed stack".
    out(b" r8=");
    out(&hex(frame.r8));
    out(b" r9=");
    out(&hex(frame.r9));
    out(b" r10=");
    out(&hex(frame.r10));
    out(b" r11=");
    out(&hex(frame.r11));
    out(b" r12=");
    out(&hex(frame.r12));
    out(b" r13=");
    out(&hex(frame.r13));
    out(b" r14=");
    out(&hex(frame.r14));
    out(b" r15=");
    out(&hex(frame.r15));
    // 8 user stack words at RSP: return address chain — protected by fixup (T1-025 FIX).
    out(b" stk=");
    if frame.rsp >= 0x1000 && frame.rsp < 0x0000_8000_0000_0000 {
        let mut words = [0u64; 8];
        if crate::syscall::fixup::copy_from_user_safe(words.as_mut_ptr() as *mut u8, frame.rsp as *const u8, 64, cpu) {
            for &w in &words {
                out(&hex(w));
                out(b" ");
            }
        }
    }
    // Extended DIAG-UD1/SEGV (run 5): 16 bytes at RDI — protected by fixup (T1-025 FIX).
    out(b" rdib=");
    if frame.rdi >= 0x1000 && frame.rdi < 0x0000_8000_0000_0000 {
        let mut bytes = [0u8; 16];
        if crate::syscall::fixup::copy_from_user_safe(bytes.as_mut_ptr(), frame.rdi as *const u8, 16, cpu) {
            let lut = b"0123456789abcdef";
            for &byte in &bytes {
                out(&[lut[(byte >> 4) as usize], lut[(byte & 0xf) as usize]]);
            }
        }
    }
    out(b">\n");
}

#[cfg(all(debug_assertions, exo_kernel_trace))]
pub fn debug_hex64(value: u64) {
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

#[cfg(all(debug_assertions, exo_kernel_trace))]
pub fn debug_read_cr3() -> u64 {
    let cr3: u64;
    // SAFETY: reading CR3 in Ring 0, memory-effect-free instruction.
    unsafe {
        core::arch::asm!(
            "mov {v}, cr3",
            v = out(reg) cr3,
            options(nostack, nomem, preserves_flags),
        );
    }
    cr3 & !0xFFFu64
}


// ── Kernel Panic ──────────────────────────────────────────────────────────────

/// Prints exception info and halts the CPU
///
/// # NO-ALLOC RULE
/// This function CANNOT allocate memory.
/// It displays registers from the frame only.
#[cold]
#[inline(never)]
pub fn kernel_panic_exception(msg: &str, frame: &mut ExceptionFrame) {
    if crate::exophoenix::resurrection::try_recover_exception(msg, frame) {
        return;
    }

    // In a full implementation: write to serial port or VGA buffer.
    // For now: consume parameters to avoid warnings.
    let _ = msg;
    let _ = frame;

    // Broadcast panic IPI to all APs to stop them.
    // SAFETY: unrecoverable situation; broadcast_ipi_panic() does not allocate.
    crate::arch::x86_64::apic::ipi::broadcast_ipi_panic();

    // SAFETY: unrecoverable situation — infinite HLT loop.
    crate::arch::x86_64::halt_cpu()
}

// ── Instrumentation ──────────────────────────────────────────────────────────

/// Per-vector counters (vectors 0–31 only here)
pub static EXC_COUNTERS: [AtomicU64; 32] = [const { AtomicU64::new(0) }; 32];

pub static NMI_COUNT: AtomicU64 = AtomicU64::new(0);
pub static MC_COUNT: AtomicU64 = AtomicU64::new(0);
pub static GP_FAULT_COUNT: AtomicU64 = AtomicU64::new(0);
pub static TIMER_IRQ_COUNT: AtomicU64 = AtomicU64::new(0);
pub static SPURIOUS_IRQ_COUNT: AtomicU64 = AtomicU64::new(0);
pub static FPU_DEVICE_NOT_AVAIL_COUNT: AtomicU64 = AtomicU64::new(0);

pub fn exc_count(vector: u8) -> u64 {
    if (vector as usize) < 32 {
        EXC_COUNTERS[vector as usize].load(Ordering::Relaxed)
    } else {
        0
    }
}
pub fn nmi_count() -> u64 {
    NMI_COUNT.load(Ordering::Relaxed)
}
pub fn machine_check_count() -> u64 {
    MC_COUNT.load(Ordering::Relaxed)
}
pub fn gp_fault_count() -> u64 {
    GP_FAULT_COUNT.load(Ordering::Relaxed)
}
pub fn timer_irq_count() -> u64 {
    TIMER_IRQ_COUNT.load(Ordering::Relaxed)
}
pub fn spurious_irq_count() -> u64 {
    SPURIOUS_IRQ_COUNT.load(Ordering::Relaxed)
}
pub fn fpu_device_not_avail_count() -> u64 {
    FPU_DEVICE_NOT_AVAIL_COUNT.load(Ordering::Relaxed)
}

pub static USER_PF_DEBUG_COUNT: AtomicU64 = AtomicU64::new(0);
