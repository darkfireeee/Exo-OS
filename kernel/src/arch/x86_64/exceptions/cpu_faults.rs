#![allow(unused_imports, dead_code, clippy::missing_safety_doc)]
use super::*;
use super::frame::*;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::arch::x86_64::{apic, halt_cpu, idt, memory_iface, paging, read_cr2, read_cr3, smp};

#[no_mangle]
pub unsafe extern "C" fn do_divide_error(frame: *mut ExceptionFrame) {
    // SAFETY: `frame` is a non-null pointer passed by the ASM stub, 16-byte aligned,
    // unique for this exception context. Its lifetime is guaranteed by the
    // stack frame pushed by the CPU prior to calling the handler.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[0].fetch_add(1, Ordering::Relaxed);

    if frame.from_userspace() {
        queue_signal_for_current(crate::process::signal::Signal::SIGFPE);
        exception_return_to_user(frame);
    } else {
        kernel_panic_exception("Division par zéro kernel", frame);
    }
}

/// Handler #DB — Debug exception
#[no_mangle]
pub unsafe extern "C" fn do_debug(frame: *mut ExceptionFrame) {
    // SAFETY: identical to do_divide_error — valid pointer passed by ASM stub.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[1].fetch_add(1, Ordering::Relaxed);

    if frame.from_userspace() {
        // SIGTRAP → ptrace/GDB
        exception_return_to_user(frame);
    } else {
        kernel_panic_exception("#DB kernel", frame);
    }
}

/// Handler #NMI — Non-Maskable Interrupt
///
/// NMI uses IST2 (dedicated stack independent of current stack).
/// DO NOT call code that could trigger an exception.
#[no_mangle]
pub unsafe extern "C" fn do_nmi(frame: *mut ExceptionFrame) {
    // SAFETY: `frame` is provided by the NMI interrupt stub on IST2, points
    // to a valid ExceptionFrame constructed by CPU on entry to handler.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[2].fetch_add(1, Ordering::Relaxed);
    NMI_COUNT.fetch_add(1, Ordering::Relaxed);

    if crate::exophoenix::resurrection::handle_nmi(frame) {
        return;
    }

    crate::security::exonmi::exonmi_check_idt_integrity();
}

/// Handler #BP — Breakpoint (INT3)
#[no_mangle]
pub unsafe extern "C" fn do_breakpoint(frame: *mut ExceptionFrame) {
    // SAFETY: identical to do_divide_error — valid pointer passed by ASM stub.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[3].fetch_add(1, Ordering::Relaxed);

    if frame.from_userspace() {
        // SIGTRAP
        exception_return_to_user(frame);
    }
    // Kernel: kprobe or kernel debug
}

/// Handler #OF — Overflow (INTO)
#[no_mangle]
pub unsafe extern "C" fn do_overflow(frame: *mut ExceptionFrame) {
    // SAFETY: identical to do_divide_error — valid pointer passed by ASM stub.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[4].fetch_add(1, Ordering::Relaxed);
    if frame.from_userspace() {
        queue_signal_for_current(crate::process::signal::Signal::SIGFPE);
        exception_return_to_user(frame);
    } else {
        kernel_panic_exception("Overflow kernel", frame);
    }
}

/// Handler #BR — Bound Range Exceeded
#[no_mangle]
pub unsafe extern "C" fn do_bound_range(frame: *mut ExceptionFrame) {
    // SAFETY: identical to do_divide_error — valid pointer passed by ASM stub.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[5].fetch_add(1, Ordering::Relaxed);
    if frame.from_userspace() {
        queue_signal_for_current(crate::process::signal::Signal::SIGSEGV);
        exception_return_to_user(frame);
    } else {
        kernel_panic_exception("Bound Range kernel", frame);
    }
}

/// Handler #UD — Invalid Opcode
#[no_mangle]
pub unsafe extern "C" fn do_invalid_opcode(frame: *mut ExceptionFrame) {
    // SAFETY: identical to do_divide_error — valid pointer passed by ASM stub.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[6].fetch_add(1, Ordering::Relaxed);

    if frame.from_userspace() {
        // DIAG-UD1 (bisect init termination): complete dump BEFORE delivering SIGILL —
        // rip + ripb (in-memory bytes: `0f 0b` = compiled ud2, random bytes
        // = corrupted jump #25) + stack (return address chain).
        diag_user_exception_dump(b"<UD1 ", frame);
        // FIX-UD-SIGILL: actually deliver SIGILL. Without this, returning to the
        // faulting RIP as-is → invalid instruction (typically server-side Rust panic
        // `ud2`) re-triggers #UD immediately → kernel livelock
        // (do_invalid_opcode loop → iretq → same opcode) freezing the entire system.
        // Like #PF/#GP, we queue SIGILL then return to Ring 3: the
        // signal is delivered (handler or termination) instead of looping.
        queue_signal_for_current(crate::process::signal::Signal::SIGILL);
        exception_return_to_user(frame);
    } else {
        // Check if it is an XTEST instruction (TSX) — retpoline sometimes generates #UD
        // For now: kernel panic
        kernel_panic_exception("Invalid Opcode kernel", frame);
    }
}

/// Handler #NM — Device Not Available (FPU lazy)
///
/// Triggered when CR0.TS=1 and a thread attempts to use the FPU.
/// scheduler/fpu/lazy.rs manages the logic — arch/ handles the exception.
#[no_mangle]
pub unsafe extern "C" fn do_device_not_avail(frame: *mut ExceptionFrame) {
    let _ = frame;
    EXC_COUNTERS[7].fetch_add(1, Ordering::Relaxed);
    FPU_DEVICE_NOT_AVAIL_COUNT.fetch_add(1, Ordering::Relaxed);

    let tcb_ptr = current_tcb_raw_checked();

    // Delegate to scheduler (RULE FPU-02 DOC3):
    //   • Clears CR0.TS (allows FPU/SIMD access).
    //   • Calls xrstor to restore current thread FPU context.
    //   • Updates fpu_loaded = true in TCB.
    // If tcb_ptr == 0 (boot / idle): simple clts without xrstor.
    // SAFETY: sched_fpu_handle_nm() is thread-safe for current CPU.
    unsafe {
        sched_fpu_handle_nm(tcb_ptr as *mut u8);
    }
}

/// Handler #DF — Double Fault
///
/// Uses IST1 (dedicated stack). If we arrive here, main kernel stack
/// is likely corrupted or overflowed.
///
/// DO NOT allocate, DO NOT acquire locks.
#[no_mangle]
pub unsafe extern "C" fn do_double_fault(frame: *mut ExceptionFrame) {
    // DIAG: unconditional raw marker on entry (#DF on IST1).
    // SAFETY: port E9, no memory side effects.
    unsafe { core::arch::asm!("out 0xE9, al", in("al") b'D', options(nomem, nostack)) };
    EXC_COUNTERS[8].fetch_add(1, Ordering::Relaxed);
    df_debug_write(b"\n[#DF] double fault");
    if !frame.is_null() {
        // SAFETY: frame non-null (verified); ExceptionFrame constructed by CPU
        // on IST stack of #DF, valid for reading during handler.
        let frame = unsafe { &*frame };
        df_debug_field(b" rip=0x", frame.rip);
        df_debug_field(b" cs=0x", frame.cs);
        df_debug_field(b" rsp=0x", frame.rsp);
        df_debug_field(b" rflags=0x", frame.rflags);
        df_debug_field(b" err=0x", frame.error_code);
    }
    df_debug_field(
        b" cpu=0x",
        crate::arch::x86_64::cpu::topology::current_cpu_id().0 as u64,
    );
    df_debug_write(b"\n");

    // NO-ALLOC RULE: this function cannot perform any allocation
    // It can only halt the CPU
    // SAFETY: unrecoverable situation — immediate halt
    crate::arch::x86_64::halt_cpu();
}

#[inline(always)]
pub fn df_debug_write(bytes: &[u8]) {
    crate::arch::x86_64::terminal::debug_write(bytes);
}

pub fn df_debug_hex64(value: u64) {
    let mut out = [0u8; 16];
    for (idx, byte) in out.iter_mut().enumerate() {
        let shift = 60 - (idx as u32 * 4);
        let nibble = ((value >> shift) & 0x0f) as u8;
        *byte = if nibble < 10 {
            b'0' + nibble
        } else {
            b'a' + (nibble - 10)
        };
    }
    df_debug_write(&out);
}

pub fn df_debug_field(label: &[u8], value: u64) {
    df_debug_write(label);
    df_debug_hex64(value);
}

/// Handler #TS — Invalid TSS
#[no_mangle]
pub unsafe extern "C" fn do_invalid_tss(frame: *mut ExceptionFrame) {
    // SAFETY: identical to do_divide_error — valid pointer passed by ASM stub.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[10].fetch_add(1, Ordering::Relaxed);
    kernel_panic_exception("Invalid TSS", frame);
}

/// Handler #NP — Segment Not Present
#[no_mangle]
pub unsafe extern "C" fn do_segment_not_present(frame: *mut ExceptionFrame) {
    // SAFETY: identical to do_divide_error — valid pointer passed by ASM stub.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[11].fetch_add(1, Ordering::Relaxed);
    if frame.from_userspace() {
        queue_signal_for_current(crate::process::signal::Signal::SIGBUS);
        exception_return_to_user(frame);
    } else {
        kernel_panic_exception("Segment Not Present kernel", frame);
    }
}

/// Handler #SS — Stack Fault
#[no_mangle]
pub unsafe extern "C" fn do_stack_fault(frame: *mut ExceptionFrame) {
    // SAFETY: identical to do_divide_error — valid pointer passed by ASM stub.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[12].fetch_add(1, Ordering::Relaxed);
    if frame.from_userspace() {
        queue_signal_for_current(crate::process::signal::Signal::SIGSEGV);
        exception_return_to_user(frame);
    } else {
        kernel_panic_exception("Stack Fault kernel", frame);
    }
}

/// Handler #GP — General Protection Fault
#[no_mangle]
pub unsafe extern "C" fn do_general_protection(frame: *mut ExceptionFrame) {
    // SAFETY: identical to do_divide_error — valid pointer passed by ASM stub.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[13].fetch_add(1, Ordering::Relaxed);
    GP_FAULT_COUNT.fetch_add(1, Ordering::Relaxed);

    if frame.from_userspace() {
        // DIAG-GP1 (bisect init termination): complete dump of userspace #GP.
        diag_user_exception_dump(b"<GP1 ", frame);
        // FIX-GP-SIGSEGV: Ring 3 #GP must deliver SIGSEGV. The old code set
        // NO signal and returned to same instruction → #GP in a
        // loop (silent livelock). Same handling as #PF/#UD.
        queue_signal_for_current(crate::process::signal::Signal::SIGSEGV);
        exception_return_to_user(frame);
    } else {
        kernel_panic_exception("#GP kernel", frame);
    }
}


#[no_mangle]
pub unsafe extern "C" fn do_x87_fp(frame: *mut ExceptionFrame) {
    // SAFETY: identical to do_divide_error — valid pointer passed by ASM stub.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[16].fetch_add(1, Ordering::Relaxed);
    if frame.from_userspace() {
        queue_signal_for_current(crate::process::signal::Signal::SIGFPE);
        exception_return_to_user(frame);
    } else {
        kernel_panic_exception("#MF x87 kernel", frame);
    }
}

/// Handler #AC — Alignment Check
#[no_mangle]
pub unsafe extern "C" fn do_alignment_check(frame: *mut ExceptionFrame) {
    // SAFETY: identical to do_divide_error — valid pointer passed by ASM stub.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[17].fetch_add(1, Ordering::Relaxed);
    if frame.from_userspace() {
        queue_signal_for_current(crate::process::signal::Signal::SIGBUS);
        exception_return_to_user(frame);
    } else {
        kernel_panic_exception("#AC kernel", frame);
    }
}

/// Handler #MC — Machine Check
///
/// Uses IST3 (dedicated stack).
/// Cannot be recovered. Immediate halt.
#[no_mangle]
pub unsafe extern "C" fn do_machine_check(_frame: *mut ExceptionFrame) {
    EXC_COUNTERS[18].fetch_add(1, Ordering::Relaxed);
    MC_COUNT.fetch_add(1, Ordering::Relaxed);
    // SAFETY: machine check = unrecoverable hardware failure
    crate::arch::x86_64::halt_cpu();
}

/// Handler #XM — SIMD FP Exception
#[no_mangle]
pub unsafe extern "C" fn do_simd_fp(frame: *mut ExceptionFrame) {
    // SAFETY: identical to do_divide_error — valid pointer passed by ASM stub.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[19].fetch_add(1, Ordering::Relaxed);
    if frame.from_userspace() {
        queue_signal_for_current(crate::process::signal::Signal::SIGFPE);
        exception_return_to_user(frame);
    } else {
        kernel_panic_exception("#XM SIMD FP kernel", frame);
    }
}

/// Handler #VE — Virtualization Exception
#[no_mangle]
pub unsafe extern "C" fn do_virtualization(frame: *mut ExceptionFrame) {
    // SAFETY: identical to do_divide_error — valid pointer passed by ASM stub.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[20].fetch_add(1, Ordering::Relaxed);
    if frame.from_userspace() {
        queue_signal_for_current(crate::process::signal::Signal::SIGSEGV);
        exception_return_to_user(frame);
    } else {
        kernel_panic_exception("#VE virtualization kernel", frame);
    }
}

/// Handler #CP — Control Protection Exception (CET)
#[no_mangle]
pub unsafe extern "C" fn do_ctrl_protection(frame: *mut ExceptionFrame) {
    // SAFETY: identical to do_divide_error — valid pointer passed by ASM stub.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[21].fetch_add(1, Ordering::Relaxed);
    crate::security::exocage::cp_handler(frame as *mut ExceptionFrame as usize, frame.error_code);

    if frame.from_userspace() {
        exception_return_to_user(frame);
    } else {
        kernel_panic_exception("#CP Control Protection kernel (post ExoCage)", frame);
    }
}

