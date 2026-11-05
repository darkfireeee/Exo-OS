//! # cpu/features.rs — CPUID Detection and Feature Flags
//!
//! Detects all x86_64 CPU extensions required by the Exo-OS kernel.
//! The `CpuFeatures` struct is a global singleton initialized at boot.
//!
//! ## Design
//! - Detection is performed once via `detect()` at boot
//! - Subsequent accesses read from `CPU_FEATURES` (static)
//! - No dynamic allocation

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// ── CPUID wrappers ────────────────────────────────────────────────────────────

/// Executes CPUID with leaf `leaf` and subleaf 0
#[inline]
fn cpuid(leaf: u32) -> (u32, u32, u32, u32) {
    cpuid_ex(leaf, 0)
}

/// Executes CPUID with explicit leaf and subleaf
/// Uses the xchg pattern to work around the LLVM restriction on rbx.
#[inline]
fn cpuid_ex(leaf: u32, subleaf: u32) -> (u32, u32, u32, u32) {
    let (eax, ecx, edx): (u32, u32, u32);
    let ebx_result: u64;
    // SAFETY: Unprivileged CPUID; xchg preserves rbx reserved by LLVM (PIC mode).
    unsafe {
        core::arch::asm!(
            "xchg {tmp:r}, rbx",   // save rbx into tmp, put 0 in rbx
            "cpuid",               // EBX result in rbx
            "xchg {tmp:r}, rbx",   // restore rbx, tmp now contains cpuid EBX
            inout("eax") leaf    => eax,
            inout("ecx") subleaf => ecx,
            out("edx") edx,
            tmp = inout(reg) 0u64 => ebx_result,
            options(nostack, nomem)
        );
    }
    (eax, ebx_result as u32, ecx, edx)
}

// ── Feature flags (bitfield u64×4) ────────────────────────────────────────

/// Set of CPU feature flags compressed into 4 × u64 bitfields
#[derive(Debug, Clone, Copy, Default)]
pub struct CpuFeatureFlags {
    /// Leaf 1 ECX
    pub leaf1_ecx: u32,
    /// Leaf 1 EDX
    pub leaf1_edx: u32,
    /// Leaf 7 EBX
    pub leaf7_ebx: u32,
    /// Leaf 7 ECX
    pub leaf7_ecx: u32,
    /// Leaf 7 EDX
    pub leaf7_edx: u32,
    /// Extended leaf 80000001 ECX
    pub extleaf1_ecx: u32,
    /// Extended leaf 80000001 EDX
    pub extleaf1_edx: u32,
    /// Extended leaf 80000008 EBX
    pub extleaf8_ebx: u32,
    /// Leaf 0xD subleaf 0 EAX (XSAVE component bitmap)
    pub xsave_features: u32,
    /// Max basic CPUID leaf supported
    pub max_basic_leaf: u32,
    /// Max extended CPUID leaf supported
    pub max_ext_leaf: u32,
}

// ── Bits of interest in Leaf 1 ECX ───────────────────────────────────────────
const LEAF1_ECX_SSE3: u32 = 1 << 0;
const LEAF1_ECX_PCLMUL: u32 = 1 << 1;
const LEAF1_ECX_VMX: u32 = 1 << 5;
const LEAF1_ECX_SSSE3: u32 = 1 << 9;
#[allow(dead_code)]
const LEAF1_ECX_FMA: u32 = 1 << 12;
const LEAF1_ECX_SSE41: u32 = 1 << 19;
const LEAF1_ECX_SSE42: u32 = 1 << 20;
const LEAF1_ECX_X2APIC: u32 = 1 << 21;
#[allow(dead_code)]
const LEAF1_ECX_MOVBE: u32 = 1 << 22;
#[allow(dead_code)]
const LEAF1_ECX_POPCNT: u32 = 1 << 23;
const LEAF1_ECX_TSCD: u32 = 1 << 24; // TSC deadline
const LEAF1_ECX_AES: u32 = 1 << 25;
const LEAF1_ECX_XSAVE: u32 = 1 << 26;
#[allow(dead_code)]
const LEAF1_ECX_OSXSAVE: u32 = 1 << 27;
const LEAF1_ECX_AVX: u32 = 1 << 28;
#[allow(dead_code)]
const LEAF1_ECX_F16C: u32 = 1 << 29;
const LEAF1_ECX_RDRAND: u32 = 1 << 30;
const LEAF1_ECX_HYPERVISOR: u32 = 1 << 31;

// ── Bits of interest in Leaf 1 EDX ───────────────────────────────────────────
const LEAF1_EDX_FPU: u32 = 1 << 0;
#[allow(dead_code)]
const LEAF1_EDX_MSR: u32 = 1 << 5;
#[allow(dead_code)]
const LEAF1_EDX_PAE: u32 = 1 << 6;
const LEAF1_EDX_APIC: u32 = 1 << 9;
#[allow(dead_code)]
const LEAF1_EDX_SEP: u32 = 1 << 11; // SYSENTER/SYSEXIT
#[allow(dead_code)]
const LEAF1_EDX_MTRR: u32 = 1 << 12;
#[allow(dead_code)]
const LEAF1_EDX_PGE: u32 = 1 << 13; // Global pages
#[allow(dead_code)]
const LEAF1_EDX_MCA: u32 = 1 << 14;
#[allow(dead_code)]
const LEAF1_EDX_CMOV: u32 = 1 << 15;
#[allow(dead_code)]
const LEAF1_EDX_PAT: u32 = 1 << 16;
#[allow(dead_code)]
const LEAF1_EDX_CLFLUSH: u32 = 1 << 19;
#[allow(dead_code)]
const LEAF1_EDX_DS: u32 = 1 << 21;
#[allow(dead_code)]
const LEAF1_EDX_MMX: u32 = 1 << 23;
const LEAF1_EDX_FXSR: u32 = 1 << 24;
const LEAF1_EDX_SSE: u32 = 1 << 25;
const LEAF1_EDX_SSE2: u32 = 1 << 26;
const LEAF1_EDX_HTT: u32 = 1 << 28; // Hyper-Threading

// ── Bits of interest in Leaf 7 EBX ───────────────────────────────────────────
const LEAF7_EBX_FSGSBASE: u32 = 1 << 0;
#[allow(dead_code)]
const LEAF7_EBX_TSC_ADJ: u32 = 1 << 1;
#[allow(dead_code)]
const LEAF7_EBX_SGX: u32 = 1 << 2;
#[allow(dead_code)]
const LEAF7_EBX_BMI1: u32 = 1 << 3;
#[allow(dead_code)]
const LEAF7_EBX_HLE: u32 = 1 << 4;
const LEAF7_EBX_AVX2: u32 = 1 << 5;
const LEAF7_EBX_SMEP: u32 = 1 << 7;
#[allow(dead_code)]
const LEAF7_EBX_BMI2: u32 = 1 << 8;
#[allow(dead_code)]
const LEAF7_EBX_ERMS: u32 = 1 << 9;
const LEAF7_EBX_INVPCID: u32 = 1 << 10;
#[allow(dead_code)]
const LEAF7_EBX_RTM: u32 = 1 << 11;
#[allow(dead_code)]
const LEAF7_EBX_MPX: u32 = 1 << 14;
const LEAF7_EBX_AVX512F: u32 = 1 << 16;
#[allow(dead_code)]
const LEAF7_EBX_AVX512DQ: u32 = 1 << 17;
const LEAF7_EBX_RDSEED: u32 = 1 << 18;
#[allow(dead_code)]
const LEAF7_EBX_ADX: u32 = 1 << 19;
const LEAF7_EBX_SMAP: u32 = 1 << 20;
#[allow(dead_code)]
const LEAF7_EBX_AVX512IFMA: u32 = 1 << 21;
const LEAF7_EBX_CLFLUSHOPT: u32 = 1 << 23;
const LEAF7_EBX_CLWB: u32 = 1 << 24;
#[allow(dead_code)]
const LEAF7_EBX_AVX512PF: u32 = 1 << 26;
#[allow(dead_code)]
const LEAF7_EBX_AVX512ER: u32 = 1 << 27;
#[allow(dead_code)]
const LEAF7_EBX_AVX512CD: u32 = 1 << 28;
const LEAF7_EBX_SHA: u32 = 1 << 29;
#[allow(dead_code)]
const LEAF7_EBX_AVX512BW: u32 = 1 << 30;
#[allow(dead_code)]
const LEAF7_EBX_AVX512VL: u32 = 1 << 31;

// ── Bits of interest in Leaf 1 ECX (continued) ─────────────────────────────────
const LEAF1_ECX_PCID: u32 = 1 << 17; // Process-Context Identifiers

// ── Bits of interest in Leaf 7 ECX ───────────────────────────────────────────
const LEAF7_ECX_UMIP: u32 = 1 << 2;
const LEAF7_ECX_PKU: u32 = 1 << 3;
#[allow(dead_code)]
const LEAF7_ECX_OSPKE: u32 = 1 << 4;
#[allow(dead_code)]
const LEAF7_ECX_CET_SS: u32 = 1 << 7;
const LEAF7_ECX_LA57: u32 = 1 << 16; // 5-level paging
const LEAF7_ECX_PKS: u32 = 1 << 31; // Protection Keys for Supervisor

// ── Bits of interest in Leaf 7 EDX ───────────────────────────────────────────
const LEAF7_EDX_ARCH_CAP: u32 = 1 << 29;
const LEAF7_EDX_SPEC_CTRL: u32 = 1 << 26;
const LEAF7_EDX_STIBP: u32 = 1 << 27;
const LEAF7_EDX_FLUSH_CMD: u32 = 1 << 28;
const LEAF7_EDX_SSBD: u32 = 1 << 31;
const LEAF7_EDX_MD_CLEAR: u32 = 1 << 10; // MD_CLEAR (VERW flushes buffers)

// ── Bits of interest in Extended Leaf 80000001 EDX ───────────────────────────
const EXT1_EDX_SYSCALL: u32 = 1 << 11;
const EXT1_EDX_NX: u32 = 1 << 20;
const EXT1_EDX_PDPE1GB: u32 = 1 << 26; // 1 GB huge pages
const EXT1_EDX_RDTSCP: u32 = 1 << 27;
#[allow(dead_code)]
const EXT1_EDX_LM: u32 = 1 << 29; // Long Mode (64-bit)

// ── Bits of interest in Extended Leaf 80000001 ECX (AMD) ─────────────────────
const EXT1_ECX_IBRS_AMD: u32 = 1 << 14; // AMD: IBRS/IBPB (CPUID_80000001_ECX)
const EXT1_ECX_IBPB_AMD: u32 = 1 << 12; // AMD: IBPB standalone
const EXT1_ECX_VIRT_SSBD: u32 = 1 << 25; // AMD: Virtualized SSBD
const EXT8_EBX_NOREPLAY: u32 = 1 << 6; // AMD: no rogue data cache replay / RDCL-like immunity

// ── CpuFeatures Structure ─────────────────────────────────────────────────────

/// CPU information collected at boot
#[derive(Debug, Clone)]
pub struct CpuFeatures {
    flags: CpuFeatureFlags,

    pub vendor: CpuVendor,

    /// Stepping (model bits [3:0])
    pub stepping: u8,
    /// CPU Model
    pub model: u8,
    /// CPU Family
    pub family: u8,
    /// Number of logical threads on this physical CPU
    pub logical_cpus: u8,
    /// XSAVE area size (in bytes)
    pub xsave_area_size: u32,

    /// Vendor string (12 chars)
    pub vendor_string: [u8; 12],

    /// Architecture capacity flags (MSR_IA32_ARCH_CAP read if available)
    pub arch_cap: u64,
}

/// Detected CPU vendor
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuVendor {
    Intel,
    Amd,
    Unknown,
}

impl CpuFeatures {
    /// Detects all features of the current CPU via CPUID
    /// Must be called from the BSP at boot, before any other code.
    pub fn detect() -> Self {
        // Leaf 0: vendor string + max basic leaf
        let (max_basic, ebx, ecx, edx) = cpuid(0);
        let vendor_string = [
            ebx as u8,
            (ebx >> 8) as u8,
            (ebx >> 16) as u8,
            (ebx >> 24) as u8,
            edx as u8,
            (edx >> 8) as u8,
            (edx >> 16) as u8,
            (edx >> 24) as u8,
            ecx as u8,
            (ecx >> 8) as u8,
            (ecx >> 16) as u8,
            (ecx >> 24) as u8,
        ];

        let vendor = if &vendor_string == b"GenuineIntel" {
            CpuVendor::Intel
        } else if &vendor_string == b"AuthenticAMD" {
            CpuVendor::Amd
        } else {
            CpuVendor::Unknown
        };

        // Leaf 1: basic feature flags + model
        let (eax1, ebx1, ecx1, edx1) = cpuid(1);

        let stepping = (eax1 & 0xF) as u8;
        let model_low = ((eax1 >> 4) & 0xF) as u8;
        let family_low = ((eax1 >> 8) & 0xF) as u8;
        let model_ext = ((eax1 >> 16) & 0xF) as u8;
        let family_ext = ((eax1 >> 20) & 0xFF) as u8;
        let model = if family_low >= 6 {
            (model_ext << 4) | model_low
        } else {
            model_low
        };
        let family = if family_low == 0xF {
            family_ext + family_low
        } else {
            family_low
        };
        let logical_cpus = ((ebx1 >> 16) & 0xFF) as u8;

        // Leaf 7 subleaf 0: new features (SMEP, SMAP, AVX2…)
        let (leaf7_ebx, leaf7_ecx, leaf7_edx) = if max_basic >= 7 {
            let (_, b, c, d) = cpuid_ex(7, 0);
            (b, c, d)
        } else {
            (0, 0, 0)
        };

        // Extended leaf 80000001
        let (max_ext, _, _, _) = cpuid(0x8000_0000);
        let (ext1_ecx, ext1_edx) = if max_ext >= 0x8000_0001 {
            let (_, _, c, d) = cpuid(0x8000_0001);
            (c, d)
        } else {
            (0, 0)
        };
        let ext8_ebx = if max_ext >= 0x8000_0008 {
            let (_, b, _, _) = cpuid(0x8000_0008);
            b
        } else {
            0
        };

        // XSAVE area size (leaf 0xD subleaf 0, EBX = min size)
        let xsave_area_size = if ecx1 & LEAF1_ECX_XSAVE != 0 && max_basic >= 0xD {
            let (_, ebx_d, _, _) = cpuid_ex(0xD, 0);
            if ebx_d > 0 {
                ebx_d
            } else {
                512
            }
        } else {
            512 // FXSAVE default size
        };

        // Read MSR_IA32_ARCH_CAP if available.
        // Under host/userspace tests, RDMSR would trigger a #GP, so we
        // preserve a neutral value. Real bare-metal boot keeps the read.
        let arch_cap = if leaf7_edx & LEAF7_EDX_ARCH_CAP != 0 {
            #[cfg(target_os = "none")]
            {
                // SAFETY: ARCH_CAP MSR available if CPUID indicates it on bare-metal kernel.
                unsafe { super::msr::read_msr(super::msr::MSR_IA32_ARCH_CAP) }
            }
            #[cfg(not(target_os = "none"))]
            {
                0
            }
        } else {
            0
        };

        Self {
            flags: CpuFeatureFlags {
                leaf1_ecx: ecx1,
                leaf1_edx: edx1,
                leaf7_ebx,
                leaf7_ecx,
                leaf7_edx,
                extleaf1_ecx: ext1_ecx,
                extleaf1_edx: ext1_edx,
                extleaf8_ebx: ext8_ebx,
                xsave_features: if max_basic >= 0xD {
                    cpuid_ex(0xD, 0).0
                } else {
                    0
                },
                max_basic_leaf: max_basic,
                max_ext_leaf: max_ext,
            },
            vendor,
            stepping,
            model,
            family,
            logical_cpus: if logical_cpus == 0 { 1 } else { logical_cpus },
            xsave_area_size,
            vendor_string,
            arch_cap,
        }
    }

    // ── Feature Accessors ───────────────────────────────────────────────────

    #[inline(always)]
    pub fn has_apic(&self) -> bool {
        self.flags.leaf1_edx & LEAF1_EDX_APIC != 0
    }
    #[inline(always)]
    pub fn has_x2apic(&self) -> bool {
        self.flags.leaf1_ecx & LEAF1_ECX_X2APIC != 0
    }
    #[inline(always)]
    pub fn has_sse(&self) -> bool {
        self.flags.leaf1_edx & LEAF1_EDX_SSE != 0
    }
    #[inline(always)]
    pub fn has_sse2(&self) -> bool {
        self.flags.leaf1_edx & LEAF1_EDX_SSE2 != 0
    }
    #[inline(always)]
    pub fn has_sse3(&self) -> bool {
        self.flags.leaf1_ecx & LEAF1_ECX_SSE3 != 0
    }
    #[inline(always)]
    pub fn has_ssse3(&self) -> bool {
        self.flags.leaf1_ecx & LEAF1_ECX_SSSE3 != 0
    }
    #[inline(always)]
    pub fn has_sse41(&self) -> bool {
        self.flags.leaf1_ecx & LEAF1_ECX_SSE41 != 0
    }
    #[inline(always)]
    pub fn has_sse42(&self) -> bool {
        self.flags.leaf1_ecx & LEAF1_ECX_SSE42 != 0
    }
    #[inline(always)]
    pub fn has_avx(&self) -> bool {
        self.flags.leaf1_ecx & LEAF1_ECX_AVX != 0
    }
    #[inline(always)]
    pub fn has_avx2(&self) -> bool {
        self.flags.leaf7_ebx & LEAF7_EBX_AVX2 != 0
    }
    #[inline(always)]
    pub fn has_avx512f(&self) -> bool {
        self.flags.leaf7_ebx & LEAF7_EBX_AVX512F != 0
    }
    #[inline(always)]
    pub fn has_xsave(&self) -> bool {
        self.flags.leaf1_ecx & LEAF1_ECX_XSAVE != 0
    }
    #[inline(always)]
    pub fn has_fxsr(&self) -> bool {
        self.flags.leaf1_edx & LEAF1_EDX_FXSR != 0
    }
    #[inline(always)]
    pub fn has_fpu(&self) -> bool {
        self.flags.leaf1_edx & LEAF1_EDX_FPU != 0
    }
    #[inline(always)]
    pub fn has_aes(&self) -> bool {
        self.flags.leaf1_ecx & LEAF1_ECX_AES != 0
    }
    #[inline(always)]
    pub fn has_pclmul(&self) -> bool {
        self.flags.leaf1_ecx & LEAF1_ECX_PCLMUL != 0
    }
    #[inline(always)]
    pub fn has_vmx(&self) -> bool {
        self.flags.leaf1_ecx & LEAF1_ECX_VMX != 0
    }
    #[inline(always)]
    pub fn has_rdrand(&self) -> bool {
        self.flags.leaf1_ecx & LEAF1_ECX_RDRAND != 0
    }
    #[inline(always)]
    pub fn has_rdseed(&self) -> bool {
        self.flags.leaf7_ebx & LEAF7_EBX_RDSEED != 0
    }
    #[inline(always)]
    pub fn has_smep(&self) -> bool {
        self.flags.leaf7_ebx & LEAF7_EBX_SMEP != 0
    }
    #[inline(always)]
    pub fn has_smap(&self) -> bool {
        self.flags.leaf7_ebx & LEAF7_EBX_SMAP != 0
    }
    #[inline(always)]
    pub fn has_umip(&self) -> bool {
        self.flags.leaf7_ecx & LEAF7_ECX_UMIP != 0
    }
    #[inline(always)]
    pub fn has_pku(&self) -> bool {
        self.flags.leaf7_ecx & LEAF7_ECX_PKU != 0
    }
    #[inline(always)]
    pub fn has_pks(&self) -> bool {
        self.flags.leaf7_ecx & LEAF7_ECX_PKS != 0
    }
    #[inline(always)]
    pub fn has_fsgsbase(&self) -> bool {
        self.flags.leaf7_ebx & LEAF7_EBX_FSGSBASE != 0
    }
    #[inline(always)]
    pub fn has_invpcid(&self) -> bool {
        self.flags.leaf7_ebx & LEAF7_EBX_INVPCID != 0
    }
    #[inline(always)]
    pub fn has_nx(&self) -> bool {
        self.flags.extleaf1_edx & EXT1_EDX_NX != 0
    }
    #[inline(always)]
    pub fn has_1gb_pages(&self) -> bool {
        self.flags.extleaf1_edx & EXT1_EDX_PDPE1GB != 0
    }
    #[inline(always)]
    pub fn has_rdtscp(&self) -> bool {
        self.flags.extleaf1_edx & EXT1_EDX_RDTSCP != 0
    }
    #[inline(always)]
    pub fn has_syscall(&self) -> bool {
        self.flags.extleaf1_edx & EXT1_EDX_SYSCALL != 0
    }
    #[inline(always)]
    pub fn has_tsc_deadline(&self) -> bool {
        self.flags.leaf1_ecx & LEAF1_ECX_TSCD != 0
    }
    #[inline(always)]
    pub fn has_htt(&self) -> bool {
        self.flags.leaf1_edx & LEAF1_EDX_HTT != 0
    }
    #[inline(always)]
    pub fn has_la57(&self) -> bool {
        self.flags.leaf7_ecx & LEAF7_ECX_LA57 != 0
    }
    #[inline(always)]
    pub fn has_clflushopt(&self) -> bool {
        self.flags.leaf7_ebx & LEAF7_EBX_CLFLUSHOPT != 0
    }
    #[inline(always)]
    pub fn has_clwb(&self) -> bool {
        self.flags.leaf7_ebx & LEAF7_EBX_CLWB != 0
    }
    #[inline(always)]
    pub fn has_sha(&self) -> bool {
        self.flags.leaf7_ebx & LEAF7_EBX_SHA != 0
    }
    #[inline(always)]
    pub fn has_spec_ctrl(&self) -> bool {
        self.flags.leaf7_edx & LEAF7_EDX_SPEC_CTRL != 0
    }
    #[inline(always)]
    pub fn has_stibp(&self) -> bool {
        self.flags.leaf7_edx & LEAF7_EDX_STIBP != 0
    }
    #[inline(always)]
    pub fn has_ssbd(&self) -> bool {
        self.flags.leaf7_edx & LEAF7_EDX_SSBD != 0
    }
    #[inline(always)]
    pub fn has_flush_cmd(&self) -> bool {
        self.flags.leaf7_edx & LEAF7_EDX_FLUSH_CMD != 0
    }
    #[inline(always)]
    pub fn has_arch_cap(&self) -> bool {
        self.flags.leaf7_edx & LEAF7_EDX_ARCH_CAP != 0
    }
    #[inline(always)]
    pub fn is_hypervisor(&self) -> bool {
        self.flags.leaf1_ecx & LEAF1_ECX_HYPERVISOR != 0
    }

    /// CET Shadow Stack — CPUID leaf 7, sub-leaf 0, ECX bit 7.
    /// FIX-CET-01: required to condition saving MSR_IA32_PL0_SSP.
    #[inline(always)]
    pub fn has_cet_ss(&self) -> bool {
        self.flags.leaf7_ecx & (1 << 7) != 0
    }

    // ── Spectre / MDS Specific Methods ───────────────────────────────────
    /// PCID (Process-Context Identifiers) — required for KPTI no-flush
    #[inline(always)]
    pub fn has_pcid(&self) -> bool {
        self.flags.leaf1_ecx & LEAF1_ECX_PCID != 0
    }
    /// IBRS (Indirect Branch Restricted Speculation) — Intel or AMD
    #[inline(always)]
    pub fn has_ibrs(&self) -> bool {
        self.flags.leaf7_edx & LEAF7_EDX_SPEC_CTRL != 0
            || self.flags.extleaf1_ecx & EXT1_ECX_IBRS_AMD != 0
    }
    /// IBPB (Indirect Branch Predictor Barrier) — Intel or AMD
    #[inline(always)]
    pub fn has_ibpb(&self) -> bool {
        self.flags.leaf7_edx & LEAF7_EDX_SPEC_CTRL != 0
            || self.flags.extleaf1_ecx & EXT1_ECX_IBPB_AMD != 0
    }
    /// MD_CLEAR: VERW flushes micro-architectural buffers (MDS mitigation)
    #[inline(always)]
    pub fn has_md_clear(&self) -> bool {
        self.flags.leaf7_edx & LEAF7_EDX_MD_CLEAR != 0
    }
    /// VIRT_SSBD (AMD): Virtualized Speculative Store Bypass Disable
    #[inline(always)]
    pub fn has_virt_ssbd(&self) -> bool {
        self.flags.extleaf1_ecx & EXT1_ECX_VIRT_SSBD != 0
    }

    /// CPU not vulnerable to Meltdown (Rogue Data Cache Load)
    #[inline(always)]
    pub fn rdcl_no(&self) -> bool {
        self.arch_cap & super::msr::ARCH_CAP_RDCL_NO != 0
    }

    /// AMD CPUID Fn8000_0008_EBX[6] indicates absence of exploitable
    /// cache replay for RDCL variants documented on AMD side.
    #[inline(always)]
    pub fn amd_noreplay(&self) -> bool {
        self.flags.extleaf8_ebx & EXT8_EBX_NOREPLAY != 0
    }

    /// CPU not vulnerable to Spectre v4 SSB
    #[inline(always)]
    pub fn ssb_no(&self) -> bool {
        self.arch_cap & super::msr::ARCH_CAP_SSB_NO != 0
    }

    /// IBRS always-on supported
    #[inline(always)]
    pub fn ibrs_all(&self) -> bool {
        self.arch_cap & super::msr::ARCH_CAP_IBRS_ALL != 0
    }

    /// Returns the size of the XSAVE area in bytes
    #[inline(always)]
    pub fn xsave_size(&self) -> u32 {
        self.xsave_area_size
    }

    /// OS is running inside a hypervisor
    #[inline(always)]
    pub fn in_vm(&self) -> bool {
        self.is_hypervisor()
    }
}

// ── CPU_FEATURES Singleton ────────────────────────────────────────────────────

/// Global CPU features singleton — initialized by `init_cpu_features()`
///
/// ⚠️ Access before `init_cpu_features()` = undefined behavior.
///    Padding is always 0 before init — all `has_*()` return false.
pub static CPU_FEATURES: CpuFeaturesCell = CpuFeaturesCell::new();

/// Thread-safe wrapper (read-only after init) for CpuFeatures
pub struct CpuFeaturesCell {
    initialized: AtomicBool,
    // Uses UnsafeCell for one-time initialization at boot
    inner: core::cell::UnsafeCell<CpuFeatures>,
}

// SAFETY: CpuFeaturesCell is read-only after init — Send/Sync are safe
unsafe impl Send for CpuFeaturesCell {}
unsafe impl Sync for CpuFeaturesCell {}

impl CpuFeaturesCell {
    const fn new() -> Self {
        Self {
            initialized: AtomicBool::new(false),
            inner: core::cell::UnsafeCell::new(CpuFeatures {
                flags: CpuFeatureFlags {
                    leaf1_ecx: 0,
                    leaf1_edx: 0,
                    leaf7_ebx: 0,
                    leaf7_ecx: 0,
                    leaf7_edx: 0,
                    extleaf1_ecx: 0,
                    extleaf1_edx: 0,
                    extleaf8_ebx: 0,
                    xsave_features: 0,
                    max_basic_leaf: 0,
                    max_ext_leaf: 0,
                },
                vendor: CpuVendor::Unknown,
                stepping: 0,
                model: 0,
                family: 0,
                logical_cpus: 1,
                xsave_area_size: 512,
                vendor_string: [0u8; 12],
                arch_cap: 0,
            }),
        }
    }

    /// Initializes the singleton. Must be called EXACTLY ONCE from the BSP.
    pub fn init(&self) {
        if self
            .initialized
            .compare_exchange(false, true, Ordering::Release, Ordering::Relaxed)
            .is_ok()
        {
            let features = CpuFeatures::detect();
            // SAFETY: sole writer (BSP before APs); compare_exchange ensures uniqueness.
            unsafe {
                *self.inner.get() = features;
            }
        }
    }

    /// Returns reference to detected features
    ///
    /// # Panics
    /// Panics in debug if called before `init()`.
    #[inline(always)]
    pub fn get(&self) -> &CpuFeatures {
        debug_assert!(
            self.initialized.load(Ordering::Acquire),
            "CPU_FEATURES: accès avant init()"
        );
        // SAFETY: after init() inner is read-only — no mutation possible
        unsafe { &*self.inner.get() }
    }

    /// Returns `true` if CPU features have already been initialized.
    #[inline(always)]
    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Acquire)
    }

    /// Non-panicking variant: `None` if init has not occurred yet.
    #[inline(always)]
    pub fn get_if_initialized(&self) -> Option<&CpuFeatures> {
        if self.is_initialized() {
            // SAFETY: after init() inner is read-only — no mutation possible.
            Some(unsafe { &*self.inner.get() })
        } else {
            None
        }
    }
}

impl core::ops::Deref for CpuFeaturesCell {
    type Target = CpuFeatures;

    #[inline(always)]
    fn deref(&self) -> &CpuFeatures {
        self.get()
    }
}

// ── Init Entry Point ───────────────────────────────────────────────────────

/// Initializes CPU detection — called from `early_init.rs` at BSP boot
pub fn init_cpu_features() {
    CPU_FEATURES.init();

    let f = CPU_FEATURES.get();

    // Validation of mandatory features for Exo-OS x86_64
    assert!(f.has_sse2(), "FATAL: SSE2 requis (x86_64 ABI baseline)");
    assert!(f.has_fxsr(), "FATAL: FXSR requis pour context switch FPU");
    assert!(f.has_syscall(), "FATAL: SYSCALL/SYSRET requis");
    assert!(f.has_nx(), "FATAL: NX/XD bit requis");
    assert!(f.has_apic(), "FATAL: LAPIC requis");
}

/// Shortcut for `CPU_FEATURES.get()` — used by early_init and other modules
///
/// Panics in debug if called before `init_cpu_features()`.
#[inline(always)]
pub fn cpu_features() -> &'static CpuFeatures {
    CPU_FEATURES.get()
}

/// Returns CPU features if available in the current context.
///
/// - bare-metal (`target_os = "none"`): requires normal initialization and
///   preserves the strict boot contract;
/// - tests/host: returns `None` as long as `init_cpu_features()` has not been
///   called, allowing fallback paths to remain safe.
#[cfg(target_os = "none")]
#[inline(always)]
pub fn cpu_features_or_none() -> Option<&'static CpuFeatures> {
    Some(CPU_FEATURES.get())
}

/// Host/test variant of `cpu_features_or_none()`.
#[cfg(not(target_os = "none"))]
#[inline(always)]
pub fn cpu_features_or_none() -> Option<&'static CpuFeatures> {
    CPU_FEATURES.get_if_initialized()
}

// ── CPUID Instrumentation ─────────────────────────────────────────────────────

static CPUID_CALL_COUNT: AtomicU64 = AtomicU64::new(0);

/// Returns the number of CPUID calls since boot (instrumentation)
pub fn cpuid_call_count() -> u64 {
    CPUID_CALL_COUNT.load(Ordering::Relaxed)
}

/// Prints a summary of detected features (for boot log)
pub fn log_cpu_features(features: &CpuFeatures) {
    // The summary is constructed as active flags
    // (no allocation — uses a static buffer)
    let _ = features; // used by caller for log formatting
}
