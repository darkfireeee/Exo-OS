//! # arch/x86_64/virt/detect.rs — Détection hyperviseur via CPUID
//!
//! La feuille CPUID 0x40000000 est réservée aux hyperviseurs :
//! - EBX/ECX/EDX contiennent la signature ASCII (12 caractères)
//! - Exemples :
//!   - "KVMKVMKVM" = KVM
//!   - "VMwareVMware" = VMware
//!   - "Microsoft Hv" = Hyper-V
//!   - "XenVMMXenVMM" = Xen
//!   - "VBoxVBoxVBox" = VirtualBox

#![allow(dead_code)]

use core::sync::atomic::{AtomicU8, Ordering};

/// Types d'hyperviseurs reconnus
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum HypervisorType {
    None      = 0,
    Kvm       = 1,
    Vmware    = 2,
    HyperV    = 3,
    Xen       = 4,
    Bhyve     = 5,
    VirtualBox= 6,
    Unknown   = 0xFF,
}

static HYPERVISOR: AtomicU8 = AtomicU8::new(0);

/// Retourne le type d'hyperviseur détecté
pub fn hypervisor_type() -> HypervisorType {
    match HYPERVISOR.load(Ordering::Relaxed) {
        0   => HypervisorType::None,
        1   => HypervisorType::Kvm,
        2   => HypervisorType::Vmware,
        3   => HypervisorType::HyperV,
        4   => HypervisorType::Xen,
        5   => HypervisorType::Bhyve,
        6   => HypervisorType::VirtualBox,
        _   => HypervisorType::Unknown,
    }
}

/// Retourne `true` si on tourne sous un hyperviseur
pub fn is_virtual() -> bool {
    hypervisor_type() != HypervisorType::None
}

/// Détecte l'hyperviseur via la feuille CPUID 0x40000000
///
/// Appelé une seule fois au boot depuis `boot::early_init`.
pub fn detect_hypervisor() -> HypervisorType {
    // Vérifier d'abord le bit hypervisor dans CPUID.1.ECX bit 31
    let (_, _, ecx, _) = cpuid(1, 0);
    if ecx & (1 << 31) == 0 {
        HYPERVISOR.store(HypervisorType::None as u8, Ordering::Release);
        return HypervisorType::None;
    }

    // Lire la signature hyperviseur à la feuille 0x40000000
    let (_, ebx, ecx, edx) = cpuid(0x4000_0000, 0);

    let sig = [
        (ebx & 0xFF) as u8, ((ebx >> 8) & 0xFF) as u8, ((ebx >> 16) & 0xFF) as u8, ((ebx >> 24) & 0xFF) as u8,
        (ecx & 0xFF) as u8, ((ecx >> 8) & 0xFF) as u8, ((ecx >> 16) & 0xFF) as u8, ((ecx >> 24) & 0xFF) as u8,
        (edx & 0xFF) as u8, ((edx >> 8) & 0xFF) as u8, ((edx >> 16) & 0xFF) as u8, ((edx >> 24) & 0xFF) as u8,
    ];

    let hv = if &sig[..9] == b"KVMKVMKVM" {
        HypervisorType::Kvm
    } else if &sig[..12] == b"VMwareVMware" {
        HypervisorType::Vmware
    } else if &sig[..12] == b"Microsoft Hv" {
        HypervisorType::HyperV
    } else if &sig[..12] == b"XenVMMXenVMM" {
        HypervisorType::Xen
    } else if &sig[..12] == b"bhyve bhyve " {
        HypervisorType::Bhyve
    } else if &sig[..12] == b"VBoxVBoxVBox" {
        HypervisorType::VirtualBox
    } else {
        HypervisorType::Unknown
    };

    HYPERVISOR.store(hv as u8, Ordering::Release);
    hv
}

// ── CPUID helper ──────────────────────────────────────────────────────────────

#[inline]
fn cpuid(leaf: u32, subleaf: u32) -> (u32, u32, u32, u32) {
    let (eax, _ebx, ecx, edx): (u32, u32, u32, u32);
    let ebx_r: u64;
    // SAFETY: CPUID non-privilégiée; xchg préserve rbx réservé par LLVM.
    unsafe {
        core::arch::asm!(
            "xchg {tmp:r}, rbx",
            "cpuid",
            "xchg {tmp:r}, rbx",
            inout("eax") leaf  => eax,
            inout("ecx") subleaf => ecx,
            out("edx") edx,
            tmp = inout(reg) 0u64 => ebx_r,
            options(nostack, nomem),
        );
    }
    let ebx = ebx_r as u32;
    (eax, ebx, ecx, edx)
}

// ── KVM PV Features ───────────────────────────────────────────────────────────

/// Feuille CPUID KVM pour les features paravirt : 0x40000001
const KVM_CPUID_FEATURES: u32 = 0x4000_0001;

// Bits des features KVM
const KVM_FEATURE_CLOCKSOURCE2:   u32 = 1 << 3;
const KVM_FEATURE_STEAL_TIME:     u32 = 1 << 5;
const KVM_FEATURE_PV_EOI:         u32 = 1 << 6;
const KVM_FEATURE_PV_TLB_FLUSH:   u32 = 1 << 9;

/// Retourne les features KVM disponibles (feuille 0x40000001 EAX)
pub fn kvm_features() -> u32 {
    if hypervisor_type() != HypervisorType::Kvm { return 0; }
    let (eax, _, _, _) = cpuid(KVM_CPUID_FEATURES, 0);
    eax
}

pub fn kvm_has_steal_time()   -> bool { kvm_features() & KVM_FEATURE_STEAL_TIME   != 0 }
pub fn kvm_has_pv_eoi()       -> bool { kvm_features() & KVM_FEATURE_PV_EOI       != 0 }
pub fn kvm_has_pv_tlb_flush() -> bool { kvm_features() & KVM_FEATURE_PV_TLB_FLUSH != 0 }
