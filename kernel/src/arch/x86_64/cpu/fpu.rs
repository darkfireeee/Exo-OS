//! # cpu/fpu.rs — Raw ASM XSAVE/XRSTOR/FXSAVE Instructions
//!
//! ⚠️ This module contains ONLY hardware FPU instructions.
//!    State logic (when to save, lazy_fpu_used flag, etc.)
//!    is in `scheduler/fpu/` — DO NOT duplicate here.
//!
//! ## Implementation
//! - `fxsave` / `fxrstor` : 512-byte fallback (FXSAVE area)
//! - `xsave`  / `xrstor`  : Full XSAVE (size detected via CPUID)
//! - `xsaveopt`           : Optimized (saves only dirty components)
//! - `xsavec`             : Compacted (saves only active components)

use core::sync::atomic::{AtomicU32, Ordering};

// ── XSAVE Constants ─────────────────────────────────────────────────────────

/// Supported XSAVE components (XCR0 bits)
pub const XSAVE_X87: u64 = 1 << 0; // x87 FPU
pub const XSAVE_SSE: u64 = 1 << 1; // XMM0–15
pub const XSAVE_AVX: u64 = 1 << 2; // YMM0–15 (upper 128)
pub const XSAVE_MPX_BNDREGS: u64 = 1 << 3; // BND0–3 (MPX)
pub const XSAVE_MPX_BNDCSR: u64 = 1 << 4; // BNDCFGU / BNDSTATUS
pub const XSAVE_AVX512_K: u64 = 1 << 5; // K0–7 opmask
pub const XSAVE_AVX512_ZMM_HI: u64 = 1 << 6; // ZMM0–15 upper 256
pub const XSAVE_AVX512_HI16: u64 = 1 << 7; // ZMM16–31
pub const XSAVE_PT: u64 = 1 << 8; // Intel PT
pub const XSAVE_PKRU: u64 = 1 << 9; // Protection Keys

/// Minimal strict historical mask — x87 + SSE only.
pub const XSAVE_MASK_X87_SSE_ONLY: u64 = XSAVE_X87 | XSAVE_SSE;

/// Minimal Exo-OS XSAVE mask — x87 + SSE + AVX.
///
/// This mask describes the context contract when AVX is available. CPU
/// init keeps the CPUID/XCR0 check prior to actually enabling AVX.
pub const XSAVE_MASK_MINIMAL: u64 = XSAVE_X87 | XSAVE_SSE | XSAVE_AVX;

/// Complete XSAVE mask — everything supported by the OS
pub const XSAVE_MASK_ALL: u64 = 0xFFFF_FFFF_FFFF_FFFFu64;

/// Fixed FXSAVE area size
pub const FXSAVE_AREA_SIZE: usize = 512;

/// Maximum XSAVE area size (AVX-512 + PT + PKRU)
pub const XSAVE_AREA_MAX: usize = 2696;

/// Alignment required for FXSAVE/XSAVE area
pub const FPU_AREA_ALIGN: usize = 64;

// ── Runtime-Determined XSAVE Size ───────────────────────────────────────

static XSAVE_AREA_SIZE_RUNTIME: AtomicU32 = AtomicU32::new(512);

/// Returns the XSAVE area size detected at boot
#[inline(always)]
pub fn xsave_area_size() -> usize {
    XSAVE_AREA_SIZE_RUNTIME.load(Ordering::Relaxed) as usize
}

/// Configures runtime XSAVE size (called from early_init)
pub fn set_xsave_area_size(size: u32) {
    XSAVE_AREA_SIZE_RUNTIME.store(size, Ordering::Release);
}

// ── Static Alignment for FPU Area ────────────────────────────────────────

/// 64-byte aligned FPU state structure (FXSAVE or XSAVE area)
/// The `data` field is oversized to cover all cases.
/// The kernel allocator must allocate with 64-byte alignment.
#[repr(C, align(64))]
pub struct FpuRawArea {
    pub data: [u8; XSAVE_AREA_MAX],
}

impl FpuRawArea {
    pub const fn zeroed() -> Self {
        Self {
            data: [0u8; XSAVE_AREA_MAX],
        }
    }
}

// ── FXSAVE / FXRSTOR ─────────────────────────────────────────────────────────

/// Saves FPU/SSE state via FXSAVE (512 bytes)
///
/// # SAFETY
/// - `dst` must be aligned to at least 16 bytes (64 recommended)
/// - `dst` must point to at least 512 valid, accessible bytes
/// - The CPU must support FXSR (CPUID.EDX[24]) — check prior to call
#[inline(always)]
pub unsafe fn fxsave(dst: *mut u8) {
    // SAFETY: delegated to caller — dst aligned and valid
    unsafe {
        core::arch::asm!(
            "fxsave64 [{dst}]",
            dst = in(reg) dst,
            options(nostack)
        );
    }
}

/// Restores FPU/SSE state from an FXSAVE area
///
/// # SAFETY
/// - `src` must be a valid previously saved FXSAVE area
/// - Minimum 16-byte alignment required
#[inline(always)]
pub unsafe fn fxrstor(src: *const u8) {
    // SAFETY: delegated to caller — src is a valid fxsave area
    unsafe {
        core::arch::asm!(
            "fxrstor64 [{src}]",
            src = in(reg) src,
            options(nostack)
        );
    }
}

// ── XSAVE / XRSTOR ───────────────────────────────────────────────────────────

/// Saves extended FPU state via XSAVE
///
/// Saves all components whose bit is 1 in `rfbm` (Request Feature BitMap).
/// Use `XSAVE_MASK_ALL` to save everything.
///
/// # SAFETY
/// - `dst` must be aligned to 64 bytes
/// - `dst` must point to at least `xsave_area_size()` valid bytes
/// - CPU must support XSAVE (CPUID.ECX[26])
/// - OSXSAVE must be enabled in CR4 (CR4.OSXSAVE = 1)
#[inline(always)]
pub unsafe fn xsave(dst: *mut u8, rfbm: u64) {
    let rfbm_lo = rfbm as u32;
    let rfbm_hi = (rfbm >> 32) as u32;
    // SAFETY: delegated to caller
    unsafe {
        core::arch::asm!(
            "xsave64 [{dst}]",
            dst  = in(reg) dst,
            in("eax") rfbm_lo,
            in("edx") rfbm_hi,
            options(nostack)
        );
    }
}

/// Restores extended FPU state via XRSTOR
///
/// # SAFETY
/// - `src` must be a valid XSAVE area saved by `xsave()`
/// - 64-byte alignment required
/// - The header component (offset 512) must be consistent
#[inline(always)]
pub unsafe fn xrstor(src: *const u8, rfbm: u64) {
    let rfbm_lo = rfbm as u32;
    let rfbm_hi = (rfbm >> 32) as u32;
    // SAFETY: delegated to caller
    unsafe {
        core::arch::asm!(
            "xrstor64 [{src}]",
            src  = in(reg) src,
            in("eax") rfbm_lo,
            in("edx") rfbm_hi,
            options(nostack)
        );
    }
}

/// Saves via XSAVEOPT (saves only modified components)
///
/// Faster than XSAVE if few components were modified since the last restore.
///
/// # SAFETY
/// Same requirements as `xsave()`. Additionally, CPU must support XSAVEOPT
/// (CPUID leaf 0xD subleaf 1, EAX[0]).
#[inline(always)]
pub unsafe fn xsaveopt(dst: *mut u8, rfbm: u64) {
    let rfbm_lo = rfbm as u32;
    let rfbm_hi = (rfbm >> 32) as u32;
    // SAFETY: delegated to caller
    unsafe {
        core::arch::asm!(
            "xsaveopt64 [{dst}]",
            dst  = in(reg) dst,
            in("eax") rfbm_lo,
            in("edx") rfbm_hi,
            options(nostack)
        );
    }
}

/// Saves via XSAVEC (compact format — saves only active components)
///
/// # SAFETY
/// Same as `xsaveopt`. CPU must support XSAVEC (CPUID leaf 0xD subleaf 1, EAX[1]).
#[inline(always)]
pub unsafe fn xsavec(dst: *mut u8, rfbm: u64) {
    let rfbm_lo = rfbm as u32;
    let rfbm_hi = (rfbm >> 32) as u32;
    // SAFETY: delegated to caller
    unsafe {
        core::arch::asm!(
            "xsavec64 [{dst}]",
            dst  = in(reg) dst,
            in("eax") rfbm_lo,
            in("edx") rfbm_hi,
            options(nostack)
        );
    }
}

// ── XCR0 Read/Write ─────────────────────────────────────────────────────

/// Reads XCR0 (XSAVE Extended Control Register 0)
///
/// # SAFETY
/// Requires active OSXSAVE in CR4. Executable only from Ring 0.
#[inline(always)]
pub unsafe fn read_xcr0() -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: delegated to caller
    unsafe {
        core::arch::asm!(
            "xgetbv",
            in("ecx") 0u32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

/// Writes XCR0 (enables XSAVE components to be saved)
///
/// # SAFETY
/// - Requires OSXSAVE in CR4
/// - Unsupported bits in xcr0 will cause a #GP
/// - Do not disable X87 (bit 0) or SSE (bit 1) once enabled
#[inline(always)]
pub unsafe fn write_xcr0(val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    // SAFETY: delegated to caller
    unsafe {
        core::arch::asm!(
            "xsetbv",
            in("ecx") 0u32,
            in("eax") lo,
            in("edx") hi,
            options(nostack, nomem)
        );
    }
}

// ── FPU Initialization at Boot ────────────────────────────────────────────────

/// Initializes the FPU on the current CPU
///
/// - Enables CR0.MP, clears CR0.EM
/// - Enables CR4.OSFXSR + CR4.OSXMMEXCPT
/// - Enables CR4.OSXSAVE if XSAVE is available
/// - Configures XCR0 with x87 + SSE (+ AVX if available)
/// - Executes FINIT to clean up x87 state
pub fn init_fpu_for_cpu() {
    let mut cr0_raw: u64;
    // SAFETY: CR0 read — no side effects
    unsafe {
        core::arch::asm!("mov {}, cr0", out(reg) cr0_raw, options(nostack, nomem));
    }

    // CR0.MP = 1 (Monitor co-processor)
    // CR0.EM = 0 (no FPU emulation)
    // CR0.NE = 1 (native FP exceptions)
    // CR0.TS = 0 (Task Switched — lazy FPU will set it to 1 after switch)
    cr0_raw |= (1 << 1) | (1 << 5); // MP, NE
    cr0_raw &= !((1 << 2) | (1 << 3)); // clear EM, TS
                                       // SAFETY: consistent CR0 modification with valid FPU state
    unsafe {
        core::arch::asm!("mov cr0, {}", in(reg) cr0_raw, options(nostack, nomem));
    }

    // CR4: enable OSFXSR and OSXMMEXCPT
    let mut cr4 = super::super::read_cr4();
    cr4 |= (1 << 9)  // OSFXSR
         | (1 << 10); // OSXMMEXCPT

    let features = &super::features::CPU_FEATURES;

    if features.has_xsave() {
        cr4 |= 1 << 18; // OSXSAVE
    }

    // SAFETY: enabling OSFXSR/OSXMMEXCPT/OSXSAVE — required for SSE/XSAVE
    unsafe {
        super::super::write_cr4(cr4);
    }

    // Configure XCR0 if XSAVE is available
    if features.has_xsave() {
        let mut xcr0 = XSAVE_X87 | XSAVE_SSE;

        if features.has_avx() {
            xcr0 |= XSAVE_AVX;
        }
        if features.has_avx512f() {
            xcr0 |= XSAVE_AVX512_K | XSAVE_AVX512_ZMM_HI | XSAVE_AVX512_HI16;
        }
        if features.has_pku() {
            xcr0 |= XSAVE_PKRU;
        }

        // SAFETY: xcr0 contains only bits supported by this CPU
        unsafe {
            write_xcr0(xcr0);
        }

        // Update detected XSAVE size
        let size = features.xsave_size();
        set_xsave_area_size(size);
    }

    // FINIT: cleans up x87 state (tag=empty, PC=80bit, RC=round-nearest)
    // SAFETY: FINIT is always valid in Ring 0 with FPU enabled
    unsafe {
        core::arch::asm!("finit", options(nostack));
    }

    // Default MXCSR: mask all non-critical SSE exceptions
    // Bits 7–12 = exception masks ON, bits 13–14 = RC=round-to-nearest
    const MXCSR_DEFAULT: u32 = 0x1F80;
    // SAFETY: ldmxcsr loads a valid MXCSR
    unsafe {
        core::arch::asm!(
            "ldmxcsr [{mxcsr}]",
            mxcsr = in(reg) &MXCSR_DEFAULT as *const u32,
            options(nostack)
        );
    }
}

// ── CLFLUSH / CLFLUSHOPT / CLWB ──────────────────────────────────────────────

/// Flushes a cache line (CLFLUSH — invalidates across all cache levels)
///
/// # SAFETY
/// `addr` must be mapped and accessible.
#[inline(always)]
pub unsafe fn clflush(addr: *const u8) {
    // SAFETY: delegated to caller
    unsafe {
        core::arch::asm!(
            "clflush [{addr}]",
            addr = in(reg) addr,
            options(nostack)
        );
    }
}

/// Optimized flush (CLFLUSHOPT — non-serializing, faster than CLFLUSH)
///
/// # SAFETY
/// Same as `clflush`. Requires CLFLUSHOPT (CPUID leaf 7, EBX[23]).
#[inline(always)]
pub unsafe fn clflushopt(addr: *const u8) {
    // SAFETY: delegated to caller
    unsafe {
        core::arch::asm!(
            "clflushopt [{addr}]",
            addr = in(reg) addr,
            options(nostack)
        );
    }
}

/// Cache line write-back (CLWB — writeback without invalidating)
///
/// # SAFETY
/// Same as `clflush`. Requires CLWB (CPUID leaf 7, EBX[24]).
#[inline(always)]
pub unsafe fn clwb(addr: *const u8) {
    // SAFETY: delegated to caller
    unsafe {
        core::arch::asm!(
            "clwb [{addr}]",
            addr = in(reg) addr,
            options(nostack)
        );
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// C ABI EXPORTS — scheduler/fpu/save_restore.rs interface
// ═════════════════════════════════════════════════════════════════════════════
//
// These #[no_mangle] extern "C" functions are the FFI bridge between
// scheduler/fpu/save_restore.rs (RULE FPU-01 DOC3) and this module.
//
// scheduler/ is Layer 1 and cannot directly import arch/.
// The logic / instructions separation is respected:
//   • scheduler/fpu/save_restore.rs = LOGIC (when to save, for whom)
//   • arch/x86_64/cpu/fpu.rs        = Raw ASM INSTRUCTIONS (xsave/xrstor)
//
// SYNCHRONIZATION: any signature modification here MUST be updated
// in scheduler/fpu/save_restore.rs extern "C" block simultaneously.
// ═════════════════════════════════════════════════════════════════════════════

/// Saves extended FPU state via XSAVE (C ABI export).
///
/// Called from `scheduler::fpu::save_restore::xsave_current()`.
///
/// # Safety
/// - `area` must be 64-byte aligned, size ≥ `xsave_area_size()`.
/// - OSXSAVE must be enabled in CR4.
/// - XSAVE must be supported (check `arch_has_xsave()` beforehand).
#[no_mangle]
pub unsafe extern "C" fn arch_xsave64(area: *mut u8, rfbm: u64) {
    // SAFETY: delegated to caller (scheduler checks arch_has_xsave and alignment).
    xsave(area, rfbm);
}

/// Restores extended FPU state via XRSTOR (C ABI export).
///
/// Called from `scheduler::fpu::save_restore::xrstor_for()`.
///
/// # Safety
/// - `area` must be a valid XSAVE area, 64-byte aligned.
/// - The header component (offset 512) must be consistent with XCR0.
#[no_mangle]
pub unsafe extern "C" fn arch_xrstor64(area: *const u8, rfbm: u64) {
    // SAFETY: delegated to caller.
    xrstor(area, rfbm);
}

/// Saves FPU/SSE state via FXSAVE (C ABI export, fallback without XSAVE).
///
/// Called from `scheduler::fpu::save_restore::xsave_current()` if !arch_has_xsave().
///
/// # Safety
/// - `area` must be at least 16-byte aligned (64 recommended), size ≥ 512.
#[no_mangle]
pub unsafe extern "C" fn arch_fxsave64(area: *mut u8) {
    // SAFETY: delegated to caller.
    fxsave(area);
}

/// Restores FPU/SSE state via FXRSTOR (C ABI export, fallback without XSAVE).
///
/// Called from `scheduler::fpu::save_restore::xrstor_for()` if !arch_has_xsave().
///
/// # Safety
/// - `area` must be a valid FXSAVE area, 16-byte aligned.
#[no_mangle]
pub unsafe extern "C" fn arch_fxrstor64(area: *const u8) {
    // SAFETY: delegated to caller.
    fxrstor(area);
}

/// Returns 1 if the CPU supports XSAVE, 0 otherwise (C ABI export).
///
/// Called from `scheduler::fpu::save_restore::init()` at boot.
/// Reads only feature flags, no side effects.
#[no_mangle]
pub extern "C" fn arch_has_xsave() -> u8 {
    super::features::cpu_features_or_none().is_some_and(|features| features.has_xsave()) as u8
}

/// Returns 1 if the CPU supports AVX (YMM registers), 0 otherwise (C ABI export).
///
/// Called from `scheduler::fpu::save_restore` to dimension the buffer.
/// Reads only feature flags, no side effects.
#[no_mangle]
pub extern "C" fn arch_has_avx() -> u8 {
    super::features::cpu_features_or_none().is_some_and(|features| features.has_avx()) as u8
}
