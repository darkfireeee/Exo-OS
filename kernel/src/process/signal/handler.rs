// kernel/src/process/signal/handler.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Construction of user signal frame (x86_64 System V) — Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// Frame is constructed on user stack (or sigaltstack if SA_ONSTACK).
// It contains complete context (ucontext_t) allowing sigreturn(2) to properly
// restore thread state after handler completion.
//
// Memory layout (growing upwards):
//   [rsp - sizeof(SignalFrame)]: SignalFrame
//      +0   : SigInfo      (siginfo_t, 128 bytes)
//      +128 : UContext     (ucontext_t, 936 bytes)
//      +... : restorer stub (8 bytes optional)

use super::default::SigAction;
use super::delivery::SyscallFrame;
use super::queue::SigInfo;
use super::tcb::SIGNAL_FRAME_MAGIC;
use crate::process::core::tcb::ProcessThread;
use core::sync::atomic::Ordering;

// ─────────────────────────────────────────────────────────────────────────────
// ABI structures
// ─────────────────────────────────────────────────────────────────────────────

/// x86_64 general purpose registers saved in ucontext_t.uc_mcontext.
#[derive(Copy, Clone, Default)]
#[repr(C)]
pub struct GRegs {
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub rdx: u64,
    pub rax: u64,
    pub rcx: u64,
    pub rsp: u64,
    pub rip: u64,
    pub eflags: u64,
    pub cs: u16,
    pub gs: u16,
    pub fs: u16,
    pub _pad: u16,
    pub err: u64,
    pub trapno: u64,
    pub oldmask: u64,
    pub cr2: u64,
}

/// Stack state for ucontext.
#[derive(Copy, Clone, Default)]
#[repr(C)]
pub struct SigAltStack {
    pub ss_sp: u64,
    pub ss_flags: i32,
    pub _pad: u32,
    pub ss_size: u64,
}

pub const SS_ONSTACK: i32 = 1;
pub const SS_DISABLE: i32 = 2;

/// Equivalent of ucontext_t (Linux x86_64).
///
/// FIX-SIGALIGN: no `align(16)` on this structure. `_fpregs_mem` is
/// currently just a zeroed reserved space (no FXSAVE/XRSTOR). An
/// `align(16)` here propagates to `SignalFrame` (nested field) and would make
/// `ptr.write` at `sig_rsp` ≡8 mod16 UB — exactly the SysV placement that
/// `setup_signal_frame` must deliver. If FXSAVE/XRSTOR is hooked up, align
/// that field specifically, not the entire structure.
#[derive(Copy, Clone)]
#[repr(C)]
pub struct UContext {
    pub uc_flags: u64,
    pub uc_link: u64, // Pointer to parent ucontext (optional)
    pub uc_stack: SigAltStack,
    pub uc_mcontext: GRegs,
    pub uc_sigmask: u64,
    pub uc_fs_base: u64,
    pub uc_gs_base: u64,
    pub _fpregs_mem: [u8; 512], // Space for FXSAVE
}

impl Default for UContext {
    fn default() -> Self {
        // SAFETY: all fields are numeric types → valid zeroes.
        unsafe { core::mem::zeroed() }
    }
}

/// Full frame placed on user stack before calling handler.
///
/// FIX-SIGALIGN (bisect OOM/GP run 12/13, `<GP1 rip=... rsp mod16=8>`):
/// no `align(16)` here nor on `UContext` (where `_fpregs_mem` lives). See
/// `setup_signal_frame`: handler entry RSP must be ≡8 mod16.
#[derive(Copy, Clone)]
#[repr(C)]
pub struct SignalFrame {
    /// Return address: points to sigreturn routine (restorer).
    pub pretcode: u64,
    /// Signal number (rdi for handler(int signo)).
    pub signo: u64,
    /// Pointer to si_info (rsi for handler(int, siginfo_t*, void*)).
    pub pinfo: u64,
    /// Pointer to uc (rdx for SA_SIGINFO handler).
    pub puc: u64,
    /// siginfo_t.
    pub info: SigInfoC,
    /// ucontext_t.
    pub uc: UContext,
}

/// C-compatible version of SigInfo (128 bytes, siginfo_t compatible).
#[derive(Copy, Clone)]
#[repr(C)]
pub struct SigInfoC {
    pub si_signo: i32,
    pub si_errno: i32,
    pub si_code: i32,
    pub _pad1: i32,
    pub si_pid: u32,
    pub si_uid: u32,
    pub si_value_int: i32,
    pub _pad2: i32,
    pub si_value_ptr: u64,
    pub _rest: [u8; 88],
}

impl Default for SigInfoC {
    fn default() -> Self {
        // SAFETY: all fields are numeric types → valid zeroes.
        unsafe { core::mem::zeroed() }
    }
}

impl SigInfoC {
    fn from_queue(info: &SigInfo) -> Self {
        Self {
            si_signo: info.signo as i32,
            si_code: info.code,
            si_pid: info.sender_pid,
            si_uid: info.sender_uid,
            si_value_int: info.value_int,
            si_value_ptr: info.value_ptr,
            ..Default::default()
        }
    }
}

// Total size of signal frame: must be <= 4096 to fit in a page.
const _: () = assert!(
    core::mem::size_of::<SignalFrame>() <= 4096,
    "SignalFrame doit tenir dans une page utilisateur"
);

/// Offset of `uc` (UContext) field in `SignalFrame`, in bytes.
///
/// Layout: pretcode(8) + signo(8) + pinfo(8) + puc(8) + info(SigInfoC=128) = 160.
/// Used by `dispatch.rs` to compute `uc_ptr` during sigreturn.
pub const SIGNAL_FRAME_UC_OFFSET: u64 = 160;

/// Registers restored from UContext on sigreturn.
///
/// Returned by `verify_and_extract_uc()` after verifying magic SIG-13.
#[derive(Copy, Clone, Default)]
pub struct UContextRegs {
    pub rip: u64,
    pub rsp: u64,
    pub rax: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rbx: u64,
    pub rbp: u64,
    pub rflags: u64,
    pub fs_base: u64,
    pub gs_base: u64,
    pub signal_mask: u64,
}

/// Verifies magic SIG-13 and extracts registers from userspace UContext.
///
/// ## Security (LAC-01 / SIG-13 / SIG-14)
/// - CONSTANT-TIME: all data is read BEFORE verifying magic.
///   No path allows a timing oracle on magic validity.
/// - Returns `None` if address is invalid or if `uc_flags != SIGNAL_FRAME_MAGIC`.
///
/// # Safety
/// `uc_ptr` must be a valid userspace address (verified internally).
pub fn verify_and_extract_uc(uc_ptr: u64) -> Option<UContextRegs> {
    const USER_SPACE_TOP: u64 = 0x0000_7FFF_FFFF_F000;
    if !(0x1000..USER_SPACE_TOP).contains(&uc_ptr) {
        return None;
    }
    // SAFETY: userspace address validated above.
    let uc = unsafe { &*(uc_ptr as *const UContext) };
    let mc = &uc.uc_mcontext;

    // Extract ALL data before checking magic (LAC-01 constant-time).
    let regs = UContextRegs {
        rip: mc.rip,
        rsp: mc.rsp,
        rax: mc.rax,
        rdi: mc.rdi,
        rsi: mc.rsi,
        rdx: mc.rdx,
        rcx: mc.rcx,
        r8: mc.r8,
        r9: mc.r9,
        r10: mc.r10,
        r12: mc.r12,
        r13: mc.r13,
        r14: mc.r14,
        r15: mc.r15,
        rbx: mc.rbx,
        rbp: mc.rbp,
        rflags: mc.eflags,
        fs_base: uc.uc_fs_base,
        gs_base: uc.uc_gs_base,
        signal_mask: uc.uc_sigmask,
    };

    // Constant-time magic verification: XOR then test (no predictor leak).
    // SIG-13 / SIG-14: if magic is wrong, sigreturn is rejected.
    let magic_diff = uc.uc_flags ^ (SIGNAL_FRAME_MAGIC as u64);
    if magic_diff != 0 {
        return None;
    }

    Some(regs)
}

// ─────────────────────────────────────────────────────────────────────────────
// setup_signal_frame — builds frame on user stack
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C, align(16))]
#[allow(dead_code)]
struct AlignedFpRegs([u8; 512]);

#[cfg(target_os = "none")]
#[inline]
fn save_fpu_regs_to_bytes() -> [u8; 512] {
    let mut fp = AlignedFpRegs([0u8; 512]);
    // SAFETY: fp is aligned on 16 bytes (required by fxsave64).
    unsafe {
        core::arch::asm!(
            "fxsave64 [{}]",
            in(reg) fp.0.as_mut_ptr(),
            options(nostack, preserves_flags)
        );
    }
    fp.0
}

#[cfg(not(target_os = "none"))]
#[inline]
fn save_fpu_regs_to_bytes() -> [u8; 512] {
    [0u8; 512]
}

#[cfg(target_os = "none")]
#[inline]
fn restore_fpu_regs_from_bytes(src: &[u8; 512]) {
    let mut fp = AlignedFpRegs(*src);
    // Mask reserved bits of MXCSR (bytes 24..28) to prohibit forged #GP(0) from Ring 3.
    let mxcsr = u32::from_le_bytes([fp.0[24], fp.0[25], fp.0[26], fp.0[27]]) & 0x0000_FFBF;
    fp.0[24..28].copy_from_slice(&mxcsr.to_le_bytes());
    // SAFETY: fp is aligned on 16 bytes and MXCSR is sanitized.
    unsafe {
        core::arch::asm!(
            "fxrstor64 [{}]",
            in(reg) fp.0.as_ptr(),
            options(nostack, preserves_flags)
        );
    }
}

#[cfg(not(target_os = "none"))]
#[inline]
fn restore_fpu_regs_from_bytes(_src: &[u8; 512]) {}

/// Constructs a SignalFrame on user stack and redirects RIP to
/// handler. Called from delivery::deliver_one() only.
///
/// Sequence:
/// 1. Choose stack (sigaltstack if SA_ONSTACK, otherwise current stack).
/// 2. Align RSP to 16 bytes (-sizeof(SignalFrame)) according to System V ABI.
/// 3. Write SignalFrame (info + ucontext with current registers).
/// 4. Modify frame->user_rip = handler, frame->user_rsp = &SignalFrame.
/// 5. Configure RDI/RSI/RDX for SA_SIGINFO convention.
pub fn setup_signal_frame(
    thread: &mut ProcessThread,
    frame: &mut SyscallFrame,
    sig_n: u8,
    info: &SigInfo,
    action: &SigAction,
) {
    const USER_SPACE_TOP: u64 = 0x0000_7FFF_FFFF_F000;
    let sig_tcb_opt = crate::process::core::registry::PROCESS_REGISTRY
        .find_by_pid(crate::process::core::pid::Pid(thread.sched_tcb.pid.0))
        .and_then(|pcb| {
            let vaddr = pcb.signal_tcb_vaddr.load(Ordering::Acquire);
            if (0x1000..USER_SPACE_TOP).contains(&vaddr) {
                Some(unsafe { &*(vaddr as *const super::tcb::SignalTcb) })
            } else {
                None
            }
        });

    // Choose target stack.
    let target_rsp = if action.flags & SigAction::SA_ONSTACK != 0 {
        let alt = thread.addresses.sigaltstack_top();
        if alt != 0 {
            alt
        } else if let Some(stcb) = sig_tcb_opt {
            if stcb.has_altstack() {
                stcb.altstack_sp
                    .load(Ordering::Acquire)
                    .saturating_add(stcb.altstack_size.load(Ordering::Acquire))
            } else {
                frame.user_rsp
            }
        } else {
            frame.user_rsp
        }
    } else {
        frame.user_rsp
    };

    // Align according to SysV: (rsp - frame_size) & ~0xF, then -8 (for red zone).
    //
    // FIX-SIGALIGN (bisect OOM/GP run 12/13): this comment already described the
    // proper algorithm, but the "-8" was never applied — `sig_rsp` came out
    // ≡0 mod16 and was used as is as entry RSP for handler
    // (`frame.user_rsp = sig_rsp` below). But SysV ABI expects RSP ≡8 mod16
    // at entry of function reached "as if by call" (what a signal handler
    // simulates): compiler of user binary emits a
    // prologue assuming this invariant (e.g. `movaps [rsp+N], xmmM`, which
    // is only correct 16-aligned). With previous calculation, this prologue saw
    // incorrect RSP by 8 bytes and `movaps` raised #GP — exactly the crash
    // observed (`<GP1 ... rsp=...b8>`, rsp mod16=8 instead of 0 expected by
    // compiled code once its own push was done).
    let frame_size = core::mem::size_of::<SignalFrame>() as u64;
    let sig_rsp = ((target_rsp - frame_size) & !0xFu64) - 8;

    // Build frame contents in a temporary buffer.
    // Ensure that sig_rsp is a valid user address (< USER_SPACE_TOP).
    if !(0x1000..USER_SPACE_TOP).contains(&sig_rsp) {
        // Corrupted user stack: force SIGSEGV.
        // In prod, set thread to SIGSEGV and proceed to Core action.
        return;
    }

    let old_mask = {
        let k_mask = thread.sched_tcb.signal_mask.load(Ordering::Acquire);
        let u_mask = sig_tcb_opt.map(|s| s.get_mask()).unwrap_or(0);
        k_mask | u_mask
    };

    // Build SignalFrame.
    let sig_frame = SignalFrame {
        pretcode: action.restorer,
        signo: sig_n as u64,
        pinfo: sig_rsp + offset_of_info(),
        puc: sig_rsp + offset_of_uc(),
        info: SigInfoC::from_queue(info),
        uc: UContext {
            // SIG-13: store SIGNAL_FRAME_MAGIC in uc_flags for verification on sigreturn.
            uc_flags: SIGNAL_FRAME_MAGIC as u64,
            uc_link: 0,
            uc_stack: SigAltStack {
                ss_sp: thread
                    .addresses
                    .sigaltstack_top()
                    .saturating_sub(thread.addresses.stack_size),
                ss_flags: if action.flags & SigAction::SA_ONSTACK != 0 {
                    SS_ONSTACK
                } else {
                    SS_DISABLE
                },
                ss_size: thread.addresses.stack_size,
                _pad: 0,
            },
            uc_mcontext: GRegs {
                rip: frame.user_rip,
                rsp: frame.user_rsp,
                rax: frame.user_rax,
                rdi: frame.user_rdi,
                rsi: frame.user_rsi,
                rdx: frame.user_rdx,
                rcx: frame.user_rcx,
                r8: frame.user_r8,
                r9: frame.user_r9,
                r10: frame.user_r10,
                r12: frame.user_r12,
                r13: frame.user_r13,
                r14: frame.user_r14,
                r15: frame.user_r15,
                rbx: frame.user_rbx,
                rbp: frame.user_rbp,
                cs: frame.user_cs as u16,
                eflags: frame.user_rflags,
                ..Default::default()
            },
            uc_sigmask: old_mask,
            uc_fs_base: thread.sched_tcb.fs_base,
            uc_gs_base: thread.sched_tcb.user_gs_base,
            _fpregs_mem: save_fpu_regs_to_bytes(),
        },
    };

    // Write frame onto user stack.
    // SAFETY: sig_rsp points to validated userspace address above; write = POSIX signal delivery.
    unsafe {
        let ptr = sig_rsp as *mut SignalFrame;
        ptr.write(sig_frame);
    }

    // Update mask during handler execution (SA_NODEFER excludes sig_n).
    let mut new_mask = old_mask | action.mask;
    if action.flags & SigAction::SA_NODEFER == 0 {
        new_mask |= 1u64 << (sig_n - 1); // block current signal
    }
    use super::mask::SigMask;
    let safe_new_mask = SigMask::from(new_mask).0;
    thread
        .sched_tcb
        .signal_mask
        .store(safe_new_mask, Ordering::Release);
    if let Some(stcb) = sig_tcb_opt {
        stcb.set_mask(safe_new_mask);
        stcb.in_handler.store(1, Ordering::Release);
    }

    // Redirect syscall return to handler.
    frame.user_rip = action.handler;
    frame.user_rsp = sig_rsp;
    // SA_SIGINFO convention (rdi = signo, rsi = *siginfo, rdx = *ucontext).
    frame.user_rdi = sig_n as u64;
    frame.user_rsi = sig_rsp + offset_of_info();
    frame.user_rdx = sig_rsp + offset_of_uc();
}

/// Restores context after sigreturn(2).
/// `uc_ptr` = address of UContext passed by user in rdi.
/// Modifies `frame` to restore saved registers.
pub fn restore_signal_frame(thread: &mut ProcessThread, frame: &mut SyscallFrame, uc_ptr: u64) {
    const USER_SPACE_TOP: u64 = 0x0000_7FFF_FFFF_F000;
    if !(0x1000..USER_SPACE_TOP).contains(&uc_ptr) {
        return;
    }

    // SAFETY: uc_ptr was written by setup_signal_frame; reread it.
    let uc = unsafe { &*(uc_ptr as *const UContext) };
    let mc = &uc.uc_mcontext;

    // SIG-13 (constant-time): extract data BEFORE checking magic.
    let rip_saved = mc.rip;
    let rsp_saved = mc.rsp;
    let rax_saved = mc.rax;
    let rdi_saved = mc.rdi;
    let rsi_saved = mc.rsi;
    let rdx_saved = mc.rdx;
    let rcx_saved = mc.rcx;
    let r8_saved = mc.r8;
    let r9_saved = mc.r9;
    let r10_saved = mc.r10;
    let r12_saved = mc.r12;
    let r13_saved = mc.r13;
    let r14_saved = mc.r14;
    let r15_saved = mc.r15;
    let rbx_saved = mc.rbx;
    let rbp_saved = mc.rbp;
    let rflags_saved = mc.eflags;
    let fs_base_saved = uc.uc_fs_base;
    let gs_base_saved = uc.uc_gs_base;
    let mask_saved = uc.uc_sigmask;
    let fpregs_saved = uc._fpregs_mem;

    // Constant-time magic verification (SIG-13 / SIG-14).
    // If magic is invalid, restore nothing (sigframe attack).
    let magic_diff = uc.uc_flags ^ (SIGNAL_FRAME_MAGIC as u64);
    if magic_diff != 0 {
        return;
    }

    restore_fpu_regs_from_bytes(&fpregs_saved);

    // Restore registers.
    frame.user_rip = rip_saved;
    frame.user_rsp = rsp_saved;
    frame.user_rax = rax_saved;
    frame.user_rdi = rdi_saved;
    frame.user_rsi = rsi_saved;
    frame.user_rdx = rdx_saved;
    frame.user_rcx = rcx_saved;
    frame.user_r8 = r8_saved;
    frame.user_r9 = r9_saved;
    frame.user_r10 = r10_saved;
    frame.user_r12 = r12_saved;
    frame.user_r13 = r13_saved;
    frame.user_r14 = r14_saved;
    frame.user_r15 = r15_saved;
    frame.user_rbx = rbx_saved;
    frame.user_rbp = rbp_saved;
    const RFLAGS_SAFE_MASK: u64 = 0x0000_0000_0024_0CD5;
    const RFLAGS_FORCE_SET: u64 = 0x0000_0000_0000_0200;
    const RFLAGS_FORCE_CLR: u64 = 0x0000_0000_0003_7100;
    frame.user_rflags = ((rflags_saved & RFLAGS_SAFE_MASK) | RFLAGS_FORCE_SET) & !RFLAGS_FORCE_CLR;
    frame.user_fs_base = fs_base_saved;
    frame.user_gs_base = gs_base_saved;
    thread.sched_tcb.fs_base = fs_base_saved;
    thread.sched_tcb.user_gs_base = gs_base_saved;
    thread.tls_gs_base.store(fs_base_saved, Ordering::Release);

    // Restore saved signal mask (without SIGKILL/SIGSTOP).
    use super::mask::SigMask;
    let restored_mask = SigMask::from(mask_saved);
    thread
        .sched_tcb
        .signal_mask
        .store(restored_mask.0, Ordering::Release);
    if let Some(pcb) = crate::process::core::registry::PROCESS_REGISTRY
        .find_by_pid(crate::process::core::pid::Pid(thread.sched_tcb.pid.0))
    {
        let vaddr = pcb.signal_tcb_vaddr.load(Ordering::Acquire);
        if (0x1000..USER_SPACE_TOP).contains(&vaddr) {
            let stcb = unsafe { &*(vaddr as *const super::tcb::SignalTcb) };
            stcb.set_mask(restored_mask.0);
            stcb.in_handler.store(0, Ordering::Release);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Const offsets in SignalFrame
// ─────────────────────────────────────────────────────────────────────────────

#[inline(always)]
fn offset_of_info() -> u64 {
    // SignalFrame { pretcode(8), signo(8), pinfo(8), puc(8), info(128), uc(...) }
    32u64
}

#[inline(always)]
fn offset_of_uc() -> u64 {
    32u64 + core::mem::size_of::<SigInfoC>() as u64
}
