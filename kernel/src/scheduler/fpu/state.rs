// kernel/src/scheduler/fpu/state.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// FPU STATE — XSAVE/FXSAVE memory area per thread (Exo-OS Scheduler · Layer 1)
// ═══════════════════════════════════════════════════════════════════════════════
//
// RULE FPU-01 (DOC3):
//   This module manages FPU state LOGIC (when, for which thread).
//   Raw ASM instructions (xsave/xrstor) reside in arch/x86_64/cpu/fpu.rs.
//   DO NOT duplicate ASM instructions here.
//
// Area sizes by FPU extension level:
//   • FXSAVE only            : 512 bytes (x86_64 minimum)
//   • XSAVE + AVX            : 832 bytes
//   • XSAVE + AVX + AVX-512  : 2688 bytes
//
// Mandatory alignment: 64 bytes (required by XSAVE instruction).
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicUsize, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// XSAVE size detection at boot
// ─────────────────────────────────────────────────────────────────────────────

/// Current XSAVE area size — detected at boot via CPUID.
/// Initialized to 512 (incompressible FXSAVE), updated by detect_xsave_size().
pub static XSAVE_AREA_SIZE: AtomicUsize = AtomicUsize::new(512);

/// Minimum size (FXSAVE, always available on x86_64).
pub const FXSAVE_SIZE: usize = 512;
/// Size with XSAVE + AVX.
pub const XSAVE_AVX_SIZE: usize = 832;
/// Size with XSAVE + AVX-512.
pub const XSAVE_AVX512_SIZE: usize = 2688;
/// Maximum supported size — must match FpuState::MAX_SIZE.
pub const FPU_STATE_MAX_SIZE: usize = 2688;

/// Detects XSAVE area size via CPUID and initializes XSAVE_AREA_SIZE.
/// Called from scheduler::init(), step 3.
pub fn detect_xsave_size() {
    #[cfg(target_arch = "x86_64")]
    {
        // CPUID.(EAX=0Dh, ECX=0):EBX returns XSAVE area size.
        let size: usize;
        // SAFETY: CPUID available on x86_64; rbx saved/restored (LLVM does not support it as direct operand).
        unsafe {
            let cpuid_eax: u32;
            let cpuid_ebx: u32;
            core::arch::asm!(
                "push rbx",
                "cpuid",
                "mov {out_ebx:e}, ebx",
                "pop rbx",
                inout("eax") 0x0Du32 => cpuid_eax,
                inout("ecx") 0u32 => _,
                out("edx") _,
                out_ebx = lateout(reg) cpuid_ebx,
            );
            let _ = cpuid_eax; // EAX ignored, only EBX is needed.
                               // ebx = minimum size of XSAVE area to save all enabled components.
            if cpuid_ebx >= 512 {
                size = (cpuid_ebx as usize).min(FPU_STATE_MAX_SIZE);
            } else {
                // CPUID not supported or invalid result → FXSAVE by default.
                size = FXSAVE_SIZE;
            }
        }
        XSAVE_AREA_SIZE.store(size, Ordering::Release);
    }
    // On non-x86_64 architecture: fixed size 512.
}

// ─────────────────────────────────────────────────────────────────────────────
// FpuState — aligned area for XSAVE/FXSAVE
// ─────────────────────────────────────────────────────────────────────────────

/// Per-thread FPU/SIMD save area — 2688 bytes, 64-byte aligned.
///
/// The effective size used is in XSAVE_AREA_SIZE (512 to 2688).
/// The remainder is unused padding (inert area).
///
/// RULE: allocated outside the TCB (TCB = 128 bytes fixed).
///       The TCB stores only `fpu_state_ptr: *mut u8`.
#[repr(C, align(64))]
pub struct FpuState {
    /// Raw FXSAVE/XSAVE data — layout dictated by CPU.
    /// First 512 bytes are always FXSAVE-compatible.
    data: [u8; FPU_STATE_MAX_SIZE],
    /// Actually used size (copy of XSAVE_AREA_SIZE at allocation time).
    pub active_size: usize,
    /// Generation number to detect stale saves (debug).
    pub generation: u64,
}

impl FpuState {
    /// Creates a pristine FPU area (initial x86_64 processor state after FINIT).
    pub const fn new() -> Self {
        let mut data = [0u8; FPU_STATE_MAX_SIZE];
        // Default FXSAVE initialization:
        // FCW = 0x037F (masks all FP exceptions except invalid)
        // MXCSR = 0x1F80 (rounding mode = round-to-nearest, flush-to-zero off)
        data[0] = 0x7F; // FCW low
        data[1] = 0x03; // FCW high
        data[24] = 0x80; // MXCSR low  : 0x1F80
        data[25] = 0x1F; // MXCSR high
        Self {
            data,
            active_size: FXSAVE_SIZE,
            generation: 0,
        }
    }

    /// Returns a raw pointer to data area for XSAVE/XRSTOR.
    #[inline(always)]
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.data.as_mut_ptr()
    }

    /// Returns a const pointer for XRSTOR.
    #[inline(always)]
    pub fn as_ptr(&self) -> *const u8 {
        self.data.as_ptr()
    }

    /// Updates active size from XSAVE_AREA_SIZE (called on first save).
    #[inline(always)]
    pub fn refresh_size(&mut self) {
        self.active_size = XSAVE_AREA_SIZE.load(Ordering::Relaxed);
    }

    /// Returns active size in bytes.
    #[inline(always)]
    pub fn size(&self) -> usize {
        self.active_size
    }
}

impl Default for FpuState {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: FpuState is a pure data structure, with no internal pointers.
// It can be transferred between CPUs via scheduler (only one CPU at a time uses it).
unsafe impl Send for FpuState {}
