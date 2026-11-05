// kernel/src/arch/x86_64/time/sources/tsc.rs
//
// ════════════════════════════════════════════════════════════════════════════════
// TSC source — capabilities, invariance, VM-aware, IA32_TSC_ADJUST
// ════════════════════════════════════════════════════════════════════════════════
//
// ## Invariant TSC (RULE TIME-04)
//   Invariant TSC = constant rate across all C-states and Turbo boost.
//   Bit 8 of EDX in CPUID 0x80000007 -> InvariantTSC.
//   Rating 400 if invariant, 50 if non-invariant.
//
// ## IA32_TSC_ADJUST MSR (Intel SDM Vol.3 § 17.15)
//   MSR 0x3B = software offset that firmware/OS can add to the TSC.
//   Raw value = RDTSC + IA32_TSC_ADJUST.
//   To be read at boot to detect non-zero offset (BIOS errata / live migration).
//
// ## IA32_TSC_AUX MSR (0xC0000103)
//   Contains the CPU_ID (core + socket) encoded by OS or firmware.
//   Accessible via RDTSCP: returns (TSC, TSC_AUX), atomic guarantee.
//
// ## VM Detection
//   KVM     : CPUID 0x40000001 bit 9 (KVM_FEATURE_CLOCKSOURCE2)
//   Hyper-V : CPUID 0x40000003 bit 20 (HV_FEATURE_TSC_INVARIANT)
//   VMware  : CPUID 0x40000010 (VMware Timing Leaf)
//   Generic : CPUID 0x1 ECX bit 31 (Hypervisor Present)
//
// ## RDTSCP
//   Serializing instruction (like LFENCE + RDTSC) + IA32_TSC_AUX -> safe in SMP.
//   Available since Intel Nehalem / AMD Barcelona.
// ════════════════════════════════════════════════════════════════════════════════

use super::ClockSource;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

// ── MSR addresses ────────────────────────────────────────────────────────────

const MSR_IA32_TSC_ADJUST: u32 = 0x0000_003B;
#[allow(dead_code)]
const MSR_IA32_TSC_AUX: u32 = 0xC000_0103;
/// Max leaf for standard CPUID (leaf 0).
const CPUID_MAX_STD_LEAF: u32 = 0x0000_0000;
/// CPUID extended features.
#[allow(dead_code)]
const CPUID_EXTENDED_FEAT: u32 = 0x8000_0007;

// ── TSC capabilities ──────────────────────────────────────────────────────────

/// Complete TSC capabilities detected at boot.
#[derive(Debug, Clone, Copy)]
pub struct TscCapabilities {
    /// Invariant TSC (CPUID 0x80000007 EDX bit 8).
    pub invariant: bool,
    /// RDTSCP available (CPUID 0x80000001 EDX bit 27).
    pub rdtscp: bool,
    /// TSC_DEADLINE available (CPUID 0x1 ECX bit 24).
    pub tsc_deadline: bool,
    /// IA32_TSC_ADJUST MSR present and non-zero (= BIOS offset the TSC).
    pub tsc_adjust_nonzero: bool,
    /// IA32_TSC_ADJUST value read at boot.
    pub tsc_adjust_value: i64,
    /// VM/hypervisor detected.
    pub hypervisor: bool,
    /// Hypervisor type.
    pub hypervisor_type: HypervisorType,
}

impl TscCapabilities {
    /// Initial value (all false, to initialize by calling `detect()`).
    pub const fn uninit() -> Self {
        TscCapabilities {
            invariant: false,
            rdtscp: false,
            tsc_deadline: false,
            tsc_adjust_nonzero: false,
            tsc_adjust_value: 0,
            hypervisor: false,
            hypervisor_type: HypervisorType::None,
        }
    }

    /// Detects all TSC capabilities for the current CPU.
    pub fn detect() -> Self {
        let invariant = check_tsc_invariant();
        let rdtscp = check_rdtscp();
        let tsc_deadline = check_tsc_deadline();
        let (hyp, hyp_type) = detect_hypervisor();
        let adj = read_tsc_adjust_msr();
        TscCapabilities {
            invariant,
            rdtscp,
            tsc_deadline,
            tsc_adjust_nonzero: adj != 0,
            tsc_adjust_value: adj,
            hypervisor: hyp,
            hypervisor_type: hyp_type,
        }
    }

    /// Conditional rating based on capabilities.
    ///   - Invariant + non-hypervisor -> 400
    ///   - Invariant + hypervisor     -> 350 (VM TSC scaling possible)
    ///   - Non-invariant              -> 50  (unstable with C-states)
    pub fn rating(&self) -> u32 {
        if self.invariant {
            if self.hypervisor {
                350
            } else {
                400
            }
        } else {
            50
        }
    }
}

// ── Hypervisor ────────────────────────────────────────────────────────────────

/// Detected hypervisor type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HypervisorType {
    None,
    KvmLinux,
    HyperV,
    VMware,
    Unknown,
}

impl HypervisorType {
    pub fn as_str(self) -> &'static str {
        match self {
            HypervisorType::None => "none",
            HypervisorType::KvmLinux => "KVM/Linux",
            HypervisorType::HyperV => "Hyper-V",
            HypervisorType::VMware => "VMware",
            HypervisorType::Unknown => "unknown-hypervisor",
        }
    }
}

// ── Globals ───────────────────────────────────────────────────────────────────

/// TSC capabilities initialized once at boot.
static TSC_CAPS_INVARIANT: AtomicBool = AtomicBool::new(false);
static TSC_CAPS_RDTSCP: AtomicBool = AtomicBool::new(false);
static TSC_CAPS_HYPERVISOR: AtomicBool = AtomicBool::new(false);
static TSC_CAPS_RATING: AtomicU32 = AtomicU32::new(0);
static TSC_CAPS_ADJUST: AtomicU64 = AtomicU64::new(0);
/// `true` if `init_tsc_source()` has been called.
static TSC_SOURCE_INIT_DONE: AtomicBool = AtomicBool::new(false);

/// Initializes TSC capabilities and updates globals.
/// To be called once from `time_init()`.
pub fn init_tsc_source() {
    if TSC_SOURCE_INIT_DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    let caps = TscCapabilities::detect();
    // Boot-stability rule: do not execute RDTSCP from critical clock paths until
    // the kernel has a recoverable #UD feature probe. Some QEMU/TCG profiles
    // expose inconsistent RDTSCP state; LFENCE+RDTSC is slower but universal.
    let rdtscp_usable = false;
    TSC_CAPS_INVARIANT.store(caps.invariant, Ordering::Relaxed);
    TSC_CAPS_RDTSCP.store(caps.rdtscp && rdtscp_usable, Ordering::Relaxed);
    TSC_CAPS_HYPERVISOR.store(caps.hypervisor, Ordering::Relaxed);
    TSC_CAPS_RATING.store(caps.rating(), Ordering::Relaxed);
    // IA32_TSC_ADJUST as raw value (bit pattern of i64).
    TSC_CAPS_ADJUST.store(caps.tsc_adjust_value as u64, Ordering::Relaxed);
}

/// Indicates whether RDTSCP is usable on the current CPU.
///
/// Before `init_tsc_source()`, returns `false` deliberately: the
/// serialized RDTSC path is available across all x86_64 and avoids executing an
/// optional instruction before CPUID has been propagated to atomics.
#[inline(always)]
pub fn rdtscp_available() -> bool {
    TSC_SOURCE_INIT_DONE.load(Ordering::Relaxed) && TSC_CAPS_RDTSCP.load(Ordering::Relaxed)
}

// ── TSC ClockSource source ────────────────────────────────────────────────────

pub struct TscSource;

impl ClockSource for TscSource {
    fn name(&self) -> &'static str {
        "TSC"
    }

    fn rating(&self) -> u32 {
        let r = TSC_CAPS_RATING.load(Ordering::Relaxed);
        if r == 0 {
            // Fallback if init_tsc_source() not yet called.
            if check_tsc_invariant() {
                400
            } else {
                50
            }
        } else {
            r
        }
    }

    fn read(&self) -> u64 {
        if TSC_CAPS_RDTSCP.load(Ordering::Relaxed) {
            rdtscp_read()
        } else {
            rdtsc_read()
        }
    }

    fn freq_hz(&self) -> u64 {
        crate::arch::x86_64::cpu::tsc::tsc_hz()
    }

    fn available(&self) -> bool {
        true
    } // TSC is always present on x86_64
}

// ── TSC reading ───────────────────────────────────────────────────────────────

/// TSC read via RDTSC (non-serializing).
#[inline(always)]
pub fn rdtsc_read() -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: RDTSC is non-privileged on x86_64.
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }
    ((hi as u64) << 32) | lo as u64
}

/// TSC read via RDTSCP (serializing + IA32_TSC_AUX in ECX).
/// Guarantees that all preceding instructions are completed before reading.
#[inline]
pub fn rdtscp_read() -> u64 {
    let lo: u32;
    let hi: u32;
    let _aux: u32; // TSC_AUX (CPU ID), ignored here — used by percpu::sync.
                   // SAFETY: RDTSCP available if check_rdtscp() == true.
    unsafe {
        core::arch::asm!(
            "rdtscp",
            out("eax") lo,
            out("edx") hi,
            out("ecx") _aux,
            options(nostack, nomem)
        );
    }
    ((hi as u64) << 32) | lo as u64
}

/// Ordered TSC read with associated logical CPU ID.
///
/// RDTSCP provides TSC and `TSC_AUX` atomically when advertised by CPU. On
/// CPUs/QEMU without RDTSCP, LFENCE+RDTSC+LFENCE is used and `GS:cpu_id` is re-read
/// before/after to prevent applying another CPU's offset if the thread migrated
/// between the TSC read and per-CPU sampling.
#[inline(always)]
pub fn read_ordered_with_cpu() -> (u64, u32) {
    loop {
        let cpu_before = crate::arch::x86_64::smp::percpu::current_cpu_id();
        // SAFETY: LFENCE serializes pipeline, no memory effect; always
        // available on x86_64 (SSE2 baseline).
        unsafe {
            core::arch::asm!("lfence", options(nostack, nomem, preserves_flags));
        }
        let tsc = rdtsc_read();
        // SAFETY: LFENCE serializes pipeline, no memory effect; always
        // available on x86_64 (SSE2 baseline).
        unsafe {
            core::arch::asm!("lfence", options(nostack, nomem, preserves_flags));
        }
        let cpu_after = crate::arch::x86_64::smp::percpu::current_cpu_id();
        if cpu_before == cpu_after {
            return (tsc, cpu_after);
        }
        core::hint::spin_loop();
    }
}

/// Read with pre/post LFENCE for full serialization (precise measurement).
#[inline]
pub fn rdtsc_serialized() -> u64 {
    // SAFETY: LFENCE serializes pipeline, no memory effect; always
    // available on x86_64 (SSE2 baseline).
    unsafe {
        core::arch::asm!("lfence", options(nostack, nomem));
    }
    let t = rdtsc_read();
    // SAFETY: LFENCE serializes pipeline, no memory effect; always
    // available on x86_64 (SSE2 baseline).
    unsafe {
        core::arch::asm!("lfence", options(nostack, nomem));
    }
    t
}

// ── Capabilities detection ────────────────────────────────────────────────────

/// Checks whether TSC is invariant via CPUID 0x80000007 EDX bit 8.
/// Invariant TSC = constant rate across all C-states and Turbo Boost.
pub fn check_tsc_invariant() -> bool {
    let edx: u32;
    // SAFETY: CPUID is a non-privileged instruction without memory effects;
    // preamble xchg rbx preserves LLVM reserved register.
    unsafe {
        core::arch::asm!(
            "xchg {tmp:r}, rbx",
            "cpuid",
            "xchg {tmp:r}, rbx",
            inout("eax") 0x8000_0007u32 => _,
            inout("ecx") 0u32           => _,
            out("edx") edx,
            tmp = inout(reg) 0u64 => _,
            options(nostack, nomem)
        );
    }
    (edx & (1 << 8)) != 0
}

/// Checks whether RDTSCP is available (CPUID 0x80000001 EDX bit 27).
pub fn check_rdtscp() -> bool {
    let max_ext: u32;
    // SAFETY: CPUID is a non-privileged instruction without memory effects;
    // preamble xchg rbx preserves LLVM reserved register.
    unsafe {
        core::arch::asm!(
            "xchg {tmp:r}, rbx",
            "cpuid",
            "xchg {tmp:r}, rbx",
            inout("eax") 0x8000_0000u32 => max_ext,
            inout("ecx") 0u32 => _,
            out("edx") _,
            tmp = inout(reg) 0u64 => _,
            options(nostack, nomem)
        );
    }
    if max_ext < 0x8000_0001 {
        return false;
    }
    let edx: u32;
    // SAFETY: CPUID is a non-privileged instruction without memory effects;
    // preamble xchg rbx preserves LLVM reserved register.
    unsafe {
        core::arch::asm!(
            "xchg {tmp:r}, rbx",
            "cpuid",
            "xchg {tmp:r}, rbx",
            inout("eax") 0x8000_0001u32 => _,
            inout("ecx") 0u32           => _,
            out("edx") edx,
            tmp = inout(reg) 0u64 => _,
            options(nostack, nomem)
        );
    }
    (edx & (1 << 27)) != 0
}

/// Checks whether TSC_DEADLINE is available (CPUID 0x1 ECX bit 24).
pub fn check_tsc_deadline() -> bool {
    let ecx: u32;
    // SAFETY: CPUID is a non-privileged instruction without memory effects;
    // preamble xchg rbx preserves LLVM reserved register.
    unsafe {
        core::arch::asm!(
            "xchg {tmp:r}, rbx",
            "cpuid",
            "xchg {tmp:r}, rbx",
            inout("eax") 0x0000_0001u32 => _,
            inout("ecx") 0u32           => ecx,
            out("edx") _,
            tmp = inout(reg) 0u64 => _,
            options(nostack, nomem)
        );
    }
    (ecx & (1 << 24)) != 0
}

/// Detects if running under a hypervisor and which one.
/// Returns (is_hypervisor, HypervisorType).
pub fn detect_hypervisor() -> (bool, HypervisorType) {
    // Bit 31 of ECX of CPUID 0x1 = Hypervisor Present.
    let ecx: u32;
    // SAFETY: CPUID is a non-privileged instruction without memory effects;
    // preamble xchg rbx preserves LLVM reserved register.
    unsafe {
        core::arch::asm!(
            "xchg {tmp:r}, rbx",
            "cpuid",
            "xchg {tmp:r}, rbx",
            inout("eax") 0x0000_0001u32 => _,
            inout("ecx") 0u32           => ecx,
            out("edx") _,
            tmp = inout(reg) 0u64 => _,
            options(nostack, nomem)
        );
    }
    if (ecx & (1 << 31)) == 0 {
        return (false, HypervisorType::None);
    }

    // Read hypervisor signature from leaf 0x40000000.
    let (ebx, ecx_leaf, edx): (u32, u32, u32);
    // SAFETY: CPUID is a non-privileged instruction without memory effects;
    // preamble xchg rbx preserves LLVM reserved register.
    unsafe {
        core::arch::asm!(
            "xchg {tmp:r}, rbx",
            "cpuid",
            "mov {rbx_out:e}, {tmp:e}",
            "xchg {tmp:r}, rbx",
            inout("eax") 0x4000_0000u32 => _,
            inout("ecx") 0u32           => ecx_leaf,
            out("edx") edx,
            tmp = inout(reg) 0u64 => _,
            rbx_out = out(reg) ebx,
            options(nostack, nomem)
        );
    }

    // KVM : "KVMKVMKVM\0\0\0" in EBX+ECX+EDX
    // Hyper-V : "Microsoft Hv"
    // VMware  : "VMwareVMware"
    let sig = [ebx, ecx_leaf, edx];
    // SAFETY: `sig` is [u32; 3] = 12 contiguous bytes on stack; reinterpreted
    // as &[u8; 12] to compare hypervisor signature. Lifetime of slice is bounded
    // to that of `sig` (same scope).
    let sig_bytes = unsafe { core::slice::from_raw_parts(sig.as_ptr() as *const u8, 12) };

    if sig_bytes.starts_with(b"KVMKVM") {
        (true, HypervisorType::KvmLinux)
    } else if sig_bytes.starts_with(b"Microsoft Hv") {
        (true, HypervisorType::HyperV)
    } else if sig_bytes.starts_with(b"VMwareVMware") {
        (true, HypervisorType::VMware)
    } else {
        (true, HypervisorType::Unknown)
    }
}

// ── IA32_TSC_ADJUST MSR ───────────────────────────────────────────────────────

/// Reads IA32_TSC_ADJUST MSR (MSR 0x3B).
///
/// Returns 0 if read fails (GP fault on old CPUs lacking this MSR).
/// On modern CPUs, this MSR is always present (available since Ivy Bridge).
pub fn read_tsc_adjust_msr() -> i64 {
    // Verify that CPUID 0x7 advertises IA32_TSC_ADJUST (EBX bit 1).
    if !cpuid_tsc_adjust_supported() {
        return 0;
    }

    let lo: u32;
    let hi: u32;
    // SAFETY: RDMSR is privileged (CPL=0 required — we are in kernel).
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") MSR_IA32_TSC_ADJUST,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }
    let raw = ((hi as u64) << 32) | lo as u64;
    raw as i64
}

/// Writes IA32_TSC_ADJUST to neutralize BIOS offset.
/// Ring 0 (kernel) only. Never call from userspace.
pub fn write_tsc_adjust_msr(value: i64) {
    if !cpuid_tsc_adjust_supported() {
        return;
    }
    let raw = value as u64;
    let lo = raw as u32;
    let hi = (raw >> 32) as u32;
    // SAFETY: x86_64 hardware write without memory side-effects (Ring 0).
    unsafe {
        core::arch::asm!(
            "wrmsr",
            in("ecx") MSR_IA32_TSC_ADJUST,
            in("eax") lo,
            in("edx") hi,
            options(nostack, nomem)
        );
    }
}

/// Checks whether IA32_TSC_ADJUST is supported (CPUID 0x7.0 EBX bit 1).
pub fn cpuid_tsc_adjust_supported() -> bool {
    let max_leaf: u32;
    // SAFETY: CPUID is a non-privileged instruction without memory effects;
    // preamble xchg rbx preserves LLVM reserved register.
    unsafe {
        core::arch::asm!(
            "xchg {tmp:r}, rbx",
            "cpuid",
            "xchg {tmp:r}, rbx",
            inout("eax") CPUID_MAX_STD_LEAF => max_leaf,
            inout("ecx") 0u32 => _,
            out("edx") _,
            tmp = inout(reg) 0u64 => _,
            options(nostack, nomem)
        );
    }
    if max_leaf < 7 {
        return false;
    }
    let ebx: u32;
    // SAFETY: CPUID is a non-privileged instruction without memory effects;
    // preamble xchg rbx preserves LLVM reserved register.
    unsafe {
        core::arch::asm!(
            "xchg {tmp:r}, rbx",
            "cpuid",
            "mov {rbx_val:e}, {tmp:e}",
            "xchg {tmp:r}, rbx",
            inout("eax") 0x0000_0007u32 => _,
            inout("ecx") 0u32 => _,
            out("edx") _,
            tmp = inout(reg) 0u64 => _,
            rbx_val = out(reg) ebx,
            options(nostack, nomem)
        );
    }
    (ebx & (1 << 1)) != 0
}

// ── CPUID leaf 0x15 availability ──────────────────────────────────────────────

/// Checks whether CPUID leaf 0x15 is available for nominal calibration.
pub fn cpuid_tsc_available() -> bool {
    let max_leaf: u32;
    // SAFETY: CPUID is a non-privileged instruction without memory effects;
    // preamble xchg rbx preserves LLVM reserved register.
    unsafe {
        core::arch::asm!(
            "xchg {tmp:r}, rbx",
            "cpuid",
            "xchg {tmp:r}, rbx",
            inout("eax") 0u32 => max_leaf,
            inout("ecx") 0u32 => _,
            out("edx") _,
            tmp = inout(reg) 0u64 => _,
            options(nostack, nomem)
        );
    }
    max_leaf >= 0x15
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Returns current TSC rating (from globals).
pub fn tsc_rating() -> u32 {
    TscSource.rating()
}

/// Returns `true` if TSC is invariant (from globals if init done).
pub fn tsc_is_invariant() -> bool {
    if TSC_SOURCE_INIT_DONE.load(Ordering::Relaxed) {
        TSC_CAPS_INVARIANT.load(Ordering::Relaxed)
    } else {
        check_tsc_invariant()
    }
}

/// Returns `true` if running under hypervisor.
pub fn tsc_under_hypervisor() -> bool {
    TSC_CAPS_HYPERVISOR.load(Ordering::Relaxed)
}

/// Returns IA32_TSC_ADJUST value read at boot.
pub fn tsc_adjust_value() -> i64 {
    TSC_CAPS_ADJUST.load(Ordering::Relaxed) as i64
}

/// Returns TSC frequency in Hz.
pub fn tsc_freq_hz() -> u64 {
    crate::arch::x86_64::cpu::tsc::tsc_hz()
}
