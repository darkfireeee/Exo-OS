// kernel/src/arch/x86_64/time/calibration/cpuid_nominal.rs
//
// ════════════════════════════════════════════════════════════════════════════════
// Nominal TSC calibration via CPUID — Intel leaf 0x15 / 0x16, AMD leaf 0x80000019
// ════════════════════════════════════════════════════════════════════════════════
//
// ## Nominal frequency sources (without external timer)
//
//   CPUID leaf 0x15  (Intel Skylake+, 2015)
//     EAX = denominator of TSC/crystal ratio
//     EBX = numerator of TSC/crystal ratio
//     ECX = crystal frequency in Hz (0 if not provided -> 24 MHz Intel fallback)
//     TSC_Hz = crystal_hz * EBX / EAX
//
//   CPUID leaf 0x16  (Intel Skylake+)
//     EAX[15:0] = CPU base frequency in MHz
//     Less precise than 0x15, used as CPUID fallback or cross-check.
//
//   CPUID leaf 0x80000019  (AMD Family 17h+, Zen)
//     CurrUcodeRev / Core Performance Boost -> nominal TSC in MHz (bits 31:16 ECX)
//     Or via MSR 0xC0010064 (P-state 0) -> FID/DID -> TSC frequency
//
// ## Intel reference crystals (ECX = 0 in leaf 0x15)
//   Skylake / Kaby Lake / Coffee Lake   : 24 000 000 Hz
//   Atom Goldmont / Silvermont          : 19 200 000 Hz
//   Atom Gemini Lake / Tremont          : 19 200 000 Hz
//   Tiger Lake / Alder Lake             : 38 400 000 Hz (newer SoCs)
//   MMIO ICH/FCH latency                : not relevant here
//
// ## Rating : 150 — manufacturer nominal, without measurable drift
//   Advantage : no need for HPET/PM Timer -> boots very fast
//   Drawback  : does not measure real TSC drift in hostile environments
//     (VM, Turbo Boost disabled, CPPC enabled, etc.)
//
// ## Accuracy : ±0.01% if reference crystal provided, ±1% if ECX=0 and fallback 24 MHz
// ════════════════════════════════════════════════════════════════════════════════

// ── Intel reference crystal constants ─────────────────────────────────────────

/// Standard Intel 24 MHz crystal frequency (Skylake, Kaby Lake, Coffee Lake...)
pub const CRYSTAL_FREQ_24MHZ: u64 = 24_000_000;
/// Standard Intel 19.2 MHz crystal frequency (Atom Goldmont, Silvermont)
pub const CRYSTAL_FREQ_192MHZ: u64 = 19_200_000;
/// Standard Intel 38.4 MHz crystal frequency (Tiger Lake, Alder Lake SoC)
pub const CRYSTAL_FREQ_384MHZ: u64 = 38_400_000;
/// AMD Zen crystal frequency (typically 100 MHz / divider -> TSC via MSRC001_00[6B:64])
pub const CRYSTAL_FREQ_100MHZ: u64 = 100_000_000;

/// Minimum acceptable frequency for nominal TSC [100 MHz].
pub const TSC_MIN_HZ: u64 = 100_000_000;
/// Maximum acceptable frequency for nominal TSC [10 GHz].
pub const TSC_MAX_HZ: u64 = 10_000_000_000;
const KVM_TSC_FREQ_LEAF: u32 = 0x4000_0010;

// ── Detailed results ──────────────────────────────────────────────────────────

/// Complete result of reading CPUID leaf 0x15.
#[derive(Debug, Clone, Copy)]
pub struct CpuidLeaf15Result {
    /// Denominator of crystal/TSC ratio (EAX).
    pub denominator: u32,
    /// Numerator of crystal/TSC ratio (EBX).
    pub numerator: u32,
    /// Crystal frequency in Hz (ECX, 0 if not provided).
    pub crystal_hz: u32,
    /// Effective crystal frequency used (after fallback if ECX=0).
    pub crystal_used: u64,
    /// Calculated TSC frequency, 0 if invalid.
    pub tsc_hz: u64,
    /// Crystal source used.
    pub crystal_src: CrystalSource,
}

/// Source of the crystal frequency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrystalSource {
    /// ECX provided by CPUID (most reliable).
    CpuidEcx,
    /// Standard 24 MHz value (Skylake, KBL, CFL inferred from CPUID brand).
    Standard24Mhz,
    /// Standard 19.2 MHz value (Atom Goldmont/Silvermont).
    Standard192Mhz,
    /// Standard 38.4 MHz value (Tiger Lake, Alder Lake).
    Standard384Mhz,
    /// Default Intel value (24 MHz, for unidentified CPUs).
    DefaultIntel24Mhz,
    /// Leaf 0x15 not available.
    NotAvailable,
}

impl CrystalSource {
    pub fn as_str(self) -> &'static str {
        match self {
            CrystalSource::CpuidEcx => "CPUID-ECX",
            CrystalSource::Standard24Mhz => "24MHz-SKL",
            CrystalSource::Standard192Mhz => "19.2MHz-ATOM",
            CrystalSource::Standard384Mhz => "38.4MHz-TGL",
            CrystalSource::DefaultIntel24Mhz => "24MHz-DFLT",
            CrystalSource::NotAvailable => "N/A",
        }
    }
}

/// Result of reading CPUID leaf 0x16.
#[derive(Debug, Clone, Copy)]
pub struct CpuidLeaf16Result {
    /// CPU base frequency in MHz (EAX[15:0]).
    pub base_mhz: u32,
    /// Max (Turbo) frequency in MHz (EBX[15:0]).
    pub max_mhz: u32,
    /// Bus frequency in MHz (ECX[15:0]).
    pub bus_mhz: u32,
    /// Inferred TSC frequency (= base_mhz * 1_000_000).
    pub tsc_hz: u64,
}

// ── Main API ──────────────────────────────────────────────────────────────────

/// Attempts to read the nominal TSC frequency via CPUID leaf 0x15.
///
/// Algorithm:
///   1. Check that leaf 0x15 is supported (max_leaf >= 0x15)
///   2. Read EAX (denominator), EBX (numerator), ECX (crystal_hz)
///   3. If ECX == 0 -> detect crystal from CPU model
///   4. Compute: tsc_hz = crystal_hz * EBX / EAX
///   5. Validate range [100 MHz, 10 GHz]
///
/// RULE CALIBRATION-CPUID-01: If EAX=0 or EBX=0, leaf 0x15 invalid -> None.
/// RULE CALIBRATION-CPUID-02: If ECX=0, detect crystal by CPU model.
///
/// Returns `None` if leaf 0x15 not available or data invalid.
pub fn cpuid_tsc_hz() -> Option<u64> {
    let result = cpuid_leaf15_full()?;
    if result.tsc_hz == 0 {
        None
    } else {
        Some(result.tsc_hz)
    }
}

/// Variant returning complete result with diagnostics.
pub fn cpuid_tsc_hz_detailed() -> Option<CpuidLeaf15Result> {
    cpuid_leaf15_full()
}

/// Attempts to read the base frequency via CPUID leaf 0x16.
///
/// Returns CPU base frequency × 1_000_000 (conversion MHz → Hz).
/// Accuracy ±1 MHz (rounded to manufacturer MHz).
pub fn cpuid_tsc_hz_leaf16() -> Option<u64> {
    let result = cpuid_leaf16_full()?;
    if result.tsc_hz == 0 {
        None
    } else {
        Some(result.tsc_hz)
    }
}

/// Variant returning complete leaf 0x16 result.
pub fn cpuid_leaf16_detailed() -> Option<CpuidLeaf16Result> {
    cpuid_leaf16_full()
}

/// Checks if CPUID leaf 0x15 is available.
pub fn cpuid_leaf15_available() -> bool {
    max_cpuid_leaf() >= 0x15
}

/// Checks if CPUID leaf 0x16 is available.
pub fn cpuid_leaf16_available() -> bool {
    max_cpuid_leaf() >= 0x16
}

/// Returns the best available CPUID estimate (0x15 > 0x16 > None).
pub fn cpuid_best_estimate() -> Option<u64> {
    if let Some(hz) = cpuid_tsc_hz() {
        return Some(hz);
    }
    cpuid_tsc_hz_leaf16()
}

/// Attempts to read the TSC frequency published by the KVM/QEMU hypervisor.
///
/// Leaf 0x4000_0010: EAX = TSC frequency in kHz. This source avoids fixed
/// fallback when QEMU exposes a paravirtualized frequency without
/// providing manufacturer leaves 0x15/0x16.
pub fn hypervisor_tsc_hz() -> Option<u64> {
    if max_hypervisor_leaf() < KVM_TSC_FREQ_LEAF {
        return None;
    }
    let (eax, _ebx, _ecx, _edx) = read_cpuid_leaf(KVM_TSC_FREQ_LEAF, 0);
    if eax == 0 {
        return None;
    }
    let hz = (eax as u64).saturating_mul(1_000);
    if !(TSC_MIN_HZ..=TSC_MAX_HZ).contains(&hz) {
        None
    } else {
        Some(hz)
    }
}

// ── Internal leaf 0x15 implementation ────────────────────────────────────────

/// Reads CPUID leaf 0x15 and calculates the complete TSC frequency.
fn cpuid_leaf15_full() -> Option<CpuidLeaf15Result> {
    if max_cpuid_leaf() < 0x15 {
        return None;
    }

    let (eax, ebx, ecx) = read_cpuid_leaf_15();

    // EAX or EBX zero = leaf invalid or unimplemented on this CPU.
    if eax == 0 || ebx == 0 {
        return None;
    }

    let ecx_hz = ecx as u64;
    let (crystal_hz, crystal_src) = if ecx_hz > 0 {
        (ecx_hz, CrystalSource::CpuidEcx)
    } else {
        // ECX = 0: detect crystal frequency from CPU model.
        detect_crystal_from_model()
    };

    // TSC_Hz = crystal_hz × EBX / EAX
    let tsc_hz_128 = (crystal_hz as u128)
        .saturating_mul(ebx as u128)
        .checked_div(eax as u128)
        .unwrap_or(0);

    let tsc_hz = if tsc_hz_128 < TSC_MIN_HZ as u128 || tsc_hz_128 > TSC_MAX_HZ as u128 {
        0
    } else {
        tsc_hz_128 as u64
    };

    Some(CpuidLeaf15Result {
        denominator: eax,
        numerator: ebx,
        crystal_hz: ecx,
        crystal_used: crystal_hz,
        tsc_hz,
        crystal_src,
    })
}

/// Reads CPUID leaf 0x16 and constructs the result.
fn cpuid_leaf16_full() -> Option<CpuidLeaf16Result> {
    if max_cpuid_leaf() < 0x16 {
        return None;
    }

    let (eax, ebx, ecx) = read_cpuid_leaf_16();
    let base_mhz = eax & 0xFFFF;

    if base_mhz == 0 {
        return None;
    }

    let tsc_hz = (base_mhz as u64).saturating_mul(1_000_000);
    if !(TSC_MIN_HZ..=TSC_MAX_HZ).contains(&tsc_hz) {
        return None;
    }

    Some(CpuidLeaf16Result {
        base_mhz,
        max_mhz: (ebx & 0xFFFF),
        bus_mhz: (ecx & 0xFFFF),
        tsc_hz,
    })
}

// ── Crystal detection by CPU model ────────────────────────────────────────────

/// Detects crystal frequency from CPUID model identifier.
///
/// Algorithm:
///   1. Read EAX of CPUID leaf 1 -> Family/Model/Stepping
///   2. Identify CPU family
///   3. Return known crystal frequency for this family
///
/// RULE CALIBRATION-CPUID-02: fallback if ECX of leaf 0x15 is zero.
fn detect_crystal_from_model() -> (u64, CrystalSource) {
    let (family, model, _stepping) = read_cpu_family_model();

    // Intel family = 6 (all modern Core, Atom).
    if family == 6 {
        return intel_crystal_by_model(model);
    }

    // AMD: family 0x17 (Zen), 0x19 (Zen 3), 0x1A (Zen 4)
    // AMD generally does not implement leaf 0x15 with EBX, but as precaution:
    if family >= 0x17 {
        // AMD Zen+ uses 100 MHz reference -> Crystal is not relevant here
        // but if leaf 0x15 is present with valid EBX/EAX, crystal is ~100 MHz.
        return (CRYSTAL_FREQ_100MHZ, CrystalSource::Standard24Mhz); // approximate notation
    }

    // Generic Intel fallback: 24 MHz.
    (CRYSTAL_FREQ_24MHZ, CrystalSource::DefaultIntel24Mhz)
}

/// Identifies Intel crystal by model number (Family 6).
fn intel_crystal_by_model(model: u32) -> (u64, CrystalSource) {
    // Intel Developer Manual Volume 3B reference, Table 18-2.
    match model {
        // Skylake-S/H/U (0x4E, 0x5E) — Kaby Lake (0x8E, 0x9E) — Coffee Lake (0x9E)
        // Whiskey Lake (0x8E) — Comet Lake (0xA5, 0xA6) — Rocket Lake (0xA7)
        0x4E | 0x5E | 0x8E | 0x9E | 0xA5 | 0xA6 | 0xA7 => {
            (CRYSTAL_FREQ_24MHZ, CrystalSource::Standard24Mhz)
        }
        // Ice Lake (0x7E, 0x7D)
        0x7E | 0x7D => (CRYSTAL_FREQ_24MHZ, CrystalSource::Standard24Mhz),
        // Tiger Lake (0x8C, 0x8D) — Alder Lake (0x97, 0x9A)
        // Raptor Lake (0xB7, 0xBA) — Meteor Lake (0xAA, 0xAC)
        0x8C | 0x8D | 0x97 | 0x9A | 0xB7 | 0xBA | 0xBF | 0xAA | 0xAC => {
            (CRYSTAL_FREQ_384MHZ, CrystalSource::Standard384Mhz)
        }
        // Atom Goldmont (0x5C) — Goldmont Plus (0x7A)
        0x5C | 0x7A => (CRYSTAL_FREQ_192MHZ, CrystalSource::Standard192Mhz),
        // Atom Tremont (0x86, Elkhart Lake 0x96, Jasper Lake 0x9C)
        0x86 | 0x96 | 0x9C => (CRYSTAL_FREQ_192MHZ, CrystalSource::Standard192Mhz),
        // Haswell (0x3C, 0x3F), Broadwell (0x47, 0x4F) — no valid crystal 0x15
        // but if EBX/EAX valid, assume 25 MHz (standard BCLK frequency).
        0x3C | 0x3F | 0x47 | 0x4F => {
            // Before Skylake, 0x15 is rarely valid — fallback if reached here.
            (CRYSTAL_FREQ_24MHZ, CrystalSource::DefaultIntel24Mhz)
        }
        // Any other Intel Family 6 model -> 24 MHz default (conservative best choice).
        _ => (CRYSTAL_FREQ_24MHZ, CrystalSource::DefaultIntel24Mhz),
    }
}

// ── Low-level CPUID primitives ────────────────────────────────────────────────

/// Returns the maximum standard CPUID leaf supported.
fn max_cpuid_leaf() -> u32 {
    let max: u32;
    // SAFETY: CPUID leaf 0, unprivileged.
    unsafe {
        core::arch::asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 0u32 => max,
            inout("ecx") 0u32 => _,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    max
}

/// Returns the maximum hypervisor CPUID leaf supported.
fn max_hypervisor_leaf() -> u32 {
    let max: u32;
    // SAFETY: Hypervisor CPUID leaf, unprivileged.
    unsafe {
        core::arch::asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 0x4000_0000u32 => max,
            inout("ecx") 0u32 => _,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    max
}

fn read_cpuid_leaf(leaf: u32, subleaf: u32) -> (u32, u32, u32, u32) {
    let eax_out: u32;
    let ebx_out: u32;
    let ecx_out: u32;
    let edx_out: u32;
    // SAFETY: CPUID is unprivileged. RBX is preserved as it is reserved by LLVM in PIC mode.
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov {ebx_r:e}, ebx",
            "pop rbx",
            inout("eax") leaf => eax_out,
            inout("ecx") subleaf => ecx_out,
            out("edx") edx_out,
            ebx_r = out(reg) ebx_out,
            options(nostack, nomem)
        );
    }
    (eax_out, ebx_out, ecx_out, edx_out)
}

/// Reads CPUID leaf 0x15 preserving rbx.
/// Returns (EAX=denominator, EBX=numerator, ECX=crystal_hz).
fn read_cpuid_leaf_15() -> (u32, u32, u32) {
    let eax_out: u32;
    let ebx_out: u32;
    let ecx_out: u32;
    // SAFETY: CPUID is unprivileged. push rbx / pop rbx preserves callee-saved rbx
    //         without interfering with compiler (manual stack frame, outside llvm).
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov {ebx_r:e}, ebx",
            "pop rbx",
            inout("eax") 0x15u32 => eax_out,
            inout("ecx") 0u32    => ecx_out,
            out("edx") _,
            ebx_r = out(reg) ebx_out,
            options(nostack, nomem)
        );
    }
    (eax_out, ebx_out, ecx_out)
}

/// Reads CPUID leaf 0x16 preserving rbx.
/// Returns (EAX=base_mhz, EBX=max_mhz, ECX=bus_mhz).
fn read_cpuid_leaf_16() -> (u32, u32, u32) {
    let eax_out: u32;
    let ebx_out: u32;
    let ecx_out: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov {ebx_r:e}, ebx",
            "pop rbx",
            inout("eax") 0x16u32 => eax_out,
            inout("ecx") 0u32    => ecx_out,
            out("edx") _,
            ebx_r = out(reg) ebx_out,
            options(nostack, nomem)
        );
    }
    (eax_out, ebx_out, ecx_out)
}

/// Reads Family/Model/Stepping from CPUID leaf 1 EAX.
///
/// Intel EAX leaf 1 format:
///   [3:0]   Stepping ID
///   [7:4]   Model
///   [11:8]  Family ID
///   [13:12] Processor Type
///   [19:16] Extended Model ID
///   [27:20] Extended Family ID
fn read_cpu_family_model() -> (u32, u32, u32) {
    let eax: u32;
    // SAFETY: CPUID leaf 1, unprivileged.
    unsafe {
        core::arch::asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 1u32 => eax,
            inout("ecx") 0u32 => _,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    let stepping = eax & 0xF;
    let base_model = (eax >> 4) & 0xF;
    let base_family = (eax >> 8) & 0xF;
    let ext_model = (eax >> 16) & 0xF;
    let ext_family = (eax >> 20) & 0xFF;

    // Intel/AMD: real family = base_family + ext_family (if base_family = 0xF or 0x6)
    let family = if base_family == 0xF {
        base_family + ext_family
    } else {
        base_family
    };
    // Real model = (ext_model << 4) | base_model  (if family == 6 or 0xF)
    let model = if base_family == 6 || base_family == 0xF {
        (ext_model << 4) | base_model
    } else {
        base_model
    };

    (family, model, stepping)
}

// ── Exported utilities ────────────────────────────────────────────────────────

/// Returns the detected crystal frequency for current CPU without calculating TSC.
/// Useful for system diagnostics.
pub fn detect_crystal_hz() -> u64 {
    let (hz, _src) = detect_crystal_from_model();
    hz
}

/// Checks if nominal TSC is consistent with the expected range for the
/// CPU base frequency announced by leaf 0x16.
///
/// Returns `true` if both sources are consistent (delta < 5%).
pub fn cross_check_0x15_vs_0x16() -> bool {
    let hz15 = match cpuid_tsc_hz() {
        Some(v) => v,
        None => return true,
    };
    let hz16 = match cpuid_tsc_hz_leaf16() {
        Some(v) => v,
        None => return true,
    };
    if hz16 == 0 {
        return true;
    }
    let diff = hz15.abs_diff(hz16);
    let pct_x100 = (diff as u128 * 10_000) / hz16 as u128;
    pct_x100 <= 500 // <= 5% acceptable delta
}

/// Checks complete availability of the nominal CPUID path.
/// Returns true if at least leaf 0x15 or 0x16 can provide a frequency.
pub fn cpuid_nominal_available() -> bool {
    cpuid_tsc_hz().is_some() || cpuid_tsc_hz_leaf16().is_some()
}

/// Returns a human-readable label of the available CPUID source.
pub fn cpuid_source_label() -> &'static str {
    if cpuid_leaf15_available() {
        if let Some(r) = cpuid_leaf15_full() {
            if r.tsc_hz > 0 {
                return "CPUID-0x15";
            }
        }
    }
    if cpuid_leaf16_available() {
        return "CPUID-0x16";
    }
    "CPUID-N/A"
}

/// Returns the extended CPU family (for diagnostics).
pub fn cpu_extended_family() -> u32 {
    read_cpu_family_model().0
}

/// Returns the extended CPU model (for diagnostics).
pub fn cpu_extended_model() -> u32 {
    read_cpu_family_model().1
}
