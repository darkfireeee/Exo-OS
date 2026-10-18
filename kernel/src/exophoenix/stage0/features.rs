//! # exophoenix/stage0/features.rs — Détection et sondage des capacités matérielles CPU/APIC
//!
//! Gère la détection CPUID (x2APIC, PKS, INVPCID, PCID, SMEP, SMAP, invariant TSC, PMC, VMX).

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use crate::arch::x86_64;
use crate::arch::x86_64::cpu::msr;

pub const CR4_VMXE: u64 = 1 << 13;

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootApicMode {
    XApic = 0,
    X2Apic = 1,
}

pub struct BootFeatures {
    apic_mode: AtomicU8,
    pub pks_available: AtomicBool,
    pub invpcid_available: AtomicBool,
    pub invariant_tsc: AtomicBool,
    pub pmc_available: AtomicBool,
    pub pmc_version: AtomicU32,
    pub hpet_available: AtomicBool,
    pub pcid_available: AtomicBool,
    pub smep_available: AtomicBool,
    pub smap_available: AtomicBool,
    pub vmx_active: AtomicBool,
}

impl BootFeatures {
    const fn new() -> Self {
        Self {
            apic_mode: AtomicU8::new(BootApicMode::XApic as u8),
            pks_available: AtomicBool::new(false),
            invpcid_available: AtomicBool::new(false),
            invariant_tsc: AtomicBool::new(false),
            pmc_available: AtomicBool::new(false),
            pmc_version: AtomicU32::new(0),
            hpet_available: AtomicBool::new(false),
            pcid_available: AtomicBool::new(false),
            smep_available: AtomicBool::new(false),
            smap_available: AtomicBool::new(false),
            vmx_active: AtomicBool::new(false),
        }
    }

    #[inline(always)]
    pub fn apic_mode(&self) -> BootApicMode {
        match self.apic_mode.load(Ordering::Acquire) {
            x if x == BootApicMode::X2Apic as u8 => BootApicMode::X2Apic,
            _ => BootApicMode::XApic,
        }
    }

    #[inline(always)]
    pub fn set_apic_mode(&self, mode: BootApicMode) {
        self.apic_mode.store(mode as u8, Ordering::Release);
    }

    #[inline(always)]
    pub fn pmc_available(&self) -> bool {
        self.pmc_available.load(Ordering::Acquire)
    }

    #[inline(always)]
    pub fn pmc_version(&self) -> u32 {
        self.pmc_version.load(Ordering::Acquire)
    }
}

pub static B_FEATURES: BootFeatures = BootFeatures::new();

#[inline(always)]
pub fn cpuid_ex(leaf: u32, subleaf: u32) -> (u32, u32, u32, u32) {
    let (eax, ecx, edx): (u32, u32, u32);
    let ebx_result: u64;
    // SAFETY: CPUID est non-privilégié ; pattern xchg pour préserver rbx.
    unsafe {
        core::arch::asm!(
            "xchg {tmp:r}, rbx",
            "cpuid",
            "xchg {tmp:r}, rbx",
            inout("eax") leaf => eax,
            inout("ecx") subleaf => ecx,
            out("edx") edx,
            tmp = inout(reg) 0u64 => ebx_result,
            options(nostack, nomem),
        );
    }
    (eax, ebx_result as u32, ecx, edx)
}

#[inline(always)]
pub fn cpuid(leaf: u32) -> (u32, u32, u32, u32) {
    cpuid_ex(leaf, 0)
}

/// Détecte le mode APIC actif conformément au plan v4.
/// x2APIC seulement si support CPUID *et* APIC_BASE.bit10 activé.
pub fn detect_apic_mode() -> BootApicMode {
    let (_, _, ecx1, _) = cpuid(1);
    // SAFETY: ring0, MSR standard IA32_APIC_BASE.
    let apic_base = unsafe { msr::read_msr(msr::MSR_IA32_APIC_BASE) };

    let cpuid_x2apic = (ecx1 & (1 << 21)) != 0;
    let apicbase_x2apic = (apic_base & (1 << 10)) != 0;

    if cpuid_x2apic && apicbase_x2apic {
        BootApicMode::X2Apic
    } else {
        BootApicMode::XApic
    }
}

#[inline(always)]
fn detect_pmc_version() -> u32 {
    let (max_basic, _, _, _) = cpuid(0);
    if max_basic < 0xA {
        return 0;
    }
    let (eax_a, _, _, _) = cpuid(0xA);
    eax_a & 0xFF
}

#[inline(always)]
fn detect_invariant_tsc() -> bool {
    let (max_ext, _, _, _) = cpuid(0x8000_0000);
    if max_ext < 0x8000_0007 {
        return false;
    }
    let (_, _, _, edx) = cpuid(0x8000_0007);
    (edx & (1 << 8)) != 0
}

/// Étape 1b : probe global des features B.
pub fn init_feature_probe(hpet_available: bool) {
    let features = crate::arch::x86_64::cpu::features::cpu_features_or_none();

    B_FEATURES.set_apic_mode(detect_apic_mode());
    B_FEATURES.pks_available.store(
        features.map_or(false, |cpu| cpu.has_pks()),
        Ordering::Release,
    );
    B_FEATURES.invpcid_available.store(
        features.map_or(false, |cpu| cpu.has_invpcid()),
        Ordering::Release,
    );
    B_FEATURES.pcid_available.store(
        features.map_or(false, |cpu| cpu.has_pcid()),
        Ordering::Release,
    );
    B_FEATURES.smep_available.store(
        features.map_or(false, |cpu| cpu.has_smep()),
        Ordering::Release,
    );
    B_FEATURES.smap_available.store(
        features.map_or(false, |cpu| cpu.has_smap()),
        Ordering::Release,
    );

    let pmc_version = detect_pmc_version();
    B_FEATURES.pmc_version.store(pmc_version, Ordering::Release);
    B_FEATURES
        .pmc_available
        .store(pmc_version != 0, Ordering::Release);

    B_FEATURES
        .invariant_tsc
        .store(detect_invariant_tsc(), Ordering::Release);
    B_FEATURES
        .hpet_available
        .store(hpet_available, Ordering::Release);

    let cr4 = x86_64::read_cr4();
    let vmx_active = features.map_or(false, |cpu| cpu.has_vmx()) && (cr4 & CR4_VMXE != 0);
    B_FEATURES.vmx_active.store(vmx_active, Ordering::Release);
}

/// Étape 8 : VMXOFF défensif si VT-x actif.
pub fn vmxoff_if_active() {
    if !B_FEATURES.vmx_active.load(Ordering::Acquire) {
        return;
    }

    let cr4 = x86_64::read_cr4();
    if cr4 & CR4_VMXE == 0 {
        B_FEATURES.vmx_active.store(false, Ordering::Release);
        return;
    }

    // SAFETY: instruction privilégiée ring0. On ne panique pas en cas d'échec VMXOFF.
    unsafe {
        core::arch::asm!("vmxoff", options(nostack));
        x86_64::write_cr4(cr4 & !CR4_VMXE);
    }

    B_FEATURES.vmx_active.store(false, Ordering::Release);
}
