// kernel/src/arch/x86_64/time/sources/tsc.rs
//
// ════════════════════════════════════════════════════════════════════════════════
// Source TSC — capacités, invariance, VM-aware, IA32_TSC_ADJUST
// ════════════════════════════════════════════════════════════════════════════════
//
// ## Invariant TSC (RÈGLE TIME-04)
//   TSC invariant = rate constant sur tous les C-states et Turbo boost.
//   Bit 8 d'EDX de CPUID 0x80000007 → InvariantTSC.
//   Rating 400 si invariant, 50 si non-invariant.
//
// ## IA32_TSC_ADJUST MSR (Intel SDM Vol.3 § 17.15)
//   MSR 0x3B = décalage logiciel que le firmware/OS peut ajouter au TSC.
//   La valeur brute = RDTSC + IA32_TSC_ADJUST.
//   À lire au boot pour détecter un décalage non nul (BIOS errata / live migration).
//
// ## IA32_TSC_AUX MSR (0xC0000103)
//   Contient le CPU_ID (core + socket) encodé par l'OS ou le firmware.
//   Accessible via RDTSCP : renvoie (TSC, TSC_AUX), garantie atomique.
//
// ## Détection VM
//   KVM   : CPUID 0x40000001 bit 9 (KVM_FEATURE_CLOCKSOURCE2)
//   Hyper-V : CPUID 0x40000003 bit 20 (HV_FEATURE_TSC_INVARIANT)
//   VMware : CPUID 0x40000010 (VMware Timing Leaf)
//   Generic : CPUID 0x1 ECX bit 31 (Hypervisor Present)
//
// ## RDTSCP
//   Instruction sérialisante (comme LFENCE + RDTSC) + IA32_TSC_AUX → sûr en SMP.
//   Disponible depuis Intel Nehalem / AMD Barcelona.
// ════════════════════════════════════════════════════════════════════════════════

use super::ClockSource;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

// ── MSR addresses ────────────────────────────────────────────────────────────

const MSR_IA32_TSC_ADJUST: u32 = 0x0000_003B;
#[allow(dead_code)]
const MSR_IA32_TSC_AUX: u32 = 0xC000_0103;
/// Max leaf pour CPUID standard (feuille 0).
const CPUID_MAX_STD_LEAF: u32 = 0x0000_0000;
/// CPUID extended features.
#[allow(dead_code)]
const CPUID_EXTENDED_FEAT: u32 = 0x8000_0007;

// ── Capacités TSC ─────────────────────────────────────────────────────────────

/// Capacités complètes du TSC détectées au boot.
#[derive(Debug, Clone, Copy)]
pub struct TscCapabilities {
    /// TSC invariant (CPUID 0x80000007 EDX bit 8).
    pub invariant: bool,
    /// RDTSCP disponible (CPUID 0x80000001 EDX bit 27).
    pub rdtscp: bool,
    /// TSC_DEADLINE disponible (CPUID 0x1 ECX bit 24).
    pub tsc_deadline: bool,
    /// IA32_TSC_ADJUST MSR présent et non nul (= BIOS a décalé le TSC).
    pub tsc_adjust_nonzero: bool,
    /// Valeur de IA32_TSC_ADJUST lue au boot.
    pub tsc_adjust_value: i64,
    /// VM/hyperviseur détecté.
    pub hypervisor: bool,
    /// Type d'hyperviseur.
    pub hypervisor_type: HypervisorType,
}

impl TscCapabilities {
    /// Valeur initiale (tout à false, à initialiser en appelant `detect()`).
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

    /// Détecte toutes les capacités TSC du CPU courant.
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

    /// Rating conditionnel selon les capacités.
    ///   - Invariant + non-hyperviseur → 400
    ///   - Invariant + hyperviseur     → 350 (TSC scaling VM possible)
    ///   - Non-invariant               → 50  (instable avec C-states)
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

// ── Hyperviseur ───────────────────────────────────────────────────────────────

/// Type d'hyperviseur détecté.
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

// ── Globales ──────────────────────────────────────────────────────────────────

/// Capacités TSC initialisées une fois au boot.
static TSC_CAPS_INVARIANT: AtomicBool = AtomicBool::new(false);
static TSC_CAPS_RDTSCP: AtomicBool = AtomicBool::new(false);
static TSC_CAPS_HYPERVISOR: AtomicBool = AtomicBool::new(false);
static TSC_CAPS_RATING: AtomicU32 = AtomicU32::new(0);
static TSC_CAPS_ADJUST: AtomicU64 = AtomicU64::new(0);
/// `true` si `init_tsc_source()` a été appelé.
static TSC_SOURCE_INIT_DONE: AtomicBool = AtomicBool::new(false);

/// Initialise les capacités TSC et met à jour les globales.
/// À appeler une seule fois depuis `time_init()`.
pub fn init_tsc_source() {
    if TSC_SOURCE_INIT_DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    let caps = TscCapabilities::detect();
    TSC_CAPS_INVARIANT.store(caps.invariant, Ordering::Relaxed);
    TSC_CAPS_RDTSCP.store(caps.rdtscp, Ordering::Relaxed);
    TSC_CAPS_HYPERVISOR.store(caps.hypervisor, Ordering::Relaxed);
    TSC_CAPS_RATING.store(caps.rating(), Ordering::Relaxed);
    // IA32_TSC_ADJUST en valeur brute (bit pattern de l'i64).
    TSC_CAPS_ADJUST.store(caps.tsc_adjust_value as u64, Ordering::Relaxed);
}

// ── Source TSC ClockSource ─────────────────────────────────────────────────────

pub struct TscSource;

impl ClockSource for TscSource {
    fn name(&self) -> &'static str {
        "TSC"
    }

    fn rating(&self) -> u32 {
        let r = TSC_CAPS_RATING.load(Ordering::Relaxed);
        if r == 0 {
            // Fallback si init_tsc_source() non encore appelé.
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
    } // TSC toujours présent sur x86_64
}

// ── Lecture TSC ───────────────────────────────────────────────────────────────

/// Lecture TSC via RDTSC (non-sérialisante).
#[inline(always)]
pub fn rdtsc_read() -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: RDTSC non-privilégié sur x86_64.
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

/// Lecture TSC via RDTSCP (sérialisante + IA32_TSC_AUX dans ECX).
/// Garantit que toutes les instructions précédentes sont complètes avant la lecture.
#[inline]
pub fn rdtscp_read() -> u64 {
    let lo: u32;
    let hi: u32;
    let _aux: u32; // TSC_AUX (CPU ID), ignoré ici — utilisé par percpu::sync.
                   // SAFETY: RDTSCP disponible si check_rdtscp() == true.
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

/// Lecture avec LFENCE pré/post pour sérialisation totale (mesure précise).
#[inline]
pub fn rdtsc_serialized() -> u64 {
    unsafe {
        core::arch::asm!("lfence", options(nostack, nomem));
    }
    let t = rdtsc_read();
    unsafe {
        core::arch::asm!("lfence", options(nostack, nomem));
    }
    t
}

// ── Détection des capacités ───────────────────────────────────────────────────

/// Vérifie si le TSC est invariant via CPUID 0x80000007 EDX bit 8.
/// TSC invariant = rate constant à travers tous les C-states et Turbo Boost.
pub fn check_tsc_invariant() -> bool {
    let edx: u32;
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

/// Vérifie si RDTSCP est disponible (CPUID 0x80000001 EDX bit 27).
pub fn check_rdtscp() -> bool {
    let max_ext: u32;
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

/// Vérifie si TSC_DEADLINE est disponible (CPUID 0x1 ECX bit 24).
pub fn check_tsc_deadline() -> bool {
    let ecx: u32;
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

/// Détecte si on s'exécute sous hyperviseur et lequel.
/// Retourne (is_hypervisor, HypervisorType).
pub fn detect_hypervisor() -> (bool, HypervisorType) {
    // Bit 31 de ECX de CPUID 0x1 = Hypervisor Present.
    let ecx: u32;
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

    // Lire la signature hyperviseur depuis la feuille 0x40000000.
    let (ebx, ecx_leaf, edx): (u32, u32, u32);
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

    // KVM : "KVMKVMKVM\0\0\0" en EBX+ECX+EDX
    // Hyper-V : "Microsoft Hv"
    // VMware  : "VMwareVMware"
    let sig = [ebx, ecx_leaf, edx];
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

/// Lit le MSR IA32_TSC_ADJUST (MSR 0x3B).
///
/// Retourne 0 si la lecture échoue (GP fault sur vieux CPU sans ce MSR).
/// Sur les CPU modernes, ce MSR est toujours présent (disponible depuis Ivy Bridge).
pub fn read_tsc_adjust_msr() -> i64 {
    // Vérifier que CPUID 0x7 annonce IA32_TSC_ADJUST (EBX bit 1).
    if !cpuid_tsc_adjust_supported() {
        return 0;
    }

    let lo: u32;
    let hi: u32;
    // SAFETY: RDMSR est privilégié (CPL=0 requis — nous sommes en kernel).
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

/// Écrit IA32_TSC_ADJUST pour neutraliser un décalage BIOS.
/// Uniquement en CPL=0 (kernel). Ne jamais appeler depuis le userespace.
pub fn write_tsc_adjust_msr(value: i64) {
    if !cpuid_tsc_adjust_supported() {
        return;
    }
    let raw = value as u64;
    let lo = raw as u32;
    let hi = (raw >> 32) as u32;
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

/// Vérifie si IA32_TSC_ADJUST est supporté (CPUID 0x7.0 EBX bit 1).
pub fn cpuid_tsc_adjust_supported() -> bool {
    let max_leaf: u32;
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

// ── CPUID leaf 0x15 disponibilité ─────────────────────────────────────────────

/// Vérifie si CPUID leaf 0x15 est disponible pour la calibration nominale.
pub fn cpuid_tsc_available() -> bool {
    let max_leaf: u32;
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

// ── API publique ─────────────────────────────────────────────────────────────

/// Retourne le rating TSC courant (depuis les globales).
pub fn tsc_rating() -> u32 {
    TscSource.rating()
}

/// Retourne `true` si le TSC est invariant (depuis les globales si init fait).
pub fn tsc_is_invariant() -> bool {
    if TSC_SOURCE_INIT_DONE.load(Ordering::Relaxed) {
        TSC_CAPS_INVARIANT.load(Ordering::Relaxed)
    } else {
        check_tsc_invariant()
    }
}

/// Retourne `true` si on est sous hyperviseur.
pub fn tsc_under_hypervisor() -> bool {
    TSC_CAPS_HYPERVISOR.load(Ordering::Relaxed)
}

/// Retourne la valeur de IA32_TSC_ADJUST lue au boot.
pub fn tsc_adjust_value() -> i64 {
    TSC_CAPS_ADJUST.load(Ordering::Relaxed) as i64
}

/// Retourne la fréquence TSC en Hz.
pub fn tsc_freq_hz() -> u64 {
    crate::arch::x86_64::cpu::tsc::tsc_hz()
}
