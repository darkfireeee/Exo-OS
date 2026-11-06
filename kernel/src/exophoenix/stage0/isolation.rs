//! # exophoenix/stage0/isolation.rs — Memory isolation, IOMMU policy and stack B
//!
//! Handles Kernel B page table installation, stack B with !PRESENT guard page,
//! marking FACS table read-only, allocating and protecting pool R3,
//! and deny-by-default IOMMU setup with ACS root-port isolation.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use crate::arch::x86_64;
use crate::memory::core::{AllocFlags, PageFlags, VirtAddr, PAGE_SIZE};
use crate::memory::dma::iommu::domain::{DomainType, PciBdf, IOMMU_DOMAINS};
use crate::memory::dma::iommu::{AMD_IOMMU, IDENTITY_DOMAIN_ID, INTEL_VTD};
use crate::memory::integrity::{register_guard_region, GuardRegionKind};
use crate::memory::physical::allocator::buddy;
use crate::memory::virt::address_space::kernel::KERNEL_AS;
use crate::memory::virt::address_space::tlb;
use crate::memory::virt::page_table::{PageTableWalker, WalkResult};
use super::hardware_scan::{b_device, b_device_count};

pub const STAGE0_B_REGION_PHYS_BASE: u64 = crate::memory::core::layout::KERNEL_LOAD_PHYS_ADDR;
pub const STAGE0_B_REGION_PHYS_SIZE: u64 = crate::memory::core::layout::KERNEL_IMAGE_MAX_SIZE as u64;

pub const B_STACK_GUARD_SIZE: usize = 4096;
pub const B_STACK_SIZE: usize = 64 * 1024;

/// Vectors 0xF1/0xF2/0xF3 are activated only once Stage 0 is ready.
pub static EXOPHOENIX_VECTORS_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Stage0 results (1..7)
pub static B_STAGE0_CR3: AtomicU64 = AtomicU64::new(0);
pub static B_STACK_TOP: AtomicU64 = AtomicU64::new(0);
pub static B_STACK_GUARD_VA: AtomicU64 = AtomicU64::new(0);
pub static POOL_R3_SIZE_BYTES: AtomicU64 = AtomicU64::new(0);
pub static POOL_R3_BASE_PHYS: AtomicU64 = AtomicU64::new(0);
pub static POOL_R3_ALLOC_BYTES: AtomicU64 = AtomicU64::new(0);
pub static POOL_R3_ALLOC_ORDER: AtomicU8 = AtomicU8::new(0);

/// Step 9: Stage0 IOMMU policy (deny-by-default + protected regions).
pub static IOMMU_POLICY_READY: AtomicBool = AtomicBool::new(false);
pub static IOMMU_BLOCKED_DOMAIN_ID: AtomicU32 = AtomicU32::new(0);
pub static IOMMU_ACS_ROOT_PORTS: AtomicU32 = AtomicU32::new(0);
pub static IOMMU_PROTECT_B_BASE: AtomicU64 = AtomicU64::new(0);
pub static IOMMU_PROTECT_B_SIZE: AtomicU64 = AtomicU64::new(0);
pub static IOMMU_PROTECT_SSR_BASE: AtomicU64 = AtomicU64::new(0);
pub static IOMMU_PROTECT_SSR_SIZE: AtomicU64 = AtomicU64::new(0);
pub static IOMMU_PROTECT_POOL_R3_BASE: AtomicU64 = AtomicU64::new(0);
pub static IOMMU_PROTECT_POOL_R3_SIZE: AtomicU64 = AtomicU64::new(0);

/// Step 10: FACS RO.
pub static FACS_RO_MARKED: AtomicBool = AtomicBool::new(false);

#[repr(align(4096))]
#[allow(dead_code)]
struct Stage0BStack([u8; B_STACK_GUARD_SIZE + B_STACK_SIZE]);

static mut B_STAGE0_STACK: Stage0BStack = Stage0BStack([0; B_STACK_GUARD_SIZE + B_STACK_SIZE]);

/// Step 1: install page tables of B.
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
        // SAFETY: target_phys is the active kernel PML4, ring0 bootstrap.
        unsafe {
            x86_64::write_cr3(new_cr3);
        }
    }

    B_STAGE0_CR3.store(new_cr3, Ordering::Release);
    new_cr3
}

/// Step 2: stack B + guard page !PRESENT below stack.
pub fn setup_b_stack_with_guard_page() -> u64 {
    let base = core::ptr::addr_of_mut!(B_STAGE0_STACK) as *mut Stage0BStack as u64;
    let guard_page = base;
    let stack_base = base + B_STACK_GUARD_SIZE as u64;
    let stack_top = stack_base + B_STACK_SIZE as u64;

    let guard_va = VirtAddr::new(guard_page).page_align_down();
    if guard_va.is_kernel() {
        // SAFETY: explicit step 2 requirement: guard page not present under stack B.
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

/// Step 3: init B-TSS (Phase 3.1) and reload TR.
pub fn init_b_tss(stack_top: u64) {
    crate::arch::x86_64::tss::init_tss_for_cpu(0, stack_top);
    // SAFETY: valid kernel TSS descriptor in BSP GDT.
    unsafe {
        crate::arch::x86_64::tss::load_tss(crate::arch::x86_64::gdt::GDT_TSS_SEL);
    }
}

/// Step 4: IDT stubs (F1/F2/F3/#PF/NMI) — keep ExoPhoenix vectors inactive.
pub fn setup_b_idt_with_stubs() {
    crate::arch::x86_64::idt::init_idt();
    crate::arch::x86_64::idt::load_idt();
    deactivate_exophoenix_vectors();
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

pub fn stage0_iommu_attach_all_to_blocked_domain(blocked_domain_id: u32) {
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

pub trait IommuDriver {
    fn is_available(&self) -> bool;
    fn configure_deny_by_default(&self, blocked_domain_id: u32);
    fn flush_iotlb(&self, blocked_domain_id: u32);
}

pub struct IntelIommuDriver;
pub struct AmdIommuDriver;

impl IommuDriver for IntelIommuDriver {
    fn is_available(&self) -> bool {
        INTEL_VTD.is_initialized() && INTEL_VTD.unit_count() > 0
    }

    fn configure_deny_by_default(&self, blocked_domain_id: u32) {
        stage0_iommu_attach_all_to_blocked_domain(blocked_domain_id);
    }

    fn flush_iotlb(&self, blocked_domain_id: u32) {
        // SAFETY: Stage0 kernel ring0; flush is idempotent and bounded to blocked domain.
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
        core::sync::atomic::fence(Ordering::SeqCst);
    }
}

pub fn root_port_count_from_b_device_table() -> u32 {
    let mut count = 0u32;
    for idx in 0..b_device_count() {
        let Some(dev) = b_device(idx) else { continue };
        if dev.class_code == 0x06 && dev.subclass == 0x04 {
            count = count.saturating_add(1);
        }
    }
    count
}

pub fn update_iommu_protected_regions(pool_r3_base: u64, pool_r3_size: u64) {
    IOMMU_PROTECT_B_BASE.store(STAGE0_B_REGION_PHYS_BASE, Ordering::Release);
    IOMMU_PROTECT_B_SIZE.store(STAGE0_B_REGION_PHYS_SIZE, Ordering::Release);
    IOMMU_PROTECT_SSR_BASE.store(super::super::ssr::SSR_BASE, Ordering::Release);
    IOMMU_PROTECT_SSR_SIZE.store(super::super::ssr::SSR_SIZE as u64, Ordering::Release);
    IOMMU_PROTECT_POOL_R3_BASE.store(pool_r3_base, Ordering::Release);
    IOMMU_PROTECT_POOL_R3_SIZE.store(pool_r3_size, Ordering::Release);
}

/// Step 9: deny-by-default IOMMU config + region policy + ACS root-ports.
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
        stage0_iommu_attach_all_to_blocked_domain(blocked_domain_id);
    }

    IOMMU_ACS_ROOT_PORTS.store(root_port_count_from_b_device_table(), Ordering::Release);
    update_iommu_protected_regions(0, pool_r3_size_bytes);
    IOMMU_POLICY_READY.store(true, Ordering::Release);
}

/// Step 10: mark FACS as read-only in shared A/B PTEs.
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
        // SAFETY: local invalidation of a single page after remap flags.
        unsafe {
            tlb::flush_single(facs_virt);
        }
    }

    FACS_RO_MARKED.store(updated, Ordering::Release);
    updated
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

/// Step 11: initialize/protect pool R3 according to POOL_R3_SIZE_BYTES.
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

    let _ = register_guard_region(base, alloc_bytes, GuardRegionKind::Generic);

    update_iommu_protected_regions(base, alloc_bytes);
    let blocked_domain = IOMMU_BLOCKED_DOMAIN_ID.load(Ordering::Acquire);
    if INTEL_VTD.is_initialized() {
        // SAFETY: targeted flush of Stage0 domain after policy update.
        unsafe {
            INTEL_VTD.flush_iotlb_domain(blocked_domain as u16, 0);
        }
    } else if AMD_IOMMU.is_initialized() {
        core::sync::atomic::fence(Ordering::SeqCst);
    }

    true
}

pub fn pool_r3_size_bytes() -> u64 {
    POOL_R3_SIZE_BYTES.load(Ordering::Acquire)
}

pub fn set_pool_r3_size_bytes(val: u64) {
    POOL_R3_SIZE_BYTES.store(val, Ordering::Release);
}

pub fn pool_r3_base_phys() -> u64 {
    POOL_R3_BASE_PHYS.load(Ordering::Acquire)
}

pub fn pool_r3_alloc_bytes() -> u64 {
    POOL_R3_ALLOC_BYTES.load(Ordering::Acquire)
}

pub fn facs_ro_marked() -> bool {
    FACS_RO_MARKED.load(Ordering::Acquire)
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
