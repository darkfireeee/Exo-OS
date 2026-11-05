//! # cpu/msr.rs — Model-Specific Registers x86_64
//!
//! Reading and writing MSRs via RDMSR / WRMSR.
//! MSR constants referenced by other modules are grouped here.
//!
//! ## SAFETY RULE
//! These functions are `unsafe` — the caller guarantees that the MSR exists
//! on the target CPU and that the written value violates no invariants.

use core::sync::atomic::Ordering;

// ── MSR Constants ────────────────────────────────────────────────────────────

/// MSR IA32_APIC_BASE: LAPIC base address + enable/x2apic flags
pub const MSR_IA32_APIC_BASE: u32 = 0x0000_001B;

/// MSR IA32_EFER: Extended Feature Enable Register (LME, NXE, SCE…)
pub const MSR_IA32_EFER: u32 = 0xC000_0080;

/// NXE bit in EFER (No-Execute Enable)
pub const EFER_NXE: u64 = 1 << 11;

/// SCE bit in EFER (SysCall Enable)
pub const EFER_SCE: u64 = 1 << 0;

/// LME bit in EFER (Long Mode Enable)
pub const EFER_LME: u64 = 1 << 8;

/// LMA bit in EFER (Long Mode Active)
pub const EFER_LMA: u64 = 1 << 10;

/// MSR STAR: CS/SS segments for SYSCALL/SYSRET
pub const MSR_STAR: u32 = 0xC000_0081;

/// MSR LSTAR: 64-bit address of SYSCALL handler
pub const MSR_LSTAR: u32 = 0xC000_0082;

/// MSR CSTAR: compat (32-bit) SYSCALL handler address
pub const MSR_CSTAR: u32 = 0xC000_0083;

/// MSR SFMASK: RFLAGS mask applied during a SYSCALL
pub const MSR_SFMASK: u32 = 0xC000_0084;

/// MSR GS_BASE: base of GS segment (kernel)
pub const MSR_GS_BASE: u32 = 0xC000_0101;

/// MSR KERNEL_GS_BASE: shadow GS base (userspace)
pub const MSR_KERNEL_GS_BASE: u32 = 0xC000_0102;

/// MSR FS_BASE: base of FS segment (userspace TLS)
pub const MSR_FS_BASE: u32 = 0xC000_0100;

/// MSR TSC_AUX: auxiliary value returned by RDTSCP (logical CPU ID)
pub const MSR_TSC_AUX: u32 = 0xC000_0103;

/// MSR IA32_TSC_DEADLINE: LAPIC TSC deadline timer
pub const MSR_TSC_DEADLINE: u32 = 0x0000_06E0;

/// MSR IA32_PKRS: Supervisor Protection Keys Rights for User pages
pub const MSR_IA32_PKRS: u32 = 0x0000_06E1;

/// MSR IA32_PL0_SSP — Ring 0 Shadow Stack Pointer (CET, Intel SDM Vol.4 §2.1)
/// Contains current thread SSP in Ring 0 mode.
/// Must be saved/restored on each context switch if CET-SS is active.
/// FIX-CET-01
pub const MSR_IA32_PL0_SSP: u32 = 0x0000_06A4;

/// MSR IA32_PMC0..7: performance counters
pub const MSR_IA32_PMC0: u32 = 0x0000_00C1;

/// MSR IA32_PERFEVTSEL0: PMU event selection
pub const MSR_IA32_PERFEVTSEL0: u32 = 0x0000_0186;

/// MSR IA32_FIXED_CTR0: fixed cycle counter
pub const MSR_IA32_FIXED_CTR0: u32 = 0x0000_0309;

/// MSR IA32_FIXED_CTR_CTRL: fixed counters control
pub const MSR_IA32_FIXED_CTR_CTRL: u32 = 0x0000_038D;

/// MSR IA32_PERF_GLOBAL_CTRL: PMU global enable
pub const MSR_IA32_PERF_GLOBAL_CTRL: u32 = 0x0000_038F;

/// MSR IA32_SPEC_CTRL: IBRS/STIBP/SSBD control
pub const MSR_IA32_SPEC_CTRL: u32 = 0x0000_0048;

/// IBRS bit in SPEC_CTRL
pub const SPEC_CTRL_IBRS: u64 = 1 << 0;

/// STIBP bit in SPEC_CTRL
pub const SPEC_CTRL_STIBP: u64 = 1 << 1;

/// SSBD bit in SPEC_CTRL
pub const SPEC_CTRL_SSBD: u64 = 1 << 2;

/// MSR IA32_PRED_CMD: prediction commands (IBPB flush)
pub const MSR_IA32_PRED_CMD: u32 = 0x0000_0049;

/// IBPB bit in PRED_CMD
pub const PRED_CMD_IBPB: u64 = 1 << 0;

/// MSR IA32_FLUSH_CMD: flush L1D cache (MDS)
pub const MSR_IA32_FLUSH_CMD: u32 = 0x0000_010B;

/// L1D_FLUSH bit in FLUSH_CMD
pub const FLUSH_CMD_L1D: u64 = 1 << 0;

/// MSR IA32_ARCH_CAP: architectural capabilities (RDCL_NO, IBRS_ALL…)
pub const MSR_IA32_ARCH_CAP: u32 = 0x0000_010A;

/// RDCL_NO bit: CPU not vulnerable to Meltdown
pub const ARCH_CAP_RDCL_NO: u64 = 1 << 0;

/// IBRS_ALL bit: IBRS operates in "always on" mode
pub const ARCH_CAP_IBRS_ALL: u64 = 1 << 1;

/// RSBA bit: RSB alternative prediction possible
pub const ARCH_CAP_RSBA: u64 = 1 << 2;

/// SSB_NO bit: CPU not vulnerable to Spectre v4
pub const ARCH_CAP_SSB_NO: u64 = 1 << 4;

/// MSR IA32_TSC_ADJUST: TSC adjustment (VM migration)
pub const MSR_IA32_TSC_ADJUST: u32 = 0x0000_003B;

/// MSR IA32_MTRR_DEF_TYPE: default memory type
pub const MSR_IA32_MTRR_DEF_TYPE: u32 = 0x0000_02FF;

/// MSR IA32_PAT: Page Attribute Table
pub const MSR_IA32_PAT: u32 = 0x0000_0277;

/// Default PAT value (WB, WT, UC-, UC, WB, WT, UC-, UC)
pub const PAT_DEFAULT: u64 = 0x0007_0406_0007_0406;

// ── Read/Write Functions ─────────────────────────────────────────────

/// Reads a 64-bit MSR
///
/// # SAFETY
/// The caller guarantees:
/// - The current CPU supports this MSR (checked via CPUID if necessary)
/// - This MSR is readable from Ring 0
/// - No #GP exception will be triggered
#[inline(always)]
pub unsafe fn read_msr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: delegated to caller; rdmsr is Ring 0 privileged
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx")  msr,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

/// Writes a 64-bit MSR
///
/// # SAFETY
/// The caller guarantees:
/// - The current CPU supports writing this MSR
/// - The value `val` respects documented constraints of the MSR
/// - This MSR is write-accessible from Ring 0
#[inline(always)]
pub unsafe fn write_msr(msr: u32, val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    // SAFETY: delegated to caller; wrmsr is Ring 0 privileged
    unsafe {
        core::arch::asm!(
            "wrmsr",
            in("ecx")  msr,
            in("eax")  lo,
            in("edx")  hi,
            options(nostack, nomem)
        );
    }
}

/// Reads an MSR and applies an AND mask (bit reading)
///
/// # SAFETY
/// Same requirements as `read_msr`.
#[inline(always)]
pub unsafe fn read_msr_bits(msr: u32, mask: u64) -> u64 {
    // SAFETY: delegated to caller
    unsafe { read_msr(msr) & mask }
}

/// Modifies bits in an MSR (read-modify-write, atomic from CPU perspective)
///
/// # SAFETY
/// Same requirements as `write_msr` and `read_msr`.
/// Caller must ensure modification is consistent with system state.
#[inline(always)]
pub unsafe fn set_msr_bits(msr: u32, bits: u64) {
    // SAFETY: delegated to caller
    unsafe {
        let val = read_msr(msr);
        write_msr(msr, val | bits);
    }
}

/// Clears bits in an MSR
///
/// # SAFETY
/// Same requirements as `set_msr_bits`.
#[inline(always)]
pub unsafe fn clear_msr_bits(msr: u32, bits: u64) {
    // SAFETY: delegated to caller
    unsafe {
        let val = read_msr(msr);
        write_msr(msr, val & !bits);
    }
}

// ── SWAPGS ────────────────────────────────────────────────────────────────────

/// Swaps GS_BASE with KERNEL_GS_BASE (SYSCALL entry / exit to userspace)
///
/// # SAFETY
/// Must be executed in pairs. A SWAPGS without its matching counterpart corrupts GS
/// and causes a per-CPU access to an invalid address.
#[inline(always)]
pub unsafe fn swapgs() {
    // SAFETY: delegated to caller — must be used in pairs
    unsafe {
        core::arch::asm!("swapgs", options(nostack, nomem));
    }
}

// ── RDTSCP ───────────────────────────────────────────────────────────────────

/// Reads TSC + logical CPU ID atomically (RDTSCP)
///
/// Returns `(tsc_value, cpu_id_aux)`.
/// `cpu_id_aux` is configured by the kernel in MSR_TSC_AUX.
///
/// # SAFETY
/// Requires CPUID.80000001H:EDX[27] = 1 (RDTSCP supported).
/// Use `CpuFeatures::has_rdtscp()` before calling.
#[inline(always)]
pub unsafe fn rdtscp() -> (u64, u32) {
    let lo: u32;
    let hi: u32;
    let aux: u32;
    // SAFETY: delegated to caller — RDTSCP is a lightweight serializing instruction
    unsafe {
        core::arch::asm!(
            "rdtscp",
            out("eax") lo,
            out("edx") hi,
            out("ecx") aux,
            options(nostack, nomem)
        );
    }
    (((hi as u64) << 32) | (lo as u64), aux)
}

// ── PAT Configuration ────────────────────────────────────────────────────────

/// Configures Page Attribute Table with default Exo-OS values
///
/// PAT index:
///   0: WB  (Write-Back)  — normal default
///   1: WT  (Write-Through)
///   2: UC- (Uncacheable-minus, ignored if MTRR=UC)
///   3: UC  (Uncacheable strict)
///   4: WB  (redundant — for compatibility)
///   5: WT
///   6: UC-
///   7: WC  (Write-Combining — for framebuffer)
pub fn configure_pat() {
    // SAFETY: MSR_IA32_PAT is supported on all modern x86_64 CPUs.
    // The PAT_WC value replaces entry 7 with Write-Combining (0x01).
    const PAT_WB: u64 = 0x06;
    const PAT_WT: u64 = 0x04;
    const PAT_UCM: u64 = 0x07;
    const PAT_UC: u64 = 0x00;
    const PAT_WC: u64 = 0x01;

    let pat = PAT_WB
        | (PAT_WT << 8)
        | (PAT_UCM << 16)
        | (PAT_UC << 24)
        | (PAT_WB << 32)
        | (PAT_WT << 40)
        | (PAT_UCM << 48)
        | (PAT_WC << 56);

    // SAFETY: PAT supported on all x86_64; CPU-local, no TLB shootdown necessary.
    unsafe {
        write_msr(MSR_IA32_PAT, pat);
    }
}

// ── MSR Statistics ─────────────────────────────────────────────────────────

static MSR_READ_COUNT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static MSR_WRITE_COUNT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

#[cfg(feature = "msr_stats")]
#[inline(always)]
fn increment_read() {
    MSR_READ_COUNT.fetch_add(1, Ordering::Relaxed);
}

#[cfg(feature = "msr_stats")]
#[inline(always)]
fn increment_write() {
    MSR_WRITE_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Returns `(reads, writes)` since boot
pub fn msr_stats() -> (u64, u64) {
    (
        MSR_READ_COUNT.load(Ordering::Relaxed),
        MSR_WRITE_COUNT.load(Ordering::Relaxed),
    )
}
