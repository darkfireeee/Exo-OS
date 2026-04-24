//! ExoPhoenix Stage 0 — bootstrap Kernel-B (étapes 1→7 du plan v4).
//!
//! Objectif ici : implémenter strictement la séquence demandée pour 3.3a :
//! 1) install page tables B
//! 0.5) probe CPUID global
//! 2) stack B + guard page !PRESENT
//! 3) init TSS (init_b_tss)
//! 4) IDT stubs (vecteurs ExoPhoenix inactifs)
//! 5) parse ACPI (MADT/FADT/FACS)
//! 5.5) enum PCI + calcul taille pool R3
//! 6) build apic_to_slot[256] depuis MADT réel
//! 7) calibration APIC timer via PIT ch2 -> TICKS_PER_US

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU64, AtomicU8, Ordering};

use crate::arch::x86_64;
use crate::arch::x86_64::acpi::{madt, parser};
use crate::arch::x86_64::apic::local_apic;
use crate::arch::x86_64::apic::{ipi, x2apic};
use crate::arch::x86_64::cpu::{msr, tsc};
use crate::arch::x86_64::smp::init::TRAMPOLINE_PAGE;
use crate::arch::x86_64::time::sources::pit;
use crate::exophoenix::{sentinel, PhoenixState, PHOENIX_STATE};
use crate::fs::exofs::core::types::BlobId;
use crate::memory::core::{AllocFlags, PageFlags, VirtAddr, PAGE_SIZE};
use crate::memory::dma::iommu::domain::{DomainType, PciBdf, IOMMU_DOMAINS};
use crate::memory::dma::iommu::{AMD_IOMMU, IDENTITY_DOMAIN_ID, INTEL_VTD};
use crate::memory::integrity::{register_guard_region, GuardRegionKind};
use crate::memory::physical::allocator::buddy;
use crate::memory::virt::address_space::kernel::KERNEL_AS;
use crate::memory::virt::address_space::tlb;
use crate::memory::virt::page_table::{PageTableWalker, WalkResult};
use crate::security::crypto::blake3::blake3_hash;

const CR4_VMXE: u64 = 1 << 13;
const APIC_X2APIC_MSR_BASE: u32 = 0x800;
const WATCHDOG_DEFAULT_MS: u64 = 5_000;
const CORE_A_SLOT: u8 = 0;
const A_ENTRY_VECTOR: u8 = TRAMPOLINE_PAGE;
const SIPI_SENT_BIT: u64 = 1;
const ACPI_MAX_TABLE_LEN: usize = 256 * 1024;

const STAGE0_B_REGION_PHYS_BASE: u64 = crate::memory::core::layout::KERNEL_LOAD_PHYS_ADDR;
const STAGE0_B_REGION_PHYS_SIZE: u64 = crate::memory::core::layout::KERNEL_IMAGE_MAX_SIZE as u64;

const MAX_B_DEVICES: usize = 256;
const B_STACK_GUARD_SIZE: usize = 4096;
const B_STACK_SIZE: usize = 64 * 1024;

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootApicMode {
    XApic = 0,
    X2Apic = 1,
}

#[derive(Clone, Copy, Debug)]
pub struct Stage0AcpiSnapshot {
    pub madt_phys: u64,
    pub fadt_phys: u64,
    pub facs_phys: u64,
    pub hpet_phys: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct Stage0Summary {
    pub b_cr3: u64,
    pub b_stack_top: u64,
    pub acpi: Stage0AcpiSnapshot,
    pub pci_device_count: usize,
    pub pool_r3_size: u64,
    pub apic_slots_mapped: usize,
    pub ticks_per_us: u64,
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

/// Table APIC ID (0..255) -> slot SSR (0..63), 0xFF = non assigné.
static APIC_TO_SLOT: [AtomicU8; 256] = [const { AtomicU8::new(0xFF) }; 256];

/// Les vecteurs 0xF1/0xF2/0xF3 ne sont activés qu'une fois Stage 0 prêt.
static EXOPHOENIX_VECTORS_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Résultats stage0 (1..7)
static B_STAGE0_CR3: AtomicU64 = AtomicU64::new(0);
static B_STACK_TOP: AtomicU64 = AtomicU64::new(0);
static B_STACK_GUARD_VA: AtomicU64 = AtomicU64::new(0);
static POOL_R3_SIZE_BYTES: AtomicU64 = AtomicU64::new(0);
static POOL_R3_BASE_PHYS: AtomicU64 = AtomicU64::new(0);
static POOL_R3_ALLOC_BYTES: AtomicU64 = AtomicU64::new(0);
static POOL_R3_ALLOC_ORDER: AtomicU8 = AtomicU8::new(0);

/// Étape 7 : ticks APIC par microseconde (calibré via PIT ch2).
pub static TICKS_PER_US: AtomicU64 = AtomicU64::new(0);
static WATCHDOG_ARMED_MS: AtomicU64 = AtomicU64::new(0);
static WATCHDOG_ARMED_TICKS: AtomicU64 = AtomicU64::new(0);

/// Étape 9 : policy IOMMU Stage0 (deny-by-default + zones protégées).
static IOMMU_POLICY_READY: AtomicBool = AtomicBool::new(false);
static IOMMU_BLOCKED_DOMAIN_ID: AtomicU32 = AtomicU32::new(0);
static IOMMU_ACS_ROOT_PORTS: AtomicU32 = AtomicU32::new(0);
static IOMMU_PROTECT_B_BASE: AtomicU64 = AtomicU64::new(0);
static IOMMU_PROTECT_B_SIZE: AtomicU64 = AtomicU64::new(0);
static IOMMU_PROTECT_SSR_BASE: AtomicU64 = AtomicU64::new(0);
static IOMMU_PROTECT_SSR_SIZE: AtomicU64 = AtomicU64::new(0);
static IOMMU_PROTECT_POOL_R3_BASE: AtomicU64 = AtomicU64::new(0);
static IOMMU_PROTECT_POOL_R3_SIZE: AtomicU64 = AtomicU64::new(0);

/// Étape 10 : FACS RO + hash MADT.
static FACS_RO_MARKED: AtomicBool = AtomicBool::new(false);
static MADT_HASH_QWORDS: [AtomicU64; 4] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];

/// Étape 13 : garde-fou anti double SIPI (G8).
static SIPI_SENT: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct PciDeviceRecord {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub header_type: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class_code: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub bar_bitmap: u8,
}

const EMPTY_PCI_DEVICE: PciDeviceRecord = PciDeviceRecord {
    bus: 0,
    device: 0,
    function: 0,
    header_type: 0,
    vendor_id: 0,
    device_id: 0,
    class_code: 0,
    subclass: 0,
    prog_if: 0,
    bar_bitmap: 0,
};

struct DeviceTableCell(UnsafeCell<[PciDeviceRecord; MAX_B_DEVICES]>);
unsafe impl Sync for DeviceTableCell {}

struct DriverBlobTableCell(UnsafeCell<[BlobId; MAX_B_DEVICES]>);
unsafe impl Sync for DriverBlobTableCell {}

static B_DEVICE_TABLE: DeviceTableCell =
    DeviceTableCell(UnsafeCell::new([EMPTY_PCI_DEVICE; MAX_B_DEVICES]));
static B_DRIVER_BLOB_TABLE: DriverBlobTableCell =
    DriverBlobTableCell(UnsafeCell::new([BlobId::ZERO; MAX_B_DEVICES]));
static B_DEVICE_COUNT: AtomicU16 = AtomicU16::new(0);
static B_DEVICE_BAR_COUNT: AtomicU32 = AtomicU32::new(0);

#[repr(align(4096))]
#[allow(dead_code)]
struct Stage0BStack([u8; B_STACK_GUARD_SIZE + B_STACK_SIZE]);

static mut B_STAGE0_STACK: Stage0BStack = Stage0BStack([0; B_STACK_GUARD_SIZE + B_STACK_SIZE]);

#[inline(always)]
fn cpuid_ex(leaf: u32, subleaf: u32) -> (u32, u32, u32, u32) {
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
fn cpuid(leaf: u32) -> (u32, u32, u32, u32) {
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

/// Étape 0.5 : probe global des features B.
///
/// `hpet_available` est injecté par le parseur ACPI (étape 5) ; avant parse ACPI,
/// l'appelant peut passer `false` puis mettre à jour ensuite.
pub fn init_feature_probe(hpet_available: bool) {
    let features = crate::arch::x86_64::cpu::features::cpu_features_or_none();

    B_FEATURES.set_apic_mode(detect_apic_mode());
    B_FEATURES
        .pks_available
        .store(features.map_or(false, |cpu| cpu.has_pks()), Ordering::Release);
    B_FEATURES
        .invpcid_available
        .store(features.map_or(false, |cpu| cpu.has_invpcid()), Ordering::Release);
    B_FEATURES
        .pcid_available
        .store(features.map_or(false, |cpu| cpu.has_pcid()), Ordering::Release);
    B_FEATURES
        .smep_available
        .store(features.map_or(false, |cpu| cpu.has_smep()), Ordering::Release);
    B_FEATURES
        .smap_available
        .store(features.map_or(false, |cpu| cpu.has_smap()), Ordering::Release);

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

/// Étape 1 : installer les page tables de B.
///
/// Ici B adopte la PML4 kernel courante (KERNEL_AS) si disponible, sinon conserve CR3.
pub fn install_b_page_tables() -> u64 {
    let current_cr3 = x86_64::read_cr3();
    let current_flags = current_cr3 & 0xFFF;
    let kernel_pml4_phys = KERNEL_AS.pml4_phys().as_u64();
    let target_phys = if kernel_pml4_phys != 0 {
        kernel_pml4_phys
    } else {
        current_cr3 & !0xFFF
    };
    let new_cr3 = target_phys | current_flags;

    if (current_cr3 & !0xFFF) != target_phys {
        // SAFETY: target_phys est la PML4 noyau active, bootstrap ring0.
        unsafe {
            x86_64::write_cr3(new_cr3);
        }
    }

    B_STAGE0_CR3.store(new_cr3, Ordering::Release);
    new_cr3
}

/// Étape 2 : stack B + guard page !PRESENT sous la pile.
pub fn setup_b_stack_with_guard_page() -> u64 {
    // addr_of_mut! évite de créer une référence mutable aliasée.
    let base = core::ptr::addr_of_mut!(B_STAGE0_STACK) as *mut Stage0BStack as u64;
    let guard_page = base;
    let stack_base = base + B_STACK_GUARD_SIZE as u64;
    let stack_top = stack_base + B_STACK_SIZE as u64;

    let guard_va = VirtAddr::new(guard_page).page_align_down();
    if guard_va.is_kernel() {
        // SAFETY: demande explicite étape 2 : guard page non présente sous la stack B.
        unsafe {
            let _ = KERNEL_AS.unmap(guard_va);
        }
    }

    let _ = register_guard_region(
        stack_base,
        B_STACK_SIZE as u64,
        GuardRegionKind::KernelStack { cpu_id: 0 },
    );

    B_STACK_GUARD_VA.store(guard_page, Ordering::Release);
    B_STACK_TOP.store(stack_top, Ordering::Release);
    stack_top
}

/// Étape 3 : init B-TSS (Phase 3.1) et rechargement TR.
pub fn init_b_tss(stack_top: u64) {
    crate::arch::x86_64::tss::init_tss_for_cpu(0, stack_top);
    // SAFETY: descripteur TSS kernel valide dans la GDT BSP.
    unsafe {
        crate::arch::x86_64::tss::load_tss(crate::arch::x86_64::gdt::GDT_TSS_SEL);
    }
}

/// Étape 4 : IDT stubs (F1/F2/F3/#PF/NMI) — on garde les vecteurs ExoPhoenix inactifs.
pub fn setup_b_idt_with_stubs() {
    crate::arch::x86_64::idt::init_idt();
    crate::arch::x86_64::idt::load_idt();
    deactivate_exophoenix_vectors();
}

#[inline(always)]
fn align_up_u64(value: u64, align: u64) -> u64 {
    if align == 0 {
        value
    } else {
        (value + align - 1) & !(align - 1)
    }
}

fn parse_facs_from_fadt(fadt_phys: u64) -> u64 {
    if fadt_phys < 0x1000 || fadt_phys >= 0x4000_0000 {
        return 0;
    }

    // SdtHeader.length à offset +4.
    // SAFETY: zone ACPI identity-mappée et validée grossièrement ci-dessus.
    let fadt_len =
        unsafe { core::ptr::read_unaligned((fadt_phys as usize + 4) as *const u32) } as usize;

    if fadt_len < 40 {
        return 0;
    }

    // ACPI 1.0 FACS 32-bit (firmware_ctrl) à offset 36.
    // SAFETY: fadt_len vérifié >= 40.
    let facs32 =
        unsafe { core::ptr::read_unaligned((fadt_phys as usize + 36) as *const u32) } as u64;

    // ACPI 2.0+ X_FIRMWARE_CTRL à offset 132 (si table assez grande).
    let facs64 = if fadt_len >= 140 {
        // SAFETY: fadt_len vérifié >= 140.
        unsafe { core::ptr::read_unaligned((fadt_phys as usize + 132) as *const u64) }
    } else {
        0
    };

    if facs64 != 0 {
        facs64
    } else {
        facs32
    }
}

/// Étape 5 : parse ACPI MADT/FADT/FACS.
pub fn parse_stage0_acpi() -> Stage0AcpiSnapshot {
    let info = if parser::acpi_available() {
        Some(*parser::acpi_info())
    } else {
        parser::init_acpi()
    };

    let acpi = if let Some(info) = info {
        Stage0AcpiSnapshot {
            madt_phys: info.madt_phys,
            fadt_phys: info.fadt_phys,
            facs_phys: parse_facs_from_fadt(info.fadt_phys),
            hpet_phys: info.hpet_phys,
        }
    } else {
        Stage0AcpiSnapshot {
            madt_phys: 0,
            fadt_phys: 0,
            facs_phys: 0,
            hpet_phys: 0,
        }
    };

    B_FEATURES
        .hpet_available
        .store(acpi.hpet_phys != 0, Ordering::Release);

    acpi
}

const PCI_CFG_ADDR: u16 = 0xCF8;
const PCI_CFG_DATA: u16 = 0xCFC;

#[inline(always)]
fn pci_read_dword(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    let addr = 0x8000_0000u32
        | ((bus as u32) << 16)
        | ((device as u32) << 11)
        | ((function as u32) << 8)
        | ((offset as u32) & 0xFC);

    // SAFETY: accès CF8/CFC en ring0 pendant bootstrap Stage0.
    unsafe {
        x86_64::outl(PCI_CFG_ADDR, addr);
        x86_64::inl(PCI_CFG_DATA)
    }
}

#[inline(always)]
fn pci_vendor_id(bus: u8, device: u8, function: u8) -> u16 {
    (pci_read_dword(bus, device, function, 0x00) & 0xFFFF) as u16
}

/// Formule V4-M2 (implémentation défensive locale) :
/// base fixe + contribution par device + contribution par BAR, puis alignement 2 MiB.
pub fn calc_pool_r3_size(device_count: usize, bar_count: usize) -> u64 {
    const BASE: u64 = 8 * 1024 * 1024;
    const PER_DEVICE: u64 = 64 * 1024;
    const PER_BAR: u64 = 16 * 1024;
    const ALIGN: u64 = 2 * 1024 * 1024;
    const MAX: u64 = 256 * 1024 * 1024;

    let raw = BASE
        .saturating_add((device_count as u64).saturating_mul(PER_DEVICE))
        .saturating_add((bar_count as u64).saturating_mul(PER_BAR));
    let aligned = align_up_u64(raw, ALIGN);
    aligned.min(MAX)
}

/// Étape 5.5 : enum PCI (devices + BARs) + taille pool R3.
pub fn enumerate_pci_devices() -> usize {
    B_DEVICE_COUNT.store(0, Ordering::Release);
    B_DEVICE_BAR_COUNT.store(0, Ordering::Release);

    let mut count = 0usize;
    let mut bar_count = 0usize;

    for bus in 0u16..=255u16 {
        let bus = bus as u8;
        for dev in 0u8..32u8 {
            if pci_vendor_id(bus, dev, 0) == 0xFFFF {
                continue;
            }

            let header0 = ((pci_read_dword(bus, dev, 0, 0x0C) >> 16) & 0xFF) as u8;
            let fn_count = if (header0 & 0x80) != 0 { 8 } else { 1 };

            for func in 0u8..fn_count {
                let id = pci_read_dword(bus, dev, func, 0x00);
                let vendor_id = (id & 0xFFFF) as u16;
                if vendor_id == 0xFFFF {
                    continue;
                }

                let device_id = (id >> 16) as u16;
                let class_reg = pci_read_dword(bus, dev, func, 0x08);
                let class_code = ((class_reg >> 24) & 0xFF) as u8;
                let subclass = ((class_reg >> 16) & 0xFF) as u8;
                let prog_if = ((class_reg >> 8) & 0xFF) as u8;
                let header_type = ((pci_read_dword(bus, dev, func, 0x0C) >> 16) & 0xFF) as u8;

                let bar_slots = if (header_type & 0x7F) == 0x01 { 2 } else { 6 };
                let mut bar_bitmap = 0u8;
                for bar_idx in 0..bar_slots {
                    let bar_offset = 0x10 + (bar_idx * 4) as u8;
                    let bar = pci_read_dword(bus, dev, func, bar_offset);
                    if bar != 0 && bar != 0xFFFF_FFFF {
                        bar_bitmap |= 1u8 << bar_idx;
                        bar_count += 1;
                    }
                }

                if count < MAX_B_DEVICES {
                    // SAFETY: bootstrap single-thread ; count borné < MAX_B_DEVICES.
                    unsafe {
                        (*B_DEVICE_TABLE.0.get())[count] = PciDeviceRecord {
                            bus,
                            device: dev,
                            function: func,
                            header_type,
                            vendor_id,
                            device_id,
                            class_code,
                            subclass,
                            prog_if,
                            bar_bitmap,
                        };
                        (*B_DRIVER_BLOB_TABLE.0.get())[count] = BlobId::ZERO;
                    }
                }

                count = count.saturating_add(1);
            }
        }
    }

    let clamped = count.min(MAX_B_DEVICES);
    B_DEVICE_COUNT.store(clamped as u16, Ordering::Release);
    B_DEVICE_BAR_COUNT.store(bar_count as u32, Ordering::Release);

    let pool_size = calc_pool_r3_size(clamped, bar_count);
    POOL_R3_SIZE_BYTES.store(pool_size, Ordering::Release);

    clamped
}

/// Étape 6 : construction apic_to_slot[256] depuis la MADT réelle.
pub fn build_apic_to_slot_from_real_madt(madt_phys: u64) -> usize {
    for slot in &APIC_TO_SLOT {
        slot.store(0xFF, Ordering::Relaxed);
    }

    if madt_phys == 0 {
        return 0;
    }

    let info = madt::parse_madt(madt_phys);
    let mut mapped = 0usize;
    for idx in 0..(info.cpu_count as usize).min(256).min(64) {
        let apic_id = info.apic_ids[idx];
        if apic_id < 256 {
            APIC_TO_SLOT[apic_id as usize].store(idx as u8, Ordering::Release);
            mapped += 1;
        }
    }

    if mapped == 0 {
        // Fallback BSP minimal si MADT inutilisable.
        let bsp_apic = match B_FEATURES.apic_mode() {
            BootApicMode::X2Apic => crate::arch::x86_64::apic::x2apic::x2apic_id(),
            BootApicMode::XApic => local_apic::lapic_id(),
        } & 0xFF;
        APIC_TO_SLOT[bsp_apic as usize].store(0, Ordering::Release);
        mapped = 1;
    }

    mapped
}

#[inline(always)]
fn apic_reg_to_x2apic_msr(reg: u32) -> u32 {
    APIC_X2APIC_MSR_BASE + (reg >> 4)
}

#[inline(always)]
fn apic_timer_write(reg: u32, val: u32) {
    match B_FEATURES.apic_mode() {
        BootApicMode::XApic => local_apic::lapic_write(reg, val),
        BootApicMode::X2Apic => {
            let msr_reg = apic_reg_to_x2apic_msr(reg);
            // SAFETY: registre x2APIC standard, ring0 bootstrap.
            unsafe {
                msr::write_msr(msr_reg, val as u64);
            }
        }
    }
}

#[inline(always)]
fn apic_timer_read(reg: u32) -> u32 {
    match B_FEATURES.apic_mode() {
        BootApicMode::XApic => local_apic::lapic_read(reg),
        BootApicMode::X2Apic => {
            let msr_reg = apic_reg_to_x2apic_msr(reg);
            // SAFETY: registre x2APIC standard, ring0 bootstrap.
            unsafe { msr::read_msr(msr_reg) as u32 }
        }
    }
}

/// Étape 7 : calibration APIC timer via PIT channel 2.
pub fn calibrate_apic_timer_via_pit_ch2() -> Option<u64> {
    let irq_flags = x86_64::irq_save();
    let pit_count: u16 = 11_931; // ~10ms @ 1.193182MHz

    pit::setup_ch2_oneshot(pit_count);
    // Timer APIC one-shot masqué, compteur au maximum.
    apic_timer_write(
        local_apic::LAPIC_LVT_TIMER,
        local_apic::TIMER_MODE_ONESHOT | 0x0001_0000 | 0xFF,
    );
    apic_timer_write(local_apic::LAPIC_TIMER_DCR, 0x3); // diviseur /16
    apic_timer_write(local_apic::LAPIC_TIMER_ICR, u32::MAX);

    let done = pit::wait_ch2_done();
    pit::disable_ch2();
    let remaining = apic_timer_read(local_apic::LAPIC_TIMER_CCR);

    x86_64::irq_restore(irq_flags);

    if !done {
        return None;
    }

    let elapsed = u32::MAX.saturating_sub(remaining) as u64;
    if elapsed == 0 {
        return None;
    }

    let measure_us = (pit_count as u64).saturating_mul(1_000_000) / pit::PIT_FREQ_HZ;
    if measure_us == 0 {
        return None;
    }

    let ticks_per_us = elapsed / measure_us;
    if ticks_per_us == 0 {
        None
    } else {
        Some(ticks_per_us)
    }
}

pub fn calibrate_and_store_ticks_per_us() -> u64 {
    let ticks = calibrate_apic_timer_via_pit_ch2().unwrap_or(0);
    TICKS_PER_US.store(ticks, Ordering::Release);
    ticks
}

/// Étape 8 : init APIC local avec dispatch explicite xAPIC/x2APIC via B_FEATURES.
pub fn init_local_apic_dispatch() {
    match B_FEATURES.apic_mode() {
        BootApicMode::X2Apic => {
            x2apic::mask_all_lvt_x2apic();
            local_apic::set_spurious_vector(crate::arch::x86_64::idt::VEC_SPURIOUS);
            if crate::arch::x86_64::cpu::features::cpu_features_or_none()
                .map_or(false, |features| features.has_tsc_deadline())
            {
                local_apic::timer_init_tsc_deadline(crate::arch::x86_64::idt::VEC_IRQ_TIMER);
            } else {
                local_apic::timer_init_oneshot(crate::arch::x86_64::idt::VEC_IRQ_TIMER);
            }
        }
        BootApicMode::XApic => {
            local_apic::init_local_apic();
            local_apic::set_spurious_vector(crate::arch::x86_64::idt::VEC_SPURIOUS);
            if crate::arch::x86_64::cpu::features::cpu_features_or_none()
                .map_or(false, |features| features.has_tsc_deadline())
            {
                local_apic::timer_init_tsc_deadline(crate::arch::x86_64::idt::VEC_IRQ_TIMER);
            } else {
                local_apic::timer_init_oneshot(crate::arch::x86_64::idt::VEC_IRQ_TIMER);
            }
        }
    }
}

fn stage0_iommu_attach_all_to_blocked_domain(blocked_domain_id: u32) {
    let _ = IOMMU_DOMAINS.with_domain(
        crate::memory::dma::core::types::IommuDomainId(blocked_domain_id),
        |domain| {
            for idx in 0..b_device_count() {
                if let Some(dev) = b_device(idx) {
                    let _ = domain.attach_device(PciBdf::new(dev.bus, dev.device, dev.function));
                }
            }
            domain.activate();
        },
    );
}

trait IommuDriver {
    fn is_available(&self) -> bool;
    fn configure_deny_by_default(&self, blocked_domain_id: u32);
    fn flush_iotlb(&self, blocked_domain_id: u32);
}

struct IntelIommuDriver;
struct AmdIommuDriver;

impl IommuDriver for IntelIommuDriver {
    fn is_available(&self) -> bool {
        INTEL_VTD.is_initialized() && INTEL_VTD.unit_count() > 0
    }

    fn configure_deny_by_default(&self, blocked_domain_id: u32) {
        stage0_iommu_attach_all_to_blocked_domain(blocked_domain_id);
    }

    fn flush_iotlb(&self, blocked_domain_id: u32) {
        // SAFETY: Stage0 kernel ring0 ; la flush est idempotente et bornée au domaine bloqué.
        unsafe {
            INTEL_VTD.flush_iotlb_domain(blocked_domain_id as u16, 0);
        }
    }
}

impl IommuDriver for AmdIommuDriver {
    fn is_available(&self) -> bool {
        AMD_IOMMU.is_initialized() && AMD_IOMMU.unit_count() > 0
    }

    fn configure_deny_by_default(&self, blocked_domain_id: u32) {
        stage0_iommu_attach_all_to_blocked_domain(blocked_domain_id);
    }

    fn flush_iotlb(&self, _blocked_domain_id: u32) {
        // Sur AMD, la spec impose un Completion Wait après invalidation.
        // Le chemin commande complète (buffer + enqueue) n'est pas encore exposé
        // publiquement dans ce module ; on impose une barrière stricte en attendant.
        core::sync::atomic::fence(Ordering::SeqCst);
    }
}

fn root_port_count_from_b_device_table() -> u32 {
    let mut count = 0u32;
    for idx in 0..b_device_count() {
        let Some(dev) = b_device(idx) else { continue };
        // PCI-PCI bridge / Root Port classes.
        if dev.class_code == 0x06 && dev.subclass == 0x04 {
            count = count.saturating_add(1);
        }
    }
    count
}

fn update_iommu_protected_regions(pool_r3_base: u64, pool_r3_size: u64) {
    IOMMU_PROTECT_B_BASE.store(STAGE0_B_REGION_PHYS_BASE, Ordering::Release);
    IOMMU_PROTECT_B_SIZE.store(STAGE0_B_REGION_PHYS_SIZE, Ordering::Release);
    IOMMU_PROTECT_SSR_BASE.store(super::ssr::SSR_BASE, Ordering::Release);
    IOMMU_PROTECT_SSR_SIZE.store(super::ssr::SSR_SIZE as u64, Ordering::Release);
    IOMMU_PROTECT_POOL_R3_BASE.store(pool_r3_base, Ordering::Release);
    IOMMU_PROTECT_POOL_R3_SIZE.store(pool_r3_size, Ordering::Release);
}

/// Étape 9 : config IOMMU deny-by-default + politique régions + ACS root-ports.
pub fn setup_iommu_stage0(pool_r3_size_bytes: u64) {
    if IOMMU_DOMAINS.domain_count() == 0 {
        IOMMU_DOMAINS.init();
    }

    let blocked_domain_id = IOMMU_DOMAINS
        .create_domain(DomainType::Blocked, 0, 0)
        .map(|id| id.0)
        .unwrap_or(IDENTITY_DOMAIN_ID.0);
    IOMMU_BLOCKED_DOMAIN_ID.store(blocked_domain_id, Ordering::Release);

    let intel = IntelIommuDriver;
    let amd = AmdIommuDriver;

    if intel.is_available() {
        intel.configure_deny_by_default(blocked_domain_id);
        intel.flush_iotlb(blocked_domain_id);
    } else if amd.is_available() {
        amd.configure_deny_by_default(blocked_domain_id);
        amd.flush_iotlb(blocked_domain_id);
    } else {
        // Fallback sans contrôleur IOMMU actif: conserver la policy en mémoire
        // et rester deny-by-default côté table de domaines.
        stage0_iommu_attach_all_to_blocked_domain(blocked_domain_id);
    }

    // ACS root-ports: recensement et activation logique stricte (fallback no-ECAM).
    IOMMU_ACS_ROOT_PORTS.store(root_port_count_from_b_device_table(), Ordering::Release);

    // Regions protégées: B region + SSR + pool R3 (base allouée à l'étape 11).
    update_iommu_protected_regions(0, pool_r3_size_bytes);
    IOMMU_POLICY_READY.store(true, Ordering::Release);
}

fn acpi_table_len(table_phys: u64) -> Option<usize> {
    if table_phys < 0x1000 || table_phys >= 0x4000_0000 {
        return None;
    }
    // SAFETY: ACPI identity-map bootstrap ; lecture header SDT length @ +4.
    let len =
        unsafe { core::ptr::read_unaligned((table_phys as usize + 4) as *const u32) } as usize;
    if len < 36 || len > ACPI_MAX_TABLE_LEN {
        None
    } else {
        Some(len)
    }
}

fn store_madt_hash(hash: [u8; 32]) {
    for (idx, chunk) in hash.chunks_exact(8).enumerate() {
        let mut q = [0u8; 8];
        q.copy_from_slice(chunk);
        MADT_HASH_QWORDS[idx].store(u64::from_le_bytes(q), Ordering::Release);
    }
}

fn load_madt_hash() -> [u8; 32] {
    let mut out = [0u8; 32];
    for idx in 0..4 {
        let q = MADT_HASH_QWORDS[idx].load(Ordering::Acquire).to_le_bytes();
        out[idx * 8..idx * 8 + 8].copy_from_slice(&q);
    }
    out
}

/// Étape 10 : marquer FACS en lecture seule dans les PTE A/B partagées.
pub fn mark_facs_ro_in_a_pts(facs_phys: u64) -> bool {
    if facs_phys == 0 {
        FACS_RO_MARKED.store(false, Ordering::Release);
        return false;
    }

    let facs_virt = VirtAddr::new(facs_phys).page_align_down();
    let mut walker = PageTableWalker::new(KERNEL_AS.pml4_phys());
    let updated = match walker.walk_read(facs_virt) {
        WalkResult::Leaf { entry, .. } => {
            let mut flags = entry.to_page_flags();
            flags = flags.clear(PageFlags::WRITABLE);
            flags = flags.set(PageFlags::NO_EXECUTE);
            flags = flags.set(PageFlags::PRESENT);
            walker.remap_flags(facs_virt, flags).is_ok()
        }
        _ => false,
    };

    if updated {
        // SAFETY: invalidation locale d'une seule page après remap flags.
        unsafe {
            tlb::flush_single(facs_virt);
        }
    }

    FACS_RO_MARKED.store(updated, Ordering::Release);
    updated
}

/// Étape 10 : calcule et stocke le hash MADT (BLAKE3, 32 bytes).
pub fn hash_and_store_madt(madt_phys: u64) -> Option<[u8; 32]> {
    let len = acpi_table_len(madt_phys)?;
    // SAFETY: table ACPI identity-mappée ; longueur bornée et validée ci-dessus.
    let bytes = unsafe { core::slice::from_raw_parts(madt_phys as *const u8, len) };
    let hash = blake3_hash(bytes);
    store_madt_hash(hash);
    Some(hash)
}

#[inline(always)]
fn pages_for_size(size_bytes: u64) -> u64 {
    (size_bytes.saturating_add(PAGE_SIZE as u64 - 1)) / PAGE_SIZE as u64
}

#[inline(always)]
fn order_for_pages(mut pages: u64) -> usize {
    if pages <= 1 {
        return 0;
    }
    pages = pages.next_power_of_two();
    pages.trailing_zeros() as usize
}

/// Étape 11 : initialise/protège pool R3 selon POOL_R3_SIZE_BYTES.
pub fn init_pool_r3_from_stage0_size(pool_r3_size_bytes: u64) -> bool {
    if pool_r3_size_bytes == 0 {
        return false;
    }

    let order = order_for_pages(pages_for_size(pool_r3_size_bytes));
    let flags = AllocFlags::ZEROED | AllocFlags::DMA32;
    let Ok(frame) = buddy::alloc_pages(order, flags) else {
        return false;
    };

    let base = frame.start_address().as_u64();
    let alloc_bytes = (PAGE_SIZE as u64) << order;

    POOL_R3_BASE_PHYS.store(base, Ordering::Release);
    POOL_R3_ALLOC_BYTES.store(alloc_bytes, Ordering::Release);
    POOL_R3_ALLOC_ORDER.store(order as u8, Ordering::Release);

    // Protection soft côté intégrité mémoire (bornes surveillées).
    let _ = register_guard_region(base, alloc_bytes, GuardRegionKind::Generic);

    // Complète la policy IOMMU avec la vraie base de pool R3 + flush IOTLB.
    update_iommu_protected_regions(base, alloc_bytes);
    let blocked_domain = IOMMU_BLOCKED_DOMAIN_ID.load(Ordering::Acquire);
    if INTEL_VTD.is_initialized() {
        // SAFETY: flush ciblée domaine Stage0 après update policy.
        unsafe {
            INTEL_VTD.flush_iotlb_domain(blocked_domain as u16, 0);
        }
    } else if AMD_IOMMU.is_initialized() {
        core::sync::atomic::fence(Ordering::SeqCst);
    }

    true
}

/// Étape 12 : arme watchdog APIC (par défaut 5000ms), basé sur TICKS_PER_US.
pub fn arm_apic_watchdog(ms: u64) -> u64 {
    let ms = if ms == 0 { WATCHDOG_DEFAULT_MS } else { ms };
    let ticks_per_us = TICKS_PER_US.load(Ordering::Acquire);
    if ticks_per_us == 0 {
        WATCHDOG_ARMED_MS.store(0, Ordering::Release);
        WATCHDOG_ARMED_TICKS.store(0, Ordering::Release);
        return 0;
    }

    let watchdog_us = ms.saturating_mul(1000);
    let ticks = watchdog_us.saturating_mul(ticks_per_us);
    let initial_count = ticks.min(u32::MAX as u64) as u32;

    apic_timer_write(
        local_apic::LAPIC_LVT_TIMER,
        local_apic::TIMER_MODE_ONESHOT | crate::arch::x86_64::idt::VEC_IRQ_TIMER as u32,
    );
    apic_timer_write(local_apic::LAPIC_TIMER_DCR, 0x3);
    apic_timer_write(local_apic::LAPIC_TIMER_ICR, initial_count);

    WATCHDOG_ARMED_MS.store(ms, Ordering::Release);
    WATCHDOG_ARMED_TICKS.store(ticks, Ordering::Release);
    ticks
}

/// Orchestrateur strict des étapes 1→12.
pub fn stage0_init_all_steps() -> Stage0Summary {
    // SAFETY: Stage0 s'exécute sur Kernel B avant la prise de contrôle normale
    // de Kernel A ; ExoSeal phase 0 est idempotent.
    unsafe {
        crate::security::exoseal::exoseal_boot_phase0();
    }

    // 1) Page tables de B
    let b_cr3 = install_b_page_tables();

    // 0.5) Probe CPUID global (HPET faux jusqu'au parse ACPI)
    init_feature_probe(false);

    // 2) Stack B + guard page
    let b_stack_top = setup_b_stack_with_guard_page();

    // 3) TSS/IST B
    init_b_tss(b_stack_top);

    // 4) IDT stubs
    setup_b_idt_with_stubs();

    // 5) ACPI (MADT/FADT/FACS)
    let acpi = parse_stage0_acpi();

    // 5.5) PCI + taille pool R3
    let pci_device_count = enumerate_pci_devices();
    const POOL_R3_MIN_SIZE: u64 = 8 * 1024 * 1024;
    let pool_r3_size = POOL_R3_SIZE_BYTES
        .load(Ordering::Acquire)
        .max(POOL_R3_MIN_SIZE);
    POOL_R3_SIZE_BYTES.store(pool_r3_size, Ordering::Release);

    // 6) APIC->slot depuis MADT réel
    let apic_slots_mapped = build_apic_to_slot_from_real_madt(acpi.madt_phys);

    // 7) APIC timer via PIT ch2
    let ticks_per_us = calibrate_and_store_ticks_per_us();

    // 8) APIC local dispatch + VMXOFF défensif
    init_local_apic_dispatch();
    vmxoff_if_active();

    // 9) IOMMU + ACS root-ports + IOTLB flush
    setup_iommu_stage0(pool_r3_size);
    crate::security::exoseal::configure_nic_iommu_policy();

    // 10) FACS en RO + hash MADT
    let _ = mark_facs_ro_in_a_pts(acpi.facs_phys);
    let _ = hash_and_store_madt(acpi.madt_phys);

    // 11) Pool R3
    let _ = init_pool_r3_from_stage0_size(pool_r3_size);

    // 12) Watchdog APIC (5000ms par défaut)
    let _ = arm_apic_watchdog(WATCHDOG_DEFAULT_MS);

    Stage0Summary {
        b_cr3,
        b_stack_top,
        acpi,
        pci_device_count,
        pool_r3_size,
        apic_slots_mapped,
        ticks_per_us,
    }
}

/// Stage0 complet (1→13): bascule Normal, SIPI one-shot, puis boucle sentinelle.
pub fn stage0_init() -> ! {
    let _summary = stage0_init_all_steps();

    if crate::exophoenix::forge::kernel_a_hash_is_zero() {
        log::error!("FORGE: hash Kernel A non initialisé — ExoPhoenix désactivé (degraded)");
        PHOENIX_STATE.store(PhoenixState::Degraded as u8, Ordering::Release);
        loop {
            unsafe {
                core::arch::asm!("hlt", options(nostack, nomem));
            }
        }
    }

    // BUG-GX-05 FIX: synchroniser le count de cœurs dans la SSR
    let n_cores = crate::arch::x86_64::smp::init::smp_cpu_count();
    exo_phoenix_ssr::init_core_count(n_cores.min(exo_phoenix_ssr::SSR_MAX_CORES_LAYOUT as u32));

    PHOENIX_STATE.store(PhoenixState::Normal as u8, Ordering::Release);
    let _ = send_sipi_once(CORE_A_SLOT, A_ENTRY_VECTOR);
    sentinel::run_forever()
}

pub fn pool_r3_size_bytes() -> u64 {
    POOL_R3_SIZE_BYTES.load(Ordering::Acquire)
}

pub fn b_device_count() -> usize {
    B_DEVICE_COUNT.load(Ordering::Acquire) as usize
}

pub fn b_device(index: usize) -> Option<PciDeviceRecord> {
    if index >= b_device_count() || index >= MAX_B_DEVICES {
        return None;
    }
    // SAFETY: table statique ; écritures uniquement au boot stage0.
    Some(unsafe { (*B_DEVICE_TABLE.0.get())[index] })
}

/// Retourne le BlobId driver associé à un BDF (clé 0x00BB_DDFF), si présent.
pub fn driver_blob_id(bdf_key: u32) -> Option<BlobId> {
    let count = b_device_count();
    let mut idx = 0usize;
    while idx < count {
        let dev = b_device(idx)?;
        let key = ((dev.bus as u32) << 16) | ((dev.device as u32) << 8) | dev.function as u32;
        if key == bdf_key {
            // SAFETY: écritures uniquement au boot stage0 ; lecture en RO ensuite.
            let blob = unsafe { (*B_DRIVER_BLOB_TABLE.0.get())[idx] };
            if blob == BlobId::ZERO {
                return None;
            }
            return Some(blob);
        }
        idx += 1;
    }
    None
}

pub fn ticks_per_us() -> u64 {
    TICKS_PER_US.load(Ordering::Acquire)
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

#[inline(always)]
pub fn activate_exophoenix_vectors() {
    EXOPHOENIX_VECTORS_ACTIVE.store(true, Ordering::Release);
}

#[inline(always)]
pub fn deactivate_exophoenix_vectors() {
    EXOPHOENIX_VECTORS_ACTIVE.store(false, Ordering::Release);
}

#[inline(always)]
pub fn exophoenix_vectors_active() -> bool {
    EXOPHOENIX_VECTORS_ACTIVE.load(Ordering::Acquire)
}

#[inline(always)]
pub fn set_apic_slot(apic_id: u8, slot_index: u8) {
    APIC_TO_SLOT[apic_id as usize].store(slot_index, Ordering::Release);
}

#[inline(always)]
pub fn apic_slot(apic_id: u32) -> Option<usize> {
    let slot = APIC_TO_SLOT[(apic_id & 0xFF) as usize].load(Ordering::Acquire);
    if slot == 0xFF {
        None
    } else {
        Some(slot as usize)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendSipiError {
    AlreadySent,
    TargetNotFound,
}

#[inline(always)]
fn current_apic_id_low8() -> u8 {
    match B_FEATURES.apic_mode() {
        BootApicMode::X2Apic => (x2apic::x2apic_id() & 0xFF) as u8,
        BootApicMode::XApic => (local_apic::lapic_id() & 0xFF) as u8,
    }
}

fn resolve_apic_id_for_slot(slot_index: u8) -> Option<u8> {
    let current = current_apic_id_low8();
    let mut fallback = None;
    for apic_id in 0u16..=255u16 {
        let mapped = APIC_TO_SLOT[apic_id as usize].load(Ordering::Acquire);
        if mapped != slot_index {
            continue;
        }
        let found = apic_id as u8;
        if found != current {
            return Some(found);
        }
        fallback = Some(found);
    }
    fallback
}

/// Étape 13 : envoi SIPI one-shot (G8) vers le slot cible.
pub fn send_sipi_once(core_slot: u8, entry_vector: u8) -> Result<(), SendSipiError> {
    let prior = SIPI_SENT.fetch_or(SIPI_SENT_BIT, Ordering::AcqRel);
    if (prior & SIPI_SENT_BIT) != 0 {
        return Err(SendSipiError::AlreadySent);
    }

    let apic_id = resolve_apic_id_for_slot(core_slot).ok_or(SendSipiError::TargetNotFound)?;
    ipi::send_init_ipi(apic_id);
    tsc::tsc_delay_ms(10);
    ipi::send_startup_ipi(apic_id, entry_vector);
    tsc::tsc_delay_ms(1);
    ipi::send_startup_ipi(apic_id, entry_vector);
    Ok(())
}

pub fn madt_hash() -> [u8; 32] {
    load_madt_hash()
}

pub fn facs_ro_marked() -> bool {
    FACS_RO_MARKED.load(Ordering::Acquire)
}

pub fn pool_r3_base_phys() -> u64 {
    POOL_R3_BASE_PHYS.load(Ordering::Acquire)
}

pub fn pool_r3_alloc_bytes() -> u64 {
    POOL_R3_ALLOC_BYTES.load(Ordering::Acquire)
}

pub fn watchdog_armed_ms() -> u64 {
    WATCHDOG_ARMED_MS.load(Ordering::Acquire)
}

pub fn watchdog_armed_ticks() -> u64 {
    WATCHDOG_ARMED_TICKS.load(Ordering::Acquire)
}

pub fn iommu_policy_ready() -> bool {
    IOMMU_POLICY_READY.load(Ordering::Acquire)
}

pub fn iommu_acs_root_ports() -> u32 {
    IOMMU_ACS_ROOT_PORTS.load(Ordering::Acquire)
}

pub fn blocked_domain_id() -> u32 {
    IOMMU_BLOCKED_DOMAIN_ID.load(Ordering::Acquire)
}
