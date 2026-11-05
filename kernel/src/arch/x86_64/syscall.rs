//! # arch/x86_64/syscall.rs — SYSCALL/SYSRET Entry Point
//!
//! Manages 64-bit system call entry and exit via SYSCALL/SYSRET.
//!
//! ## DOC1 Rule (RULE SIGNAL-01 / SIGNAL-02)
//! arch/ ORCHESTRATES signal delivery upon returning to userspace.
//! arch/ calls `process::signal::delivery::handle_pending_signals()`.
//!
//! ## Exo-OS Syscall ABI (Linux-compatible)
//! - rax = syscall number
//! - rdi, rsi, rdx, r10, r8, r9 = arguments
//! - rcx = saved by SYSCALL (userspace return RIP)
//! - r11 = saved by SYSCALL (userspace RFLAGS)
//!
//! ## SWAPGS
//! - Entry: SWAPGS to access kernel per-CPU data
//! - Exit: SWAPGS before SYSRET to restore userspace GS
//!
//! ## KPTI
//! The syscall path switches CR3 directly in `syscall_entry_asm` via the
//! per-CPU GS slots `kpti_kernel_cr3`/`kpti_user_cr3`. `switch_asm.s` only covers
//! scheduler context switches.

use core::sync::atomic::{AtomicU64, Ordering};

use super::cpu::msr;
use super::gdt::{GDT_KERNEL_CS, GDT_USER_CS32};

// ── Syscall Numbers ──────────────────────────────────────────────────────────

pub const SYSCALL_READ: u64 = 0;
pub const SYSCALL_WRITE: u64 = 1;
pub const SYSCALL_OPEN: u64 = 2;
pub const SYSCALL_CLOSE: u64 = 3;
pub const SYSCALL_STAT: u64 = 4;
pub const SYSCALL_FSTAT: u64 = 5;
pub const SYSCALL_LSTAT: u64 = 6;
pub const SYSCALL_POLL: u64 = 7;
pub const SYSCALL_LSEEK: u64 = 8;
pub const SYSCALL_MMAP: u64 = 9;
pub const SYSCALL_MPROTECT: u64 = 10;
pub const SYSCALL_MUNMAP: u64 = 11;
pub const SYSCALL_BRK: u64 = 12;
pub const SYSCALL_RT_SIGACTION: u64 = 13;
pub const SYSCALL_RT_SIGPROCMASK: u64 = 14;
pub const SYSCALL_RT_SIGRETURN: u64 = 15;
pub const SYSCALL_IOCTL: u64 = 16;
pub const SYSCALL_FORK: u64 = 57;
pub const SYSCALL_VFORK: u64 = 58;
pub const SYSCALL_EXECVE: u64 = 59;
pub const SYSCALL_EXIT: u64 = 60;
pub const SYSCALL_WAIT4: u64 = 61;
pub const SYSCALL_KILL: u64 = 62;
pub const SYSCALL_CLONE: u64 = 56;
pub const SYSCALL_FUTEX: u64 = 202;
pub const SYSCALL_SCHED_YIELD: u64 = 24;
pub const SYSCALL_NANOSLEEP: u64 = 35;
pub const SYSCALL_GETPID: u64 = 39;
pub const SYSCALL_SOCKET: u64 = 41;
pub const SYSCALL_CONNECT: u64 = 42;
pub const SYSCALL_ACCEPT: u64 = 43;
pub const SYSCALL_SENDTO: u64 = 44;
pub const SYSCALL_RECVFROM: u64 = 45;

/// Maximum supported syscall number
pub const SYSCALL_MAX: u64 = 512;

// ── Syscall Frame ────────────────────────────────────────────────────────────

/// Registers saved at SYSCALL entry
///
/// ## Memory Layout (MUST match ASM push order)
/// Fields are in ascending address order: rax is lowest
/// (last push = [rsp+0]), rcx is highest (first push = [rsp+120]).
///
/// ASM push order (first to last):
///   push rcx  → [rsp+120]  (return RIP)
///   push r11  → [rsp+112]  (userspace RFLAGS)
///   push rbp  → [rsp+104]
///   push rbx  → [rsp+96]
///   push r12  → [rsp+88]
///   push r13  → [rsp+80]
///   push r14  → [rsp+72]
///   push r15  → [rsp+64]
///   push user_rsp → [rsp+56]
///   push rsi  → [rsp+48]   (arg2)
///   push rdi  → [rsp+40]   (arg1)
///   push rdx  → [rsp+32]   (arg3)
///   push r10  → [rsp+24]   (arg4)
///   push r8   → [rsp+16]   (arg5)
///   push r9   → [rsp+8]    (arg6)
///   push rax  → [rsp+0]    (syscall nr / return)
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct SyscallFrame {
    pub rax: u64, // offset   0 — syscall number / return value (last push)
    pub r9: u64,  // offset   8 — arg6
    pub r8: u64,  // offset  16 — arg5
    pub r10: u64, // offset  24 — arg4
    pub rdx: u64, // offset  32 — arg3
    pub rdi: u64, // offset  40 — arg1
    pub rsi: u64, // offset  48 — arg2
    pub rsp: u64, // offset  56 — saved userspace RSP
    pub r15: u64, // offset  64
    pub r14: u64, // offset  72
    pub r13: u64, // offset  80
    pub r12: u64, // offset  88
    pub rbx: u64, // offset  96
    pub rbp: u64, // offset 104
    pub r11: u64, // offset 112 — userspace RFLAGS (saved by SYSCALL hw)
    pub rcx: u64, // offset 120 — userspace return RIP (first push)
}

// ── SYSCALL/SYSRET Initialization ────────────────────────────────────────────

/// Configures MSRs for SYSCALL/SYSRET
///
/// Must be called on each CPU after GDT init.
pub fn init_syscall() {
    use msr::*;

    // 1. Enable SCE in EFER (SYSCALL Enable)
    // SAFETY: EFER.SCE is safe to enable once GDT is in place
    unsafe {
        set_msr_bits(MSR_IA32_EFER, EFER_SCE);
    }

    // 2. MSR STAR: segment selectors
    // Bits [47:32] = SYSCALL CS (kernel CS); SS = CS+8 = KERNEL_DS
    // Bits [63:48] = SYSRET CS32; SYSRET 64 CS = +16, SS = +8
    //
    // Note: STAR.SYSRET_CS points to USER_CS32 (ring 3).
    // SYSRET 64-bit will use USER_CS32+16 = USER_CS64 for CS
    // and USER_CS32+8 = USER_DS for SS.
    let star_kernel = (GDT_KERNEL_CS as u64) << 32;
    let star_user = ((GDT_USER_CS32 & !3) as u64) << 48;
    // SAFETY: STAR contains only valid GDT selectors
    unsafe {
        write_msr(MSR_STAR, star_kernel | star_user);
    }

    // 3. MSR LSTAR: 64-bit address of SYSCALL handler
    // SAFETY: syscall_entry_asm is the valid syscall entry point
    unsafe {
        write_msr(MSR_LSTAR, syscall_entry_asm as *const () as u64);
    }

    // 4. MSR CSTAR: compat mode handler (AMD-only, WRMSR #GP(0) on bare-metal Intel)
    if super::cpu::features::cpu_features().vendor == super::cpu::features::CpuVendor::Amd {
        // SAFETY: syscall_cstar_noop is a valid noop function
        unsafe {
            write_msr(MSR_CSTAR, syscall_cstar_noop as *const () as u64);
        }
    }

    // 5. MSR SFMASK: masks RFLAGS during SYSCALL
    // Mask IF, TF, DF, IOPL, NT, RF, VM, and AC before entering Ring 0 (conforms to Linux / SDM).
    let sfmask = (1 << 9)   // IF
               | (1 << 8)   // TF
               | (1 << 10)  // DF
               | (3 << 12)  // IOPL (bits 12-13)
               | (1 << 14)  // NT
               | (1 << 16)  // RF
               | (1 << 17)  // VM (bit 17)
               | (1 << 18); // AC
    // SAFETY: SFMASK masks dangerous flags only
    unsafe {
        write_msr(MSR_SFMASK, sfmask);
    }
}

// ── SYSCALL Entry in ASM ─────────────────────────────────────────────────────

const SYSRETQ_USER_CS64: u64 = super::gdt::GDT_USER_CS64 as u64;
const SYSRETQ_USER_DS: u64 = super::gdt::GDT_USER_DS as u64;
const SYSRETQ_SAFE_RSP: u64 = crate::memory::core::layout::USER_STACK_TOP.as_u64();

// 64-bit SYSCALL entry point — defined in global_asm!
//
// Sequence:
// 1. SWAPGS (kernel GS)
// 2. If KPTI active, load kernel CR3 from GS:[0x40]
// 3. Save userspace RSP in per-CPU area
// 4. Load kernel RSP0 from per-CPU area
// 5. Save all caller-saved registers + RCX/R11
// 6. Call `syscall_rust_handler()`
// 7. Restore registers from SyscallFrame
// 8. Check RCX/RSP before SYSRETQ (CVE-2012-0217)
// 9. If KPTI active, load user CR3 from GS:[0x48]
// 10. Load userspace RSP from SyscallFrame
// 11. SWAPGS (restore userspace GS)
// 12. SYSRETQ or fallback IRETQ
core::arch::global_asm!(
    ".section .text",
    ".global syscall_entry_asm",
    ".type   syscall_entry_asm, @function",
    "syscall_entry_asm:",
    // ── 1. SWAPGS: activate kernel GS (kernel GS_BASE ← KERNEL_GS_BASE) ───────
    "swapgs",
    // ── 1b. KPTI: switch to kernel CR3 before any kernel stack access ──────────
    // rax contains syscall number: save it in GS:[0x50].
    "mov   qword ptr gs:[0x50], rax",
    "mov   rax, qword ptr gs:[0x40]",
    "test  rax, rax",
    "jz    998f",
    "mov   cr3, rax",
    "998:",
    "mov   rax, qword ptr gs:[0x50]",
    // ── 2. Save userspace RSP in gs:[0x08] (user_rsp slot) ──────────────────────
    // gs:[0x00] = kernel_rsp (pre-allocated kernel stack)
    // gs:[0x08] = user_rsp  (save slot for current userspace RSP)
    "mov   qword ptr gs:[0x08], rsp",
    // ── 3. Load kernel stack from gs:[0x00] (kernel_rsp slot) ──────────────────
    "mov   rsp, qword ptr gs:[0x00]",
    // ── 4. Build SyscallFrame on kernel stack ───────────────────────────────────
    // Push order: first → high address, last → low address = [rsp]
    // MUST match SyscallFrame field order exactly
    "push  rcx",                 // [rsp+120] return RIP (SYSCALL saves in rcx)
    "push  r11",                 // [rsp+112] userspace RFLAGS (SYSCALL saves in r11)
    "push  rbp",                 // [rsp+104]
    "push  rbx",                 // [rsp+ 96] (rbx will be clobbered temporarily — see step 6)
    "push  r12",                 // [rsp+ 88]
    "push  r13",                 // [rsp+ 80]
    "push  r14",                 // [rsp+ 72]
    "push  r15",                 // [rsp+ 64]
    "push  qword ptr gs:[0x08]", // [rsp+ 56] Userspace RSP (read from save slot)
    "push  rsi",                 // [rsp+ 48] arg2
    "push  rdi",                 // [rsp+ 40] arg1
    "push  rdx",                 // [rsp+ 32] arg3
    "push  r10",                 // [rsp+ 24] arg4
    "push  r8",                  // [rsp+ 16] arg5
    "push  r9",                  // [rsp+  8] arg6
    "push  rax",                 // [rsp+  0] syscall number (frame.rax)
    // ── 5. Save frame pointer in rbx (callee-saved) ───────────────────────────
    // rbx is callee-saved: syscall_rust_handler will preserve it.
    // Value saved on frame (frame.rbx) is already on stack at [rsp+96].
    // rbx can thus be freely used as frame pointer until pop.
    "mov   rbx, rsp",
    // ── 6. Align stack to 16 bytes for AMD64 ABI ────────────────────────────────
    // After 16 pushes of 8 bytes = 128 bytes (multiple of 16), rsp is already aligned.
    // This instruction is a safeguard in case initial kernel_rsp was unaligned.
    "and   rsp, -16",
    // ── 7. Call Rust handler with SyscallFrame* as first argument ───────────────
    "mov   rdi, rbx",
    "call  syscall_rust_handler",
    // ── 8. Restore rsp to frame start (undoes possible alignment) ───────────────
    // rbx is preserved by syscall_rust_handler (callee-saved ABI)
    "mov   rsp, rbx",
    // ── 9. Restore registers directly from frame ─────────────────────────────────
    // Do not rebuild userspace RSP from gs:[0x08]: this slot is per-CPU and
    // can become stale in long paths. SyscallFrame is the single source of
    // truth for the pending return; RSP is loaded last.
    "mov   rax, [rsp +   0]",
    "mov   r9,  [rsp +   8]",
    "mov   r8,  [rsp +  16]",
    "mov   r10, [rsp +  24]",
    "mov   rdx, [rsp +  32]",
    "mov   rdi, [rsp +  40]",
    "mov   rsi, [rsp +  48]",
    "mov   r15, [rsp +  64]",
    "mov   r14, [rsp +  72]",
    "mov   r13, [rsp +  80]",
    "mov   r12, [rsp +  88]",
    "mov   rbx, [rsp +  96]",
    "mov   rbp, [rsp + 104]",
    "mov   r11, [rsp + 112]",
    "mov   rcx, [rsp + 120]",
    // ── 10. Prepare userspace RSP and verify SYSRETQ return ──────────────────────
    "mov   qword ptr gs:[0x50], rax",
    "mov   rax, [rsp +  56]",
    "mov   qword ptr gs:[0x08], rax",
    // CVE-2012-0217: SYSRETQ may raise #GP in CPL0 with non-canonical RCX/RSP.
    // Test canonicity via bit 47 sign-extension before leaving Ring 0.
    "mov   rax, rcx",
    "shl   rax, 16",
    "sar   rax, 16",
    "cmp   rax, rcx",
    "jne   997f",
    "mov   rax, qword ptr gs:[0x08]",
    "shl   rax, 16",
    "sar   rax, 16",
    "cmp   rax, qword ptr gs:[0x08]",
    "jne   997f",
    "mov   rax, qword ptr gs:[0x48]",
    "test  rax, rax",
    "jz    999f",
    "mov   cr3, rax",
    "999:",
    "mov   rax, qword ptr gs:[0x50]",
    "mov   rsp, qword ptr gs:[0x08]",
    // ── 11. Restore userspace GS ─────────────────────────────────────────────────
    "swapgs",
    // ── 12. Return to 64-bit Ring 3 mode ─────────────────────────────────────────
    "sysretq",
    "997:",
    // IRETQ Fallback: switch back to user CR3, then build canonical user frame
    // that faults in Ring 3 on RIP=0 instead of #GP in Ring 0.
    "mov   rax, qword ptr gs:[0x48]",
    "test  rax, rax",
    "jz    996f",
    "mov   cr3, rax",
    "996:",
    "sub   rsp, 40",
    "mov   qword ptr [rsp +  0], 0",
    "mov   qword ptr [rsp +  8], {user_cs}",
    "mov   qword ptr [rsp + 16], r11",
    "mov   rax, {safe_rsp}",
    "mov   qword ptr [rsp + 24], rax",
    "mov   qword ptr [rsp + 32], {user_ds}",
    "mov   rax, qword ptr gs:[0x50]",
    "swapgs",
    "iretq",
    ".size syscall_entry_asm, . - syscall_entry_asm",
    user_cs = const SYSRETQ_USER_CS64,
    user_ds = const SYSRETQ_USER_DS,
    safe_rsp = const SYSRETQ_SAFE_RSP,
);

// CSTAR noop for compat mode SYSCALL (unsupported)
core::arch::global_asm!(
    ".section .text",
    ".global syscall_cstar_noop",
    ".type   syscall_cstar_noop, @function",
    "syscall_cstar_noop:",
    // Close short Ring 0 window on userspace stack before stack switch.
    "cli",
    // Activate kernel GS to access per-CPU area.
    "swapgs",
    // KPTI: switch to kernel CR3 if slot is published.
    "mov rax, qword ptr gs:[0x40]",
    "test rax, rax",
    "jz 997f",
    "mov cr3, rax",
    "997:",
    // Save userspace RSP in gs:[0x08] (standard save slot).
    // RSP has NOT been changed yet (compat SYSCALL does not touch RSP).
    "mov qword ptr gs:[0x08], rsp",
    // Load kernel RSP from gs:[0x00] to avoid any work on user stack.
    "mov rsp, qword ptr gs:[0x00]",
    // Return -ENOSYS (errno 38 = ENOSYS in Linux ABI).
    "mov eax, -38",
    // KPTI: switch to user CR3 right before return.
    "mov qword ptr gs:[0x50], rax",
    "mov rax, qword ptr gs:[0x48]",
    "test rax, rax",
    "jz 996f",
    "mov cr3, rax",
    "996:",
    "mov rax, qword ptr gs:[0x50]",
    // Restore userspace RSP from save slot.
    "mov rsp, qword ptr gs:[0x08]",
    // Restore userspace GS.
    "swapgs",
    // 32-bit compat return: `sysret` without suffix emits compat variant,
    // unlike `sysretq` which forces 64-bit return and would cause #GP.
    "sysret",
    ".size syscall_cstar_noop, . - syscall_cstar_noop",
);

extern "C" {
    fn syscall_entry_asm();
    fn syscall_cstar_noop();
}

// ── x86_64 Canonical Address Check (RULE ARCH-SYSRET / V-35) ────────────────

/// Returns `true` if address is canonical (bits 63..47 identical).
/// A non-canonical address in RCX at SYSRET time causes a kernel fault
/// on certain Intel/AMD CPUs — intercepted here to deliver it to Ring 3.
#[inline(always)]
fn is_canonical(addr: u64) -> bool {
    let sign_bits = addr >> 47;
    sign_bits == 0 || sign_bits == 0x1FFFF
}

#[inline(always)]
fn is_user_return_addr(addr: u64) -> bool {
    is_canonical(addr) && addr < crate::memory::core::layout::USER_END.as_u64()
}

#[inline(always)]
unsafe fn write_saved_user_rsp(rsp: u64) {
    core::arch::asm!(
        "mov qword ptr gs:[0x08], {rsp}",
        rsp = in(reg) rsp,
        options(nostack, preserves_flags),
    );
}

// ── Rust SYSCALL Handler ─────────────────────────────────────────────────────

/// Syscall dispatch table
/// Indexed by syscall number, returns handler or default
#[allow(dead_code)]
type SyscallFn = fn(u64, u64, u64, u64, u64, u64) -> i64;

/// Handler called from ASM with SyscallFrame
///
/// # Safety
/// Called only from `syscall_entry_asm` with a valid `SyscallFrame`
/// on the kernel stack.
#[no_mangle]
pub unsafe extern "C" fn syscall_rust_handler(frame: *mut SyscallFrame) {
    // SAFETY: frame is a valid SyscallFrame built by the entry ASM
    let frame = &mut *frame;

    SYSCALL_COUNT.fetch_add(1, Ordering::Relaxed);

    // FIX-B-02 (Security_Audit_Pass2 §B-02): apply_ibrs() on syscall entry path.
    // On CPUs without Enhanced IBRS (EIBRS), IBRS must be enabled on each
    // kernel entry from Ring 3 to mitigate Spectre v2 "kernel-vs-user".
    // apply_ibrs() is idempotent (no-op if EIBRS active or IBRS disabled).
    // SAFETY: Ring 0 MSR write — guaranteed by syscall_entry_asm state.
    crate::arch::x86_64::spectre::apply_ibrs();

    // ── RULE SIGNAL-01 (DOC1) ─────────────────────────────────────────────────
    // The complete pipeline (fast-path → compat → table → signal delivery)
    // is managed by crate::syscall::dispatch::dispatch().
    // arch/ orchestrates only: frame is passed by raw pointer,
    // dispatch() reads arguments and writes frame.rax.
    // Signal delivery occurs in post_dispatch() before return.

    // SAFETY: frame originates from syscall_rust_handler which receives a
    // valid pointer from ASM. Lifetime is bounded to this stackframe.
    crate::syscall::dispatch::dispatch(frame);

    // ── ARCH-SYSRET (V-35): return address verification ────────────────────
    // Non-canonical addresses are left to ASM, which switches to IRETQ
    // to avoid CVE-2012-0217 Ring 0 #GP. Canonical addresses outside user
    // space are redirected to a controlled Ring 3 fault.
    if !is_user_return_addr(frame.rcx) {
        #[cfg(debug_assertions)]
        crate::arch::x86_64::terminal::debug_write(b"syscall: bad rcx\n");
        if is_canonical(frame.rcx) {
            frame.rcx = 0; // address 0 = unmapped → userspace #PF, not kernel
        }
    }
    if !is_user_return_addr(frame.rsp) {
        #[cfg(debug_assertions)]
        crate::arch::x86_64::terminal::debug_write(b"syscall: bad rsp\n");
        if is_canonical(frame.rsp) {
            frame.rsp = SYSRETQ_SAFE_RSP;
        }
    }
    // Keep per-CPU slot synchronized for compat/debug paths. Main return
    // restores RSP from SyscallFrame above.
    unsafe {
        write_saved_user_rsp(frame.rsp);
    }
}

// ── Syscall Instrumentation ──────────────────────────────────────────────────

static SYSCALL_COUNT: AtomicU64 = AtomicU64::new(0);
static SYSCALL_ERROR_COUNT: AtomicU64 = AtomicU64::new(0);

pub fn syscall_count() -> u64 {
    SYSCALL_COUNT.load(Ordering::Relaxed)
}
pub fn syscall_error_count() -> u64 {
    SYSCALL_ERROR_COUNT.load(Ordering::Relaxed)
}

pub fn record_syscall_error() {
    SYSCALL_ERROR_COUNT.fetch_add(1, Ordering::Relaxed);
}
