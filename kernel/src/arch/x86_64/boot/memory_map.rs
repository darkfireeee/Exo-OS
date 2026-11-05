// kernel/src/arch/x86_64/boot/memory_map.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// BOOT MEMORY MAP BRIDGE ↔ MEMORY/ — x86_64
// ═══════════════════════════════════════════════════════════════════════════════
//
// This module translates the physical memory described by the bootloader (Multiboot2
// or UEFI) into calls to the memory/ subsystem to initialize allocators.
//
// ## Initialization Sequence (Rule MEM-02, DOC2)
//
//   1. EmergencyPool::init()           — FIRST, before any allocator
//   2. init_memory_subsystem_multiboot2() or init_memory_subsystem_uefi()
//      a. init_phase1_bitmap(total_phys_start, total_phys_end)
//      b. For each `Usable` region in E820:
//         init_phase2_free_region(start, end)
//   3. init_phase3_slab_slub()         — after physmap
//   4. init_phase4_numa(nodes_mask)    — after ACPI topology
//   5. KernelAddressSpace::init()      — kernel PML4
//   6. memory_iface::init_memory_integration() — IPI TLB sender

use crate::arch::x86_64::boot::multiboot2::{MmapEntry, Multiboot2Info, MMAP_AVAILABLE};
use crate::arch::x86_64::boot::uefi::UefiMemoryMap;
use crate::exophoenix::ssr;
use crate::memory::core::AllocError;
use crate::memory::core::{AllocFlags, Frame, PageFlags, PhysAddr, HUGE_PAGE_SIZE, PAGE_SIZE};
use crate::memory::physical::allocator::{
    alloc_page, free_page, init_phase1_bitmap, init_phase2_free_region,
    init_phase2b_buddy_free_region, init_phase2b_buddy_zone, init_phase3_slab_slub,
    init_phase4_numa, register_slab_page_provider, SlabPageProvider, BOOTSTRAP_BITMAP,
};
use crate::memory::virt::page_table::{
    phys_to_table_mut, read_cr3, write_cr3, FrameAllocatorForWalk, PageTableEntry,
};

const _: () = assert!(
    crate::memory::core::layout::PHYS_MAP_SIZE as u64
        >= crate::arch::constants::PHYSMAP_INITIAL_COVERAGE_BYTES,
    "physmap must cover at least CORR-76 initial memory"
);

// ─────────────────────────────────────────────────────────────────────────────
// STATIC BITMAP FOR BUDDY ALLOCATOR (DMA32 zone — <4 GiB)
// ─────────────────────────────────────────────────────────────────────────────
//
// Covers up to 4 GiB of RAM:
//   - Max pages: 4 GiB / 4 KiB = 1,048,576 pages
//   - u64 words: ceil(1,048,576 / 64) = 16,384 → 128 KiB in .bss
//
// Allocated in .bss (zero-initialized by bootloader). BuddyZone::init()
// resets it to 0xFF…FF (all-allocated) then add_free_range() clears
// bits of genuinely free pages.
const BUDDY_DMA32_BITMAP_WORDS: usize = 16_384;
static mut BUDDY_DMA32_BITMAP: [u64; BUDDY_DMA32_BITMAP_WORDS] = [0u64; BUDDY_DMA32_BITMAP_WORDS];

#[inline(always)]
fn buddy_dma32_bitmap_ptr() -> *mut u64 {
    core::ptr::addr_of_mut!(BUDDY_DMA32_BITMAP).cast::<u64>()
}

// ─────────────────────────────────────────────────────────────────────────────
// PHYSICAL PAGE PROVIDER FOR SLAB (bootstrap)
// ─────────────────────────────────────────────────────────────────────────────

/// Implementation of SlabPageProvider using buddy as soon as ready,
/// with bootstrap bitmap fallback only if buddy is not yet initialized.
///
/// This prevents SLUB and vmalloc from allocating in parallel across two
/// independent physical pools once boot advances.
struct KernelSlabProvider;

// SAFETY: KernelSlabProvider is a ZST; the invoked physical backends are
// each thread-safe according to their internal invariants.
unsafe impl Sync for KernelSlabProvider {}

impl SlabPageProvider for KernelSlabProvider {
    fn get_page(&self) -> Result<PhysAddr, AllocError> {
        match alloc_page(crate::memory::core::AllocFlags::NONE) {
            Ok(frame) => Ok(frame.start_address()),
            Err(AllocError::NotInitialized) => {
                let frame = BOOTSTRAP_BITMAP.alloc_frame(crate::memory::core::AllocFlags::NONE)?;
                Ok(frame.start_address())
            }
            Err(e) => Err(e),
        }
    }

    fn put_page(&self, phys: PhysAddr) {
        let frame = Frame::containing(phys);
        if matches!(free_page(frame), Err(AllocError::NotInitialized)) {
            BOOTSTRAP_BITMAP.free_frame(frame);
        }
    }
}

/// Static provider instance ('static lifetime required by slab).
static KERNEL_SLAB_PROVIDER: KernelSlabProvider = KernelSlabProvider;

struct BootPageTableAllocator;

impl FrameAllocatorForWalk for BootPageTableAllocator {
    fn alloc_frame(&self, flags: AllocFlags) -> Result<Frame, AllocError> {
        BOOTSTRAP_BITMAP.alloc_frame(flags)
    }

    fn free_frame(&self, frame: Frame) {
        BOOTSTRAP_BITMAP.free_frame(frame);
    }
}

#[inline]
fn assert_buddy_has_free_frames(origin: &str) {
    let free = crate::memory::physical::allocator::BUDDY.total_free_frames();
    assert!(
        free > 0,
        "{}: buddy initialise sans aucune frame libre",
        origin
    );
}

fn install_extended_physmap(phys_end: PhysAddr) {
    let pt_alloc = BootPageTableAllocator;
    let active = read_cr3();
    let result = unsafe { install_fresh_high_physmap(active, phys_end.as_u64(), &pt_alloc) };
    if result.is_err() {
        crate::arch::x86_64::halt_cpu();
    }
    // SAFETY: rewriting CR3 with active PML4 forces a TLB flush after extension.
    unsafe {
        write_cr3(active);
    }
    crate::memory::core::layout::set_physmap_limit(phys_end.as_u64());
}

unsafe fn install_fresh_high_physmap<A: FrameAllocatorForWalk>(
    active_pml4: PhysAddr,
    phys_size: u64,
    alloc: &A,
) -> Result<(), AllocError> {
    let pml4 = phys_to_table_mut(active_pml4);
    let mut offset = 0u64;
    let mut delayed_p4_256 = PhysAddr::new(0);
    let mut has_delayed_p4_256 = false;

    while offset < phys_size {
        let virt = crate::memory::core::layout::PHYS_MAP_BASE.as_u64() + offset;
        let p4_idx = crate::memory::core::VirtAddr::new(virt).p4_index();
        let pdpt_frame = alloc.alloc_frame(AllocFlags::ZEROED)?;
        let pdpt_phys = pdpt_frame.start_address();
        let pdpt = phys_to_table_mut(pdpt_phys);

        while offset < phys_size {
            let virt = crate::memory::core::layout::PHYS_MAP_BASE.as_u64() + offset;
            let vaddr = crate::memory::core::VirtAddr::new(virt);
            if vaddr.p4_index() != p4_idx {
                break;
            }

            let pd_frame = alloc.alloc_frame(AllocFlags::ZEROED)?;
            let pd_phys = pd_frame.start_address();
            pdpt[vaddr.p3_index()] = table_pointer_entry(pd_phys);
            let pd = phys_to_table_mut(pd_phys);

            while offset < phys_size {
                let virt = crate::memory::core::layout::PHYS_MAP_BASE.as_u64() + offset;
                let vaddr = crate::memory::core::VirtAddr::new(virt);
                if vaddr.p4_index() != p4_idx
                    || pdpt[vaddr.p3_index()].phys_addr().as_u64() != pd_phys.as_u64()
                {
                    break;
                }

                let remaining = phys_size - offset;
                let phys = PhysAddr::new(offset);
                if remaining >= HUGE_PAGE_SIZE as u64
                    && (virt & (HUGE_PAGE_SIZE as u64 - 1)) == 0
                    && phys.is_aligned(HUGE_PAGE_SIZE as u64)
                {
                    pd[vaddr.p2_index()] = PageTableEntry::from_page_flags(
                        Frame::containing(phys),
                        PageFlags::KERNEL_DATA | PageFlags::HUGE_PAGE,
                    );
                    offset = offset.saturating_add(HUGE_PAGE_SIZE as u64);
                } else {
                    let pt_frame = alloc.alloc_frame(AllocFlags::ZEROED)?;
                    let pt_phys = pt_frame.start_address();
                    pd[vaddr.p2_index()] = table_pointer_entry(pt_phys);
                    let pt = phys_to_table_mut(pt_phys);
                    while offset < phys_size {
                        let virt = crate::memory::core::layout::PHYS_MAP_BASE.as_u64() + offset;
                        let tail_vaddr = crate::memory::core::VirtAddr::new(virt);
                        if tail_vaddr.p4_index() != p4_idx
                            || tail_vaddr.p3_index() != vaddr.p3_index()
                            || tail_vaddr.p2_index() != vaddr.p2_index()
                        {
                            break;
                        }
                        let tail_phys = PhysAddr::new(offset);
                        pt[tail_vaddr.p1_index()] = PageTableEntry::from_page_flags(
                            Frame::containing(tail_phys),
                            PageFlags::KERNEL_DATA,
                        );
                        offset = offset.saturating_add(PAGE_SIZE as u64);
                    }
                }
            }
        }

        if p4_idx == 256 {
            delayed_p4_256 = pdpt_phys;
            has_delayed_p4_256 = true;
        } else {
            pml4[p4_idx] = table_pointer_entry(pdpt_phys);
        }
    }

    if has_delayed_p4_256 {
        // Switch shared boot entry PML4[256] last: new high-half
        // tables already map active PML4 frame via PHYS_MAP_BASE.
        pml4[256] = table_pointer_entry(delayed_p4_256);
    }
    Ok(())
}

#[inline]
fn table_pointer_entry(phys: PhysAddr) -> PageTableEntry {
    PageTableEntry::from_raw(
        phys.as_u64() | PageTableEntry::FLAG_PRESENT | PageTableEntry::FLAG_WRITABLE,
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// SYSTEM PHYSICAL BOUNDS
// ─────────────────────────────────────────────────────────────────────────────

/// Physical start address of identified RAM.
/// Reserved up to 1 MiB for legacy BIOS/ISA.
pub const PHYS_MEMORY_START: u64 = 0x0010_0000; // 1 MiB

/// Maximum supported physical address (48-bit x86_64 PA).
pub const PHYS_MEMORY_MAX: u64 = (1u64 << 48) - 1;

// ── Physical kernel end (binary + boot stack) ─────────────────────────────────
/// Returns page-aligned physical address immediately following BSP boot stack.
///
/// **The kernel physically occupies** `PHYS_MEMORY_START .. kernel_physical_end()`.
/// None of these pages may be declared free to the bitmap/buddy.
///
/// Uses linker symbol `_exo_boot_stack_top` (defined in main.rs).
/// This symbol marks the end of `.boot_stack` section (highest memory
/// used by binary kernel + its bootstrap stack).
#[inline]
fn kernel_physical_end() -> u64 {
    extern "C" {
        static _exo_boot_stack_top: u8;
    }
    // &raw const does not dereference the pointer — no value access.
    // No unsafe needed: operation does not read memory.
    let top = &raw const _exo_boot_stack_top as u64;
    // Round up to next page (keeps a safety margin).
    (top + PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1)
}

// ─────────────────────────────────────────────────────────────────────────────
// MEMORY MAP REPORTED TO KERNEL
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum number of memory regions in E820 map (covers real-world cases).
pub const MAX_MEMORY_REGIONS: usize = 256;

/// Memory region type (aligned with E820 / UEFI map).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryRegionType {
    /// Usable RAM.
    Usable,
    /// Reserved (firmware, BIOS, reserved DMA).
    Reserved,
    /// ACPI reclaimable (can be freed after ACPI parse).
    AcpiReclaimable,
    /// ACPI NVS (non-volatile storage — never free).
    AcpiNvs,
    /// Bad/defective (memory errors detected by firmware).
    Bad,
    /// Kernel image (loaded by bootloader).
    KernelImage,
}

/// Physical memory map region.
#[derive(Debug, Clone, Copy)]
pub struct MemoryRegion {
    pub base: u64,
    pub size: u64,
    pub region_type: MemoryRegionType,
}

impl MemoryRegion {
    #[inline]
    pub fn end(&self) -> u64 {
        self.base + self.size
    }

    #[inline]
    pub fn is_usable(&self) -> bool {
        matches!(self.region_type, MemoryRegionType::Usable)
    }
}

/// Static memory map (built at boot, immutable afterwards).
pub static mut MEMORY_MAP: [MemoryRegion; MAX_MEMORY_REGIONS] = [MemoryRegion {
    base: 0,
    size: 0,
    region_type: MemoryRegionType::Reserved,
}; MAX_MEMORY_REGIONS];

/// Number of valid regions in `MEMORY_MAP`.
pub static mut MEMORY_REGION_COUNT: usize = 0;

// ─────────────────────────────────────────────────────────────────────────────
// INIT FROM MULTIBOOT2
// ─────────────────────────────────────────────────────────────────────────────

/// Initializes the memory subsystem from a Multiboot2 structure.
///
/// Reads MmapEntry entries and invokes buddy allocator init phases.
///
/// # Safety
/// - Must be called EXACTLY ONCE from BSP, before SMP.
/// - `info.mmap_ptr` must point to a valid array of `info.mmap_count` MmapEntry.
/// - EmergencyPool must have been initialized before this call (RULE MEM-02).
pub unsafe fn init_memory_subsystem_multiboot2(info: &Multiboot2Info) {
    debug_assert!(info.mmap_count > 0, "Multiboot2 : mmap vide");
    debug_assert!(info.mmap_ptr != 0, "Multiboot2 : mmap_ptr nul");

    // ── Phase 1: detect total physical range ──────────────────────────────────

    // SAFETY: mmap_ptr and mmap_count validated by multiboot2 parser.
    let entries =
        core::slice::from_raw_parts(info.mmap_ptr as *const MmapEntry, info.mmap_count as usize);

    let mut phys_start = u64::MAX;
    let mut phys_end = 0u64;
    let mut region_count = 0usize;

    for entry in entries {
        let base = entry.base_addr;
        let end = base + entry.length;

        // Filter regions below 1 MiB (legacy BIOS / ISA)
        if end <= PHYS_MEMORY_START {
            continue;
        }
        let base_adj = base.max(PHYS_MEMORY_START);

        if base_adj < phys_start {
            phys_start = base_adj;
        }
        if end > phys_end {
            phys_end = end;
        }

        // Populate MEMORY_MAP
        if region_count < MAX_MEMORY_REGIONS {
            let rtype = e820_type_to_region_type(entry.entry_type);
            MEMORY_MAP[region_count] = MemoryRegion {
                base: base_adj,
                size: end.saturating_sub(base_adj),
                region_type: rtype,
            };
            region_count += 1;
        }
    }

    MEMORY_REGION_COUNT = region_count;

    if phys_start == u64::MAX || phys_end == 0 {
        // No RAM found — unrecoverable condition
        crate::arch::x86_64::halt_cpu();
    }

    // Align to pages
    let phys_start_pa = PhysAddr::new(align_up(phys_start, PAGE_SIZE as u64));
    let phys_end_pa = PhysAddr::new(align_down(phys_end, PAGE_SIZE as u64));

    // ── Phase 1: initialize allocator bitmap ──────────────────────────────────
    init_phase1_bitmap(phys_start_pa, phys_end_pa);

    let (mod_start_raw, mod_end_raw) =
        super::multiboot2::get_multiboot2_module().unwrap_or((0, 0));
    let mod_start = align_down(mod_start_raw, PAGE_SIZE as u64);
    let mod_end = align_up(mod_end_raw, PAGE_SIZE as u64);

    // ── Phase 2: free all usable regions ──────────────────────────────────────
    for entry in entries {
        if entry.entry_type != MMAP_AVAILABLE {
            continue;
        }

        let base = entry.base_addr;
        let end = base + entry.length;
        if end <= PHYS_MEMORY_START {
            continue;
        }

        let base_adj = align_up(base.max(kernel_physical_end()), PAGE_SIZE as u64);
        let end_adj = align_down(end.min(PHYS_MEMORY_MAX), PAGE_SIZE as u64);
        if base_adj >= end_adj {
            continue;
        }

        for_each_usable_subrange_excluding_reserved(base_adj, end_adj, mod_start, mod_end, |s, e| {
            init_phase2_free_region(PhysAddr::new(s), PhysAddr::new(e));
        });
    }
    install_extended_physmap(phys_end_pa);

    // ── Phase 2b: Initialize buddy allocator (DMA32 zone, < 4 GiB) ───────────
    // Required by vmalloc/kalloc (allocations > 2 KiB: kernel stacks, etc.).
    // Physmap must be operational (mapped in main.rs trampoline).
    // SAFETY: Single-CPU. BUDDY_DMA32_BITMAP is 'static (BSS, infinite lifetime).
    //         init_phase2b_buddy_zone initializes zone then mark_initialized().
    init_phase2b_buddy_zone(
        phys_start_pa,
        phys_end_pa,
        // SAFETY: mutable static in BSS, 'static lifetime ≥ buddy lifetime.
        buddy_dma32_bitmap_ptr(),
        BUDDY_DMA32_BITMAP_WORDS,
    );
    // Populate buddy with same free regions as bitmap.
    for entry in entries {
        if entry.entry_type != MMAP_AVAILABLE {
            continue;
        }

        let base = entry.base_addr;
        let end = base + entry.length;
        if end <= PHYS_MEMORY_START {
            continue;
        }

        let base_adj = align_up(base.max(kernel_physical_end()), PAGE_SIZE as u64);
        let end_adj = align_down(end.min(PHYS_MEMORY_MAX), PAGE_SIZE as u64);
        if base_adj >= end_adj {
            continue;
        }

        for_each_usable_subrange_excluding_reserved(base_adj, end_adj, mod_start, mod_end, |s, e| {
            init_phase2b_buddy_free_region(PhysAddr::new(s), PhysAddr::new(e));
        });
    }
    assert_buddy_has_free_frames("multiboot2");

    // ── Phase 2.5: Register page provider for slab/slub ───────────────────────
    // SAFETY: provider is 'static and Sync.
    //         It uses buddy as soon as ready, and only falls back
    //         to bootstrap bitmap prior to buddy initialization.
    //         Must be called BEFORE init_phase3_slab_slub().
    register_slab_page_provider(&KERNEL_SLAB_PROVIDER as *const dyn SlabPageProvider);
    // ── Phase 3: Slab / SLUB ──────────────────────────────────────────────────
    init_phase3_slab_slub();

    // ── Phase 4: NUMA (node 0 by default if no ACPI topology yet) ─────────────
    init_phase4_numa(0b0000_0001); // node 0 active
}

/// Initializes the memory subsystem from an UEFI memory map.
///
/// Converts EFI descriptors into memory regions and calls buddy
/// allocator init phases.
///
/// # Safety
/// Same constraints as `init_memory_subsystem_multiboot2`.
pub unsafe fn init_memory_subsystem_uefi(uefi_map: &UefiMemoryMap) {
    let mut phys_start = u64::MAX;
    let mut phys_end = 0u64;
    let mut region_count = 0usize;

    // ── First pass: detect bounds and populate MEMORY_MAP ─────────────────────
    for desc in uefi_map.iter() {
        let base = desc.physical_start;
        let end = base + desc.number_of_pages * PAGE_SIZE as u64;

        if end <= PHYS_MEMORY_START {
            continue;
        }
        let base_adj = base.max(PHYS_MEMORY_START);

        if base_adj < phys_start {
            phys_start = base_adj;
        }
        if end > phys_end {
            phys_end = end;
        }

        if region_count < MAX_MEMORY_REGIONS {
            let rtype = uefi_type_to_region_type(desc.mem_type);
            MEMORY_MAP[region_count] = MemoryRegion {
                base: base_adj,
                size: end.saturating_sub(base_adj),
                region_type: rtype,
            };
            region_count += 1;
        }
    }

    MEMORY_REGION_COUNT = region_count;

    if phys_start == u64::MAX || phys_end == 0 {
        crate::arch::x86_64::halt_cpu();
    }

    let phys_start_pa = PhysAddr::new(align_up(phys_start, PAGE_SIZE as u64));
    let phys_end_pa = PhysAddr::new(align_down(phys_end, PAGE_SIZE as u64));

    // ── Phase 1: allocator bitmap ─────────────────────────────────────────────
    init_phase1_bitmap(phys_start_pa, phys_end_pa);

    // ── Phase 2: free conventional regions ────────────────────────────────────
    for desc in uefi_map.iter() {
        if !desc.is_usable() {
            continue;
        }

        let base = desc.physical_start;
        let end = base + desc.number_of_pages * PAGE_SIZE as u64;
        if end <= PHYS_MEMORY_START {
            continue;
        }

        let base_adj = align_up(base.max(kernel_physical_end()), PAGE_SIZE as u64);
        let end_adj = align_down(end.min(PHYS_MEMORY_MAX), PAGE_SIZE as u64);
        if base_adj >= end_adj {
            continue;
        }

        for_each_usable_subrange_excluding_ssr(base_adj, end_adj, |s, e| {
            init_phase2_free_region(PhysAddr::new(s), PhysAddr::new(e));
        });
    }
    install_extended_physmap(phys_end_pa);
    // ── Phase 2b: Buddy allocator (DMA32 zone, < 4 GiB) ──────────────────────
    init_phase2b_buddy_zone(
        phys_start_pa,
        phys_end_pa,
        buddy_dma32_bitmap_ptr(),
        BUDDY_DMA32_BITMAP_WORDS,
    );
    for desc in uefi_map.iter() {
        if !desc.is_usable() {
            continue;
        }
        let base = desc.physical_start;
        let end = base + desc.number_of_pages * PAGE_SIZE as u64;
        if end <= PHYS_MEMORY_START {
            continue;
        }
        let base_adj = align_up(base.max(kernel_physical_end()), PAGE_SIZE as u64);
        let end_adj = align_down(end.min(PHYS_MEMORY_MAX), PAGE_SIZE as u64);
        if base_adj >= end_adj {
            continue;
        }
        for_each_usable_subrange_excluding_ssr(base_adj, end_adj, |s, e| {
            init_phase2b_buddy_free_region(PhysAddr::new(s), PhysAddr::new(e));
        });
    }
    assert_buddy_has_free_frames("uefi");
    // ── Phase 2.5: Register page provider for slab/slub ───────────────────────
    register_slab_page_provider(&KERNEL_SLAB_PROVIDER as *const dyn SlabPageProvider);
    // ── Phase 3 & 4 ───────────────────────────────────────────────────────────
    init_phase3_slab_slub();
    init_phase4_numa(0b0000_0001);
}

// ─────────────────────────────────────────────────────────────────────────────
// PRIVATE HELPERS
// ─────────────────────────────────────────────────────────────────────────────

/// Converts an E820 type (Multiboot2) to MemoryRegionType.
#[inline]
fn e820_type_to_region_type(e820_type: u32) -> MemoryRegionType {
    match e820_type {
        1 => MemoryRegionType::Usable,
        2 => MemoryRegionType::Reserved,
        3 => MemoryRegionType::AcpiReclaimable,
        4 => MemoryRegionType::AcpiNvs,
        5 => MemoryRegionType::Bad,
        _ => MemoryRegionType::Reserved,
    }
}

/// Converts an UEFI type to MemoryRegionType.
#[inline]
fn uefi_type_to_region_type(uefi_type: u32) -> MemoryRegionType {
    // UEFI Memory types (EFI_MEMORY_TYPE)
    match uefi_type {
        7 => MemoryRegionType::Usable,          // EfiConventionalMemory
        9 => MemoryRegionType::AcpiReclaimable, // EfiACPIReclaimMemory
        10 => MemoryRegionType::AcpiNvs,        // EfiACPIMemoryNVS
        11 => MemoryRegionType::Bad,            // EfiMemoryMappedIO → reserved
        _ => MemoryRegionType::Reserved,
    }
}

/// Rounds `n` up to alignment `align` (must be a power of 2).
#[inline]
const fn align_up(n: u64, align: u64) -> u64 {
    (n + align - 1) & !(align - 1)
}

/// Rounds `n` down to alignment `align` (must be a power of 2).
#[inline]
const fn align_down(n: u64, align: u64) -> u64 {
    n & !(align - 1)
}

/// Applies `f(start, end)` on usable subranges, explicitly excluding
/// the ExoPhoenix SSR [SSR_BASE, SSR_BASE + SSR_SIZE).
///
/// Doc rule: this exclusion must be applied BEFORE any insertion into
/// bitmap/buddy allocators to avoid accidental allocation of the SSR.
#[inline]
fn for_each_usable_subrange_excluding_ssr(start: u64, end: u64, mut f: impl FnMut(u64, u64)) {
    if start >= end {
        return;
    }

    let ssr_start = ssr::SSR_BASE;
    let ssr_end = ssr::SSR_BASE + ssr::SSR_SIZE as u64;

    // No overlap with SSR
    if end <= ssr_start || start >= ssr_end {
        f(start, end);
        return;
    }

    // Lower part before SSR
    if start < ssr_start {
        f(start, ssr_start);
    }

    // Upper part after SSR
    if end > ssr_end {
        f(ssr_end, end);
    }
}

/// Applies `f(start, end)` on usable subranges, excluding both
/// the ExoPhoenix SSR and the Multiboot2 module range (rootfs ramdisk).
#[inline]
fn for_each_usable_subrange_excluding_reserved(
    start: u64,
    end: u64,
    mod_start: u64,
    mod_end: u64,
    mut f: impl FnMut(u64, u64),
) {
    for_each_usable_subrange_excluding_ssr(start, end, |s1, e1| {
        if mod_start >= mod_end || e1 <= mod_start || s1 >= mod_end {
            f(s1, e1);
        } else {
            if s1 < mod_start {
                f(s1, mod_start);
            }
            if e1 > mod_end {
                f(mod_end, e1);
            }
        }
    });
}


// ─────────────────────────────────────────────────────────────────────────────
// INIT FROM EXO-BOOT (native UEFI)
// ─────────────────────────────────────────────────────────────────────────────

/// 64-bit magic of exo-boot BootInfo ("EXOOS_BO" in LE).
pub const EXOBOOT_BOOT_INFO_MAGIC: u64 = 0x4F42_5F53_4F4F_5845;

/// 32-bit magic passed in EAX → RDI by exo-boot before `kernel_main`.
/// Synchronized with exo-boot/src/kernel_loader/handoff.rs::EXOBOOT_MAGIC_U32.
pub const EXOBOOT_MAGIC_U32: u32 = 0x4F4F_5845;

// ── repr(C) shims to read exo-boot BootInfo ──────────────────────────────────

/// exo-boot memory region — repr(C) layout, 24 bytes.
/// Exact mirror of `exo-boot::memory::map::MemoryRegion`.
#[repr(C)]
#[derive(Clone, Copy)]
struct ExoMemRegion {
    base: u64,   // byte  0
    length: u64, // byte  8
    kind: u32,   // byte 16 (MemoryKind repr u32)
    _pad: u32,   // byte 20 (padding → total size 24)
}

/// Minimal header of exo-boot BootInfo.
/// Partial mirror of `exo-boot::kernel_loader::handoff::BootInfo` repr(C, align(4096)).
/// Only used fields are modeled; subsequent fields are ignored.
#[repr(C, align(4096))]
struct ExoBootInfo {
    magic: u64,                          // offset   0
    version: u32,                        // offset   8
    memory_region_count: u32,            // offset  12
    memory_regions: [ExoMemRegion; 256], // offset 16, 256 × 24 = 6144 bytes
    // FramebufferInfo (40 bytes, offset 6160)
    fb_phys_addr: u64,  // offset 6160
    fb_width: u32,      // offset 6168
    fb_height: u32,     // offset 6172
    fb_stride: u32,     // offset 6176
    fb_bpp: u32,        // offset 6180
    fb_format: u32,     // offset 6184 (PixelFormat repr u32)
    _fb_pad: u32,       // offset 6188 (align u64)
    fb_size_bytes: u64, // offset 6192
    acpi_rsdp: u64,     // offset 6200
    entropy: [u8; 64],  // offset 6208
    kernel_physical_base: u64, // offset 6272
    kernel_entry_offset: u64,  // offset 6280
    kernel_elf_phys: u64,      // offset 6288
    kernel_elf_size: u64,      // offset 6296
    boot_flags: u64,           // offset 6304
    boot_tsc: u64,             // offset 6312
    exofs_root_phys: u64,      // offset 6320
    exofs_root_lba: u64,       // offset 6328
    exofs_root_sectors: u64,   // offset 6336
    exofs_data_phys: u64,      // offset 6344
    exofs_data_lba: u64,       // offset 6352
    exofs_data_sectors: u64,   // offset 6360
    disk_guid: [u8; 16],       // offset 6368
    boot_partition_guid: [u8; 16], // offset 6384
    boot_from_usb: u8,         // offset 6400
    _pad_v2: [u8; 7],          // offset 6401
    nvme_controller_phys: u64, // offset 6408
    ahci_controller_phys: u64, // offset 6416
}

/// Converts exo-boot `MemoryKind` (repr u32) to kernel `MemoryRegionType`.
#[inline]
fn exo_kind_to_region_type(kind: u32) -> MemoryRegionType {
    match kind {
        1 => MemoryRegionType::Usable,          // Usable
        2 | 3 => MemoryRegionType::KernelImage, // KernelCode / KernelData
        4 => MemoryRegionType::Usable,          // BootloaderReclaimable → to kernel
        5 => MemoryRegionType::Reserved,        // PageTables (until kernel re-init)
        6 => MemoryRegionType::AcpiReclaimable, // AcpiReclaimable
        7 => MemoryRegionType::AcpiNvs,         // AcpiNvs
        8 => MemoryRegionType::Reserved,        // Reserved
        9 => MemoryRegionType::Reserved,        // Framebuffer
        10 => MemoryRegionType::Reserved,       // Mmio
        _ => MemoryRegionType::Reserved,        // Unknown / others
    }
}

/// Initializes the memory subsystem from BootInfo provided by exo-boot.
///
/// `boot_info_phys` is the value passed by exo-boot in `mb2_info` (RSI of
/// `kernel_main`), namely the identity-mapped physical address of BootInfo.
///
/// # Safety
/// - `boot_info_phys` must point to a valid, 4096-aligned exo-boot BootInfo.
/// - EmergencyPool must have been initialized beforehand (RULE MEM-02).
/// - Unique call from BSP, before SMP.
pub unsafe fn init_memory_subsystem_exoboot(boot_info_phys: u64) {
    debug_assert!(boot_info_phys != 0, "exo-boot: adresse BootInfo nulle");

    // SAFETY: boot_info_phys is identity-mapped (0–4 GiB, covered by exo-boot tables).
    let bi = &*(boot_info_phys as *const ExoBootInfo);

    // Validate magic before any additional access
    if bi.magic != EXOBOOT_BOOT_INFO_MAGIC {
        crate::arch::x86_64::halt_cpu();
    }

    let count = (bi.memory_region_count as usize).min(MAX_MEMORY_REGIONS);
    let mut phys_start = u64::MAX;
    let mut phys_end = 0u64;
    let mut region_count = 0usize;

    // ── First pass: physical bounds + populating MEMORY_MAP ────────────────────
    for i in 0..count {
        let r = &bi.memory_regions[i];
        if r.length == 0 {
            continue;
        }

        let base = r.base;
        let end = base + r.length;
        if end <= PHYS_MEMORY_START {
            continue;
        }

        let base_adj = base.max(PHYS_MEMORY_START);

        if base_adj < phys_start {
            phys_start = base_adj;
        }
        if end > phys_end {
            phys_end = end;
        }

        if region_count < MAX_MEMORY_REGIONS {
            MEMORY_MAP[region_count] = MemoryRegion {
                base: base_adj,
                size: end.saturating_sub(base_adj),
                region_type: exo_kind_to_region_type(r.kind),
            };
            region_count += 1;
        }
    }

    MEMORY_REGION_COUNT = region_count;

    if phys_start == u64::MAX || phys_end == 0 {
        crate::arch::x86_64::halt_cpu();
    }

    let phys_start_pa = PhysAddr::new(align_up(phys_start, PAGE_SIZE as u64));
    let phys_end_pa = PhysAddr::new(align_down(phys_end, PAGE_SIZE as u64));

    // ── Phase 1: allocator bitmap ─────────────────────────────────────────────
    init_phase1_bitmap(phys_start_pa, phys_end_pa);

    // ── Phase 2: free usable regions ──────────────────────────────────────────
    for i in 0..count {
        let r = &bi.memory_regions[i];
        if r.length == 0 {
            continue;
        }

        match exo_kind_to_region_type(r.kind) {
            MemoryRegionType::Usable | MemoryRegionType::AcpiReclaimable => {}
            _ => continue,
        }

        let base = r.base;
        let end = base + r.length;
        if end <= PHYS_MEMORY_START {
            continue;
        }

        let base_adj = align_up(base.max(kernel_physical_end()), PAGE_SIZE as u64);
        let end_adj = align_down(end.min(PHYS_MEMORY_MAX), PAGE_SIZE as u64);
        if base_adj >= end_adj {
            continue;
        }

        for_each_usable_subrange_excluding_ssr(base_adj, end_adj, |s, e| {
            init_phase2_free_region(PhysAddr::new(s), PhysAddr::new(e));
        });
    }
    install_extended_physmap(phys_end_pa);

    // ── Phase 2b: Buddy allocator (DMA32 zone, < 4 GiB) ──────────────────────
    init_phase2b_buddy_zone(
        phys_start_pa,
        phys_end_pa,
        buddy_dma32_bitmap_ptr(),
        BUDDY_DMA32_BITMAP_WORDS,
    );
    for i in 0..count {
        let r = &bi.memory_regions[i];
        if r.length == 0 {
            continue;
        }
        match exo_kind_to_region_type(r.kind) {
            MemoryRegionType::Usable | MemoryRegionType::AcpiReclaimable => {}
            _ => continue,
        }
        let base = r.base;
        let end = base + r.length;
        if end <= PHYS_MEMORY_START {
            continue;
        }
        let base_adj = align_up(base.max(kernel_physical_end()), PAGE_SIZE as u64);
        let end_adj = align_down(end.min(PHYS_MEMORY_MAX), PAGE_SIZE as u64);
        if base_adj >= end_adj {
            continue;
        }
        for_each_usable_subrange_excluding_ssr(base_adj, end_adj, |s, e| {
            init_phase2b_buddy_free_region(PhysAddr::new(s), PhysAddr::new(e));
        });
    }
    assert_buddy_has_free_frames("boot-info");

    // ── Phase 2.5: Register page provider for slab/slub ───────────────────────
    // SAFETY: provider is 'static and Sync.
    //         It uses buddy as soon as ready, and only falls back
    //         to bootstrap bitmap prior to buddy initialization.
    register_slab_page_provider(&KERNEL_SLAB_PROVIDER as *const dyn SlabPageProvider);

    // ── Phase 3: Slab / SLUB ──────────────────────────────────────────────────
    init_phase3_slab_slub();

    // ── Phase 4: NUMA (node 0 by default, topology refined after ACPI) ────────
    init_phase4_numa(0b0000_0001);
}
