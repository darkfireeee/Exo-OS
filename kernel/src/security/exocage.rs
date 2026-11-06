// kernel/src/security/exocage.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ExoCage — CET Shadow Stack + IBT with TCB integration (ExoShield v1.0)
// ═══════════════════════════════════════════════════════════════════════════════
//
// ExoCage is the Layer 1 module of ExoShield — it guarantees control flow
// integrity via Intel CET (hardware Shadow Stack + Indirect Branch Tracking).
//
// Architecture:
//   • Shadow Stack: hardware copy of return addresses (CET Shadow Stack)
//   • IBT: every indirect branch target must begin with ENDBR64
//   • Mandatory token at top of shadow stack (Intel CET §3.4)
//   • #CP Handler: any violation = confirmed compromise = immediate HANDOFF
//
// TCB INTEGRATION GI-01:
//   Layout _cold_reserve (TCB absolute offset -> relative offset _cold_reserve)
//   TCB offset 144 -> _cold_reserve[0..7]   : shadow_stack_token : u64
//   TCB offset 152 -> _cold_reserve[8]      : cet_flags          : u8
//   TCB offset 153 -> _cold_reserve[9]      : threat_score_u8    : u8
//   TCB offset 160 -> _cold_reserve[16..23] : pt_buffer_phys     : u64
//   TCB offset 231 -> _cold_reserve[87]     : last byte (88 bytes total)
//   RULE: index 0 of _cold_reserve corresponds to TCB absolute offset 144.
//
// ABSOLUTE CONSTRAINT: size_of::<TCB>() == 256 bytes — ZERO impact on hardcoded offsets.
//
// REFERENCES:
//   Intel SDM Vol.1 §18.3 (CET Overview)
//   Intel SDM Vol.3 §2.7 (Shadow Stack)
//   ExoShield_v1_Production.md — MODULE 3: ExoCage
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::arch::x86_64::cpu::msr;
use crate::memory::core::{AllocFlags, Frame, PhysAddr};
use crate::memory::physical::allocator::buddy;
use crate::scheduler::core::task::ThreadControlBlock;

// ─────────────────────────────────────────────────────────────────────────────
// CET MSR constants (reference: Intel SDM Vol.4 §2)
// ─────────────────────────────────────────────────────────────────────────────

/// MSR IA32_U_CET — User-mode CET control.
const MSR_IA32_U_CET: u32 = 0x6A0;
/// MSR IA32_S_CET — Supervisor-mode CET control.
const MSR_IA32_S_CET: u32 = 0x6A2;
/// MSR IA32_PL0_SSP — Ring 0 Shadow Stack Pointer.
const MSR_IA32_PL0_SSP: u32 = 0x6A4;
/// MSR IA32_PL1_SSP — Ring 1 Shadow Stack Pointer.
const MSR_IA32_PL1_SSP: u32 = 0x6A5;
/// MSR IA32_PL2_SSP — Ring 2 Shadow Stack Pointer.
#[allow(dead_code)]
const MSR_IA32_PL2_SSP: u32 = 0x6A6;
/// MSR IA32_PL3_SSP — Ring 3 Shadow Stack Pointer (userspace).
#[allow(dead_code)]
const MSR_IA32_PL3_SSP: u32 = 0x6A7;
/// MSR IA32_INTERRUPT_SSP_TABLE — SSP table for interrupt shadow stack.
#[allow(dead_code)]
const MSR_IA32_INTERRUPT_SSP_TABLE: u32 = 0x6A8;

// ─────────────────────────────────────────────────────────────────────────────
// CET control bits (MSR IA32_S_CET / IA32_U_CET)
// ─────────────────────────────────────────────────────────────────────────────

/// Bit 0 — Shadow Stack Enable.
const CET_SHSTK_EN: u64 = 1 << 0;
/// Bit 1 — WRSS instruction enable (required for kernel shadow stack management).
const CET_WR_SHSTK_EN: u64 = 1 << 1;
/// Bit 2 — ENDBRANCH enforcement (IBT).
const CET_ENDBR_EN: u64 = 1 << 2;
/// Bit 3 — Legacy Shadow Stack compatibility (IBT only when set).
#[allow(dead_code)]
const CET_LEG_IW_EN: u64 = 1 << 3;
/// Bit 4 — No Track for indirect CALL/JMP (suppress IBT when set).
#[allow(dead_code)]
const CET_NO_TRACK_EN: u64 = 1 << 4;
/// Bit 5 — Suppress Shadow Stack error on WRSS.
#[allow(dead_code)]
const CET_SUPPRESS_DIS: u64 = 1 << 5;

// ─────────────────────────────────────────────────────────────────────────────
// CR4.CET (bit 23) — Feature Enable
// ─────────────────────────────────────────────────────────────────────────────

const CR4_CET_BIT: u64 = 1 << 23;

// ─────────────────────────────────────────────────────────────────────────────
// TCB _cold_reserve flags — cet_flags (offset [152])
// ─────────────────────────────────────────────────────────────────────────────

/// Bit 0: CET Shadow Stack enabled for this thread.
pub const CET_FLAG_ENABLED: u8 = 1 << 0;
/// Bit 1: IBT enabled for this thread.
pub const CET_FLAG_IBT: u8 = 1 << 1;
/// Bit 2: Shadow stack token validated at setup.
pub const CET_FLAG_TOKEN_VALID: u8 = 1 << 2;

// ─────────────────────────────────────────────────────────────────────────────
// Per-thread Shadow Stack size
// ─────────────────────────────────────────────────────────────────────────────

/// Number of pages allocated for each thread's shadow stack (4 pages = 16 KiB).
const SHADOW_STACK_PAGES: usize = 4;
/// Page size (4 KiB).
const PAGE_SIZE: u64 = 4096;

// ─────────────────────────────────────────────────────────────────────────────
// Global ExoCage state
// ─────────────────────────────────────────────────────────────────────────────

/// CET Shadow Stack enabled globally (ExoSeal step 0).
static EXOCAGE_GLOBAL_ENABLED: AtomicBool = AtomicBool::new(false);

/// IBT enabled globally (ExoSeal step 0).
static EXOCAGE_IBT_ENABLED: AtomicBool = AtomicBool::new(false);

/// #CP violation counter (cumulative).
static CP_VIOLATION_COUNT: AtomicU64 = AtomicU64::new(0);

/// Counter of threads with active CET.
static CET_THREAD_COUNT: AtomicU64 = AtomicU64::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// ExoCage errors
// ─────────────────────────────────────────────────────────────────────────────

/// Possible errors during per-thread CET enablement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExoCageError {
    /// CET not supported by CPU.
    NotSupported,
    /// CET not enabled globally (ExoSeal step 0 not executed).
    NotGloballyEnabled,
    /// Unable to allocate shadow stack pages.
    AllocFailed,
    /// Shadow stack token is invalid after write.
    TokenVerificationFailed,
}

// ─────────────────────────────────────────────────────────────────────────────
// CPUID CET — hardware support verification
// ─────────────────────────────────────────────────────────────────────────────

/// Verifies CET support via CPUID.7.0.
///
/// Returns `(shadow_stack_supported, ibt_supported)`.
///
/// - CPUID.07H.0H:ECX bit 7 = CET Shadow Stack
/// - CPUID.07H.0H:EDX bit 20 = IBT (Indirect Branch Tracking)
#[inline]
pub fn cpuid_cet_available() -> (bool, bool) {
    let ecx: u32;
    let edx: u32;
    // SAFETY: validated low-level operation — see block documentation.
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 7",
            "xor ecx, ecx",
            "cpuid",
            "pop rbx",
            out("ecx") ecx,
            out("edx") edx,
            lateout("eax") _,
        );
    }
    let ss = ecx & (1 << 7) != 0;
    let ibt = edx & (1 << 20) != 0;
    (ss, ibt)
}

// ─────────────────────────────────────────────────────────────────────────────
// TCB _cold_reserve helpers — secure accesses to ExoShield offsets
// ─────────────────────────────────────────────────────────────────────────────

/// Writes a u64 into TCB _cold_reserve at specified offset.
///
/// # Safety
/// Offset + 8 must not exceed _cold_reserve size (88 bytes).
/// Only documented ExoShield offsets are permitted.
#[inline(always)]
// SAFETY: validated low-level operation — see block documentation.
unsafe fn tcb_write_cold_u64(tcb: &mut ThreadControlBlock, offset: usize, val: u64) {
    assert!(
        offset + 8 <= 88,
        "PATCH-P1-DEBUG: TCB _cold_reserve write out of bounds: offset={}",
        offset
    ); // promis debug_assert → assert (release visible)
    let base = tcb._cold_reserve.as_ptr() as *const u8 as *mut u8;
    core::ptr::write_volatile(base.add(offset) as *mut u64, val);
}

/// Reads a u64 from TCB _cold_reserve at specified offset.
#[inline(always)]
// SAFETY: validated low-level operation — see block documentation.
unsafe fn tcb_read_cold_u64(tcb: &ThreadControlBlock, offset: usize) -> u64 {
    assert!(
        offset + 8 <= 88,
        "PATCH-P1-DEBUG: TCB _cold_reserve read out of bounds: offset={}",
        offset
    ); // promis debug_assert → assert (release visible)
    let base = tcb._cold_reserve.as_ptr() as *const u8;
    core::ptr::read_volatile(base.add(offset) as *const u64)
}

/// Writes a u8 into TCB _cold_reserve at specified offset.
#[inline(always)]
// SAFETY: validated low-level operation — see block documentation.
unsafe fn tcb_write_cold_u8(tcb: &mut ThreadControlBlock, offset: usize, val: u8) {
    assert!(
        offset < 88,
        "PATCH-P1-DEBUG: TCB _cold_reserve write out of bounds: offset={}",
        offset
    ); // promis debug_assert → assert (release visible)
    let base = tcb._cold_reserve.as_ptr() as *const u8 as *mut u8;
    core::ptr::write_volatile(base.add(offset), val);
}

/// Reads a u8 from TCB _cold_reserve at specified offset.
#[inline(always)]
// SAFETY: validated low-level operation — see block documentation.
unsafe fn tcb_read_cold_u8(tcb: &ThreadControlBlock, offset: usize) -> u8 {
    assert!(
        offset < 88,
        "PATCH-P1-DEBUG: TCB _cold_reserve read out of bounds: offset={}",
        offset
    ); // promis debug_assert → assert (release visible)
    let base = tcb._cold_reserve.as_ptr() as *const u8;
    core::ptr::read_volatile(base.add(offset))
}

// ─────────────────────────────────────────────────────────────────────────────
// ExoShield offsets in _cold_reserve (relative to [144])
// ─────────────────────────────────────────────────────────────────────────────

/// Offset of shadow_stack_token in _cold_reserve (= [144] - [144] = 0).
const OFF_SHADOW_STACK_TOKEN: usize = 0; // [144] u64
/// Offset of cet_flags in _cold_reserve (= [152] - [144] = 8).
const OFF_CET_FLAGS: usize = 8; // [152] u8
/// Offset of threat_score_u8 in _cold_reserve (= [153] - [144] = 9).
const OFF_THREAT_SCORE: usize = 9; // [153] u8
/// Offset of pt_buffer_phys in _cold_reserve (= [160] - [144] = 16).
const OFF_PT_BUFFER_PHYS: usize = 16; // [160] u64
const HANDOFF_FREEZE_REQ: u64 = 1;

// ─────────────────────────────────────────────────────────────────────────────
// Shadow Stack allocation
// ─────────────────────────────────────────────────────────────────────────────

/// Allocates `count` contiguous physical pages for the shadow stack.
///
/// Returns physical base address, or 0 if allocation fails.
///
#[inline]
fn alloc_shadow_stack_pages(count: usize) -> u64 {
    if count == 0 {
        return 0;
    }

    let mut order = 0usize;
    while (1usize << order) < count {
        order += 1;
    }

    match buddy::alloc_pages(order, AllocFlags::ZEROED | AllocFlags::PIN) {
        Ok(frame) => frame.start_address().as_u64(),
        Err(_) => 0,
    }
}

/// Frees shadow stack pages of a thread (called upon thread termination).
#[inline]
fn free_shadow_stack_pages(base: u64, count: usize) {
    if base == 0 || count == 0 {
        return;
    }

    let mut order = 0usize;
    while (1usize << order) < count {
        order += 1;
    }

    let frame = Frame::containing(PhysAddr::new(base));
    let _ = buddy::free_pages(frame, order);
}

// ─────────────────────────────────────────────────────────────────────────────
// enable_cet_for_thread — Per-thread CET enablement
// ─────────────────────────────────────────────────────────────────────────────

/// Enables CET Shadow Stack + IBT for a specific thread.
///
/// This function configures:
/// 1. SSP MSRs (PL0/PL1) to point to the allocated shadow stack
/// 2. Mandatory token at the top of the shadow stack (Intel CET §3.4)
/// 3. Flags in TCB `_cold_reserve[144..154]`
///
/// # Exact sequence (ExoShield spec MODULE 3)
///
/// ```text
/// 1. CPUID check (CET_SS available)
/// 2. Allocate 4 shadow stack pages (bit 63 PTE = shadow stack marker)
/// 3. Write mandatory token at top (busy bit = 1)
/// 4. WRMSR IA32_PL0_SSP (0x6A4) = token_addr
/// 5. WRMSR IA32_PL1_SSP (0x6A5) = token_addr
/// 6. Save token in TCB _cold_reserve[144]
/// 7. Save cet_flags in TCB _cold_reserve[152]
/// ```
///
/// # Safety
/// - Must be called from Ring 0 only
/// - TCB must be properly aligned (64 bytes, repr(C))
/// - CET must be globally enabled (CR4.CET + IA32_S_CET) by ExoSeal
/// - Thread must NOT be executing on another CPU
///
/// # Constraints
/// - `size_of::<TCB>()` remains 256 bytes — writes solely in `_cold_reserve[144..232]`
/// - Hardcoded offsets unchanged: kstack@8, cr3@56, fpu@232, rq@240/248
// SAFETY: validated low-level operation — see block documentation.
pub unsafe fn enable_cet_for_thread(tcb: &mut ThreadControlBlock) -> Result<(), ExoCageError> {
    // 1. CPUID check
    let (ss_ok, ibt_ok) = cpuid_cet_available();
    if !ss_ok {
        return Err(ExoCageError::NotSupported);
    }

    // 2. Verify that CET is globally enabled (ExoSeal step 0)
    if !EXOCAGE_GLOBAL_ENABLED.load(Ordering::Acquire) {
        return Err(ExoCageError::NotGloballyEnabled);
    }

    if tcb_read_cold_u8(tcb, OFF_CET_FLAGS) & CET_FLAG_ENABLED != 0 {
        return Ok(());
    }

    // 3. Allocate 4 pages for shadow stack
    let ss_base = alloc_shadow_stack_pages(SHADOW_STACK_PAGES);
    if ss_base == 0 {
        return Err(ExoCageError::AllocFailed);
    }

    let ss_top = ss_base + (SHADOW_STACK_PAGES as u64) * PAGE_SIZE;

    // 4. MANDATORY token at top (Intel CET Spec §3.4)
    //    Prevents SROP (Sigreturn-Oriented Programming)
    //    Format: (token_addr & ~0x7) | busy_bit (bit 0 = 1)
    let token_addr = ss_top - 8;
    let token_val = (token_addr & !0x7_u64) | 0x1; // busy bit = 1

    // Token write via WRSSQ (only authorized instruction on shadow stack)
    // SAFETY: token_addr points to an allocated PTE shadow stack page above.
    //         CR4.CET and IA32_S_CET.WR_SHSTK_EN are active (ExoSeal step 0).
    core::arch::asm!(
        ".byte 0xF3, 0x48, 0x0F, 0x01, 0x3E",  // WRSSQ [rdi], rsi
        in("rdi") token_addr as *mut u64,
        in("rsi") token_val,
        options(nostack, preserves_flags),
    );

    // 5. Verify written token (volatile read to confirm)
    let readback = core::ptr::read_volatile(token_addr as *const u64);
    if readback != token_val {
        return Err(ExoCageError::TokenVerificationFailed);
    }

    // 6. Configure Ring 0 and Ring 1 SSP MSRs
    //    PL0 = Ring 0 (kernel), PL1 = Ring 1 (servers)
    //    SAFETY: existing MSRs, CET globally enabled, Ring 0 only.
    msr::write_msr(MSR_IA32_PL0_SSP, token_addr as u64);
    msr::write_msr(MSR_IA32_PL1_SSP, token_addr as u64);
    tcb.set_pl0_ssp(token_addr as u64);

    // 7. Save into TCB _cold_reserve
    //    [144] shadow_stack_token : u64
    //    [152] cet_flags          : u8
    //    [153] threat_score_u8    : u8 (initialized to 0)
    tcb_write_cold_u64(tcb, OFF_SHADOW_STACK_TOKEN, token_val);

    let mut cet_flags: u8 = CET_FLAG_ENABLED | CET_FLAG_TOKEN_VALID;
    if ibt_ok {
        cet_flags |= CET_FLAG_IBT;
    }
    tcb_write_cold_u8(tcb, OFF_CET_FLAGS, cet_flags);
    tcb_write_cold_u8(tcb, OFF_THREAT_SCORE, 0); // initial score = 0

    // pt_buffer_phys initialised to zero — LBR/PT hardware tracing
    // is not yet active for this thread. Will be populated by
    // ExoArgos when LBR is configured (Phase 3.2+ per ExoShield spec).
    tcb_write_cold_u64(tcb, OFF_PT_BUFFER_PHYS, 0);

    CET_THREAD_COUNT.fetch_add(1, Ordering::Relaxed);

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// disable_cet_for_thread — Deactivation upon thread death
// ─────────────────────────────────────────────────────────────────────────────

/// Disables CET for a thread and frees its resources.
///
/// Called from `do_exit()` or thread cleanup.
///
/// # Safety
/// Thread must no longer be schedulable (state == Dead).
// SAFETY: validated low-level operation — see block documentation.
pub unsafe fn disable_cet_for_thread(tcb: &mut ThreadControlBlock) {
    let cet_flags = tcb_read_cold_u8(tcb, OFF_CET_FLAGS);
    if cet_flags & CET_FLAG_ENABLED == 0 {
        return; // CET never enabled for this thread
    }

    // Retrieve token to calculate shadow stack base
    let token_val = tcb_read_cold_u64(tcb, OFF_SHADOW_STACK_TOKEN);
    if token_val != 0 {
        // Token contains address (token_val & ~0x7) at top of SS
        let token_addr = token_val & !0x7_u64;
        let ss_base = token_addr + 8 - (SHADOW_STACK_PAGES as u64) * PAGE_SIZE;
        free_shadow_stack_pages(ss_base, SHADOW_STACK_PAGES);
    }

    // Clear ExoShield fields in TCB
    tcb.set_pl0_ssp(0);
    tcb_write_cold_u64(tcb, OFF_SHADOW_STACK_TOKEN, 0);
    tcb_write_cold_u8(tcb, OFF_CET_FLAGS, 0);
    tcb_write_cold_u8(tcb, OFF_THREAT_SCORE, 0);
    tcb_write_cold_u64(tcb, OFF_PT_BUFFER_PHYS, 0);

    CET_THREAD_COUNT.fetch_sub(1, Ordering::Relaxed);
}

// ─────────────────────────────────────────────────────────────────────────────
// Handler #CP — Control Protection Exception (vector 21)
// ─────────────────────────────────────────────────────────────────────────────

/// #CP exception handler (Control Protection).
///
/// Any #CP violation = confirmed compromise = IMMEDIATE HANDOFF to ExoPhoenix.
///
/// # #CP error code (error_code)
///
/// | Bits   | Meaning                                     |
/// |--------|---------------------------------------------|
/// | [5:0]  | Type: 1=SHADOW_STACK, 2=IBT, 3=FPES         |
/// | [31:6] | Reserved (0)                                |
/// | [63:32]| Page-addr hint (shadow stack violations)    |
///
/// # ExoShield Spec
/// "Any #CP triggered and logged on any CET violation.
///  Immediate handoff — no progressive scoring."
///
/// # Safety
/// This function is an interrupt handler — it must never return
/// normally. It triggers a HANDOFF to Kernel B.
pub extern "C" fn cp_handler(_frame: usize, error_code: u64) {
    // 1. Increment global violation counter
    CP_VIOLATION_COUNT.fetch_add(1, Ordering::Relaxed);

    // 2. Log in ExoLedger P0 zone (non-overwritable)
    //    Note: exo_ledger_append_p0() is ISR-safe (lock-free, zero allocation)
    crate::security::exoledger::exo_ledger_append_p0(
        crate::security::exoledger::ActionTag::CpViolation { error_code },
    );

    // 3. Immediate HANDOFF to ExoPhoenix Kernel B
    //    Write to SSR.HANDOFF_FLAG — triggers Kernel A freeze
    //    via IPI from Kernel B.
    //
    //    SAFETY: SSR is an E820 reserved memory region, accessible in Ring 0.
    //            Write is atomic (single u64 write).
    //
    //    Note: ssr_write_atomic will be hooked up to existing ExoPhoenix
    //          infrastructure (kernel/src/exophoenix/ssr.rs).
    // SAFETY: validated low-level operation — see block documentation.
    unsafe {
        crate::exophoenix::ssr::ssr_atomic(crate::exophoenix::ssr::SSR_HANDOFF_FLAG)
            .store(HANDOFF_FREEZE_REQ, Ordering::Release);
    }

    // 4. Never return — HANDOFF will freeze this core
    //    While awaiting Kernel B IPI, we spin
    loop {
        core::hint::spin_loop();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Global ExoCage enablement (called by ExoSeal in step 0)
// ─────────────────────────────────────────────────────────────────────────────

/// Enables CET Shadow Stack + IBT globally (ExoSeal step 0).
///
/// This function is called EXACTLY ONCE by Kernel B before Kernel A
/// starts. It configures:
/// - CR4.CET (bit 23) = 1
/// - IA32_S_CET: SHSTK_EN | WR_SHSTK_EN | ENDBR_EN
/// - Sets global ExoCage flags
///
/// # Safety
/// - Must be called from Ring 0 on Core 0 (Kernel B / ExoSeal)
/// - Must be called BEFORE Kernel A starts (step 0)
/// - No other CPU must be active during this call
// SAFETY: validated low-level operation — see block documentation.
pub unsafe fn exocage_global_enable() -> Result<(), ExoCageError> {
    let (ss_ok, ibt_ok) = cpuid_cet_available();
    if !ss_ok {
        return Err(ExoCageError::NotSupported);
    }

    // 1. Enable CR4.CET (bit 23)
    let cr4: u64;
    core::arch::asm!("mov {}, cr4", out(reg) cr4);
    let cr4_new = cr4 | CR4_CET_BIT;
    core::arch::asm!("mov cr4, {}", in(reg) cr4_new);

    // 2. Configure IA32_S_CET: Shadow Stack + WRSS + IBT
    let mut s_cet_val = CET_SHSTK_EN | CET_WR_SHSTK_EN;
    if ibt_ok {
        s_cet_val |= CET_ENDBR_EN;
    }
    msr::write_msr(MSR_IA32_S_CET, s_cet_val);

    // 3. Enable IA32_U_CET for future userspace threads
    let mut u_cet_val = CET_SHSTK_EN;
    if ibt_ok {
        u_cet_val |= CET_ENDBR_EN;
    }
    msr::write_msr(MSR_IA32_U_CET, u_cet_val);

    // 4. Set global flags
    EXOCAGE_GLOBAL_ENABLED.store(true, Ordering::Release);
    if ibt_ok {
        EXOCAGE_IBT_ENABLED.store(true, Ordering::Release);
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Public accessors
// ─────────────────────────────────────────────────────────────────────────────

/// Returns true if CET Shadow Stack is globally enabled.
#[inline(always)]
pub fn is_cet_global_enabled() -> bool {
    EXOCAGE_GLOBAL_ENABLED.load(Ordering::Acquire)
}

/// Returns true if IBT is globally enabled.
#[inline(always)]
pub fn is_ibt_global_enabled() -> bool {
    EXOCAGE_IBT_ENABLED.load(Ordering::Acquire)
}

/// Reads shadow_stack_token of a TCB.
///
/// # Safety
/// TCB must be valid and aligned. Read-only, ISR-safe.
#[inline(always)]
// SAFETY: validated low-level operation — see block documentation.
pub unsafe fn get_shadow_stack_token(tcb: &ThreadControlBlock) -> u64 {
    tcb_read_cold_u64(tcb, OFF_SHADOW_STACK_TOKEN)
}

/// Reads cet_flags of a TCB.
#[inline(always)]
// SAFETY: validated low-level operation — see block documentation.
pub unsafe fn get_cet_flags(tcb: &ThreadControlBlock) -> u8 {
    tcb_read_cold_u8(tcb, OFF_CET_FLAGS)
}

/// Reads threat_score of a TCB.
#[inline(always)]
// SAFETY: validated low-level operation — see block documentation.
pub unsafe fn get_threat_score(tcb: &ThreadControlBlock) -> u8 {
    tcb_read_cold_u8(tcb, OFF_THREAT_SCORE)
}

/// Writes threat_score of a TCB.
///
/// # Safety
/// TCB must be valid. Thread must not be executing on another CPU
/// (exclusive access).
#[inline(always)]
// SAFETY: validated low-level operation — see block documentation.
pub unsafe fn set_threat_score(tcb: &mut ThreadControlBlock, score: u8) {
    debug_assert!(score <= 100, "threat_score must be 0..=100");
    tcb_write_cold_u8(tcb, OFF_THREAT_SCORE, score.min(100));
}

/// Total number of #CP violations since boot.
#[inline(always)]
pub fn cp_violation_count() -> u64 {
    CP_VIOLATION_COUNT.load(Ordering::Relaxed)
}

/// Number of threads with active CET.
#[inline(always)]
pub fn cet_thread_count() -> u64 {
    CET_THREAD_COUNT.load(Ordering::Relaxed)
}

// ─────────────────────────────────────────────────────────────────────────────
// ExoCage statistics
// ─────────────────────────────────────────────────────────────────────────────

/// Snapshot of ExoCage statistics.
#[derive(Debug, Clone, Copy)]
pub struct ExoCageStats {
    /// CET Shadow Stack enabled globally.
    pub global_enabled: bool,
    /// IBT enabled globally.
    pub ibt_enabled: bool,
    /// Number of threads with active CET.
    pub thread_count: u64,
    /// Total number of #CP violations.
    pub cp_violations: u64,
}

/// Returns a snapshot of ExoCage statistics.
pub fn exocage_stats() -> ExoCageStats {
    ExoCageStats {
        global_enabled: EXOCAGE_GLOBAL_ENABLED.load(Ordering::Relaxed),
        ibt_enabled: EXOCAGE_IBT_ENABLED.load(Ordering::Relaxed),
        thread_count: CET_THREAD_COUNT.load(Ordering::Relaxed),
        cp_violations: CP_VIOLATION_COUNT.load(Ordering::Relaxed),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CET validation for context_switch
// ─────────────────────────────────────────────────────────────────────────────

/// Verifies that TCB shadow stack token is valid prior to a context switch.
///
/// Called from `switch_context()` to validate that the next thread
/// has a valid CET token (ROP/JOP prevention).
///
/// Returns `true` if thread has CET enabled AND a valid token.
#[inline(always)]
pub fn validate_thread_cet(tcb: &ThreadControlBlock) -> bool {
    // SAFETY: read-only access to _cold_reserve, no modification.
    unsafe {
        let flags = tcb_read_cold_u8(tcb, OFF_CET_FLAGS);
        if flags & CET_FLAG_ENABLED == 0 {
            return true; // CET not enabled for this thread — acceptable (kthreads, etc.)
        }
        // CET enabled: verify that token is present
        let token = tcb_read_cold_u64(tcb, OFF_SHADOW_STACK_TOKEN);
        token != 0 && (token & 0x1) != 0 // busy bit must be 1
    }
}
