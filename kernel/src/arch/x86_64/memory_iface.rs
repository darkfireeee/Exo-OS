// kernel/src/arch/x86_64/memory_iface.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ARCHITECTURE ↔ MEMORY BRIDGE — x86_64
// ═══════════════════════════════════════════════════════════════════════════════
//
// This module is the bidirectional integration point between the
// arch/ subsystem and the memory/ subsystem (Layer 0).
//
// ## Integration Points
//
// ### arch/ → memory/
//   `exceptions::do_page_fault`
//       → `memory::virt::fault::handler::handle_page_fault(ctx, alloc)`
//   `exceptions::do_ipi_tlb_shootdown`
//       → `memory::virt::address_space::tlb::TLB_QUEUE.handle_remote(cpu_id)`
//
// ### memory/ → arch/
//   TLB IPI: `memory::virt::address_space::tlb::register_tlb_ipi_sender()`
//   ← arch registers `send_tlb_ipi_to_mask` at boot (via `init_memory_integration()`)
//   Function-pointer pattern: avoids circular Layer 0 dependency.
//
// ### arch boot → memory init
//   `boot::memory_map::init_memory_subsystem()` → starts the init sequence
//   of the memory subsystem using the bootloader's E820 map.
//
// ## Architecture Rules (DOC2)
//   MEM-01: memory/ may import arch/ for ASM instructions (authorized).
//   TLB-01: flush_local → synchronous IPI ACK → free_pages (never the reverse).
//   MEM-04: Frames are NEVER freed before ACK from all target CPUs.

use core::sync::atomic::{AtomicBool, Ordering};

use crate::memory::core::{AllocError, AllocFlags, Frame, PageFlags, PhysAddr, VirtAddr};
use crate::memory::physical::{alloc_page, free_page};
use crate::memory::virt::address_space::UserAddressSpace;
use crate::memory::virt::fault::handler::FaultAllocator;
use crate::memory::virt::page_table::{FrameAllocatorForWalk, PageTableWalker};

// ─────────────────────────────────────────────────────────────────────────────
// CONSTANTS
// ─────────────────────────────────────────────────────────────────────────────

/// TLB shootdown IPI vector (see kernel IDT).
pub const IPI_TLB_SHOOTDOWN_VECTOR: u8 = super::idt::VEC_IPI_TLB_SHOOTDOWN;

/// Maximum number of supported CPUs (corresponds to percpu::MAX_CPUS).
const MAX_CPUS: usize = super::smp::percpu::MAX_CPUS;

// ─────────────────────────────────────────────────────────────────────────────
// LOW-LEVEL CPU INSTRUCTIONS — exported to memory/
// ─────────────────────────────────────────────────────────────────────────────

/// Returns the content of CR2 (faulting virtual address of last #PF).
///
/// # Safety
/// CPL 0 only.
#[inline(always)]
pub unsafe fn read_cr2() -> u64 {
    let val: u64;
    core::arch::asm!(
        "mov {v}, cr2",
        v = out(reg) val,
        options(nostack, nomem, preserves_flags),
    );
    val
}

/// Reloads CR3 with the current value (non-global TLB flush).
///
/// # Safety
/// CPL 0. Active page tables must remain valid.
#[inline(always)]
pub unsafe fn flush_cr3() {
    let cr3: u64;
    core::arch::asm!(
        "mov {v}, cr3",
        v = out(reg) cr3,
        options(nostack, nomem, preserves_flags),
    );
    core::arch::asm!(
        "mov cr3, {v}",
        v = in(reg) cr3,
        options(nostack, nomem),
    );
}

/// Switches to a new address space by loading `pml4_phys` into CR3.
///
/// # Safety
/// CPL 0. `pml4_phys` must be the physical address of a valid PML4, 4 KiB aligned.
#[inline(always)]
pub unsafe fn switch_cr3(pml4_phys: u64) {
    core::arch::asm!(
        "mov cr3, {v}",
        v = in(reg) pml4_phys & !0xFFFu64,
        options(nostack, nomem),
    );
}

/// Invalidates a TLB entry for `addr` on the current CPU (INVLPG).
///
/// # Safety
/// `addr` must be a canonical x86_64 address.
#[inline(always)]
pub unsafe fn flush_tlb_single(addr: u64) {
    core::arch::asm!(
        "invlpg [{v}]",
        v = in(reg) addr,
        options(nostack),
    );
}

/// Invalidates a range of TLB entries [start, end) on the current CPU.
///
/// # Safety
/// All addresses in [start, end) must be canonical.
#[inline]
pub unsafe fn flush_tlb_range(start: u64, end: u64) {
    const PAGE: u64 = 0x1000;
    let mut addr = start & !(PAGE - 1);
    let end_aligned = (end.wrapping_add(PAGE - 1)) & !(PAGE - 1);
    while addr < end_aligned {
        flush_tlb_single(addr);
        addr = addr.wrapping_add(PAGE);
    }
}

/// Global TLB flush (toggles CR4.PGE) — invalidates even global entries.
///
/// # Safety
/// CPL 0. Must be executed with interrupts disabled.
#[inline]
pub unsafe fn flush_tlb_global() {
    let cr4: u64;
    core::arch::asm!(
        "mov {v}, cr4",
        v = out(reg) cr4,
        options(nostack, nomem, preserves_flags),
    );
    // Clear CR4.PGE (bit 7)
    core::arch::asm!(
        "mov cr4, {v}",
        v = in(reg) cr4 & !(1u64 << 7),
        options(nostack, nomem),
    );
    // Restore CR4.PGE — forces flush of global entries
    core::arch::asm!(
        "mov cr4, {v}",
        v = in(reg) cr4,
        options(nostack, nomem),
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// TLB SHOOTDOWN IPI SENDING
// ─────────────────────────────────────────────────────────────────────────────

/// Sends a TLB shootdown IPI to all CPUs in the mask.
///
/// For each bit `i` of `cpu_mask`, retrieves the `lapic_id` of CPU
/// `base_cpu + i` from the per-CPU table and sends an IPI via the active APIC.
///
/// # Note
/// The current CPU is excluded: local flush was already performed by caller
/// before the call (rule TLB-01 DOC2: flush_local THEN synchronous IPI).
///
/// # Safety
/// - Must be called from memory/ after submitting the request to `TLB_QUEUE`.
/// - `cpu_mask` must represent only CPUs currently online
///   in the window `[base_cpu, base_cpu + 64)`.
unsafe fn send_tlb_ipi_to_mask(base_cpu: usize, cpu_mask: u64) {
    use super::apic::{self, local_apic, x2apic};
    use super::smp::percpu;

    let current = percpu::current_cpu_id() as usize;

    // Iterate over the 64 possible bits of the window.
    for bit in 0..64usize {
        if cpu_mask & (1u64 << bit) == 0 {
            continue;
        }
        let cpu_idx = base_cpu + bit;
        // No TLB self-IPI — local flush was already performed by caller
        if cpu_idx == current || cpu_idx >= MAX_CPUS {
            continue;
        }
        let lapic_id = percpu::per_cpu(cpu_idx).lapic_id as u32;
        // Send fixed TLB shootdown IPI (rule TLB-01 DOC2)
        if apic::is_x2apic() {
            x2apic::send_ipi_x2apic(lapic_id, IPI_TLB_SHOOTDOWN_VECTOR, 0);
        } else if lapic_id <= u8::MAX as u32 {
            local_apic::send_ipi(
                lapic_id as u8,
                IPI_TLB_SHOOTDOWN_VECTOR,
                local_apic::ICR_DM_FIXED,
            );
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ARCH ↔ MEMORY INTEGRATION INITIALIZATION
// ─────────────────────────────────────────────────────────────────────────────

static MEMORY_INTEGRATION_DONE: AtomicBool = AtomicBool::new(false);

/// Initializes bidirectional integration between arch/ and memory/.
///
/// Must be called ONCE from the BSP, after Local APIC initialization
/// and before AP boot.
///
/// ## Operations
/// 1. Registers `send_tlb_ipi_to_mask` as TLB IPI sending function
///    with `memory::virt::address_space::tlb::register_tlb_ipi_sender()`.
///    This function-pointer pattern breaks the circular dependency because
///    memory/ (Layer 0) cannot import arch/ directly for IPIs.
/// 2. (Future) Registers arch callback for per-CPU frame pool init.
///
/// # Safety
/// CPL 0. APIC initialized. memory::virt::address_space already initialized.
pub unsafe fn init_memory_integration() {
    if MEMORY_INTEGRATION_DONE.swap(true, Ordering::SeqCst) {
        return; // Idempotent
    }

    // Register the TLB IPI sender with the TLB shootdown subsystem.
    crate::memory::virt::address_space::tlb::register_tlb_ipi_sender(send_tlb_ipi_to_mask);
}

// ─────────────────────────────────────────────────────────────────────────────
// USERSPACE FAULT ALLOCATOR (UserFaultAllocator)
// ─────────────────────────────────────────────────────────────────────────────

/// FaultAllocator bound to the current process address space.
pub struct UserFaultAllocator<'a> {
    user_as: &'a UserAddressSpace,
}

impl<'a> UserFaultAllocator<'a> {
    pub fn new(user_as: &'a UserAddressSpace) -> Self {
        Self { user_as }
    }
}

impl FrameAllocatorForWalk for UserFaultAllocator<'_> {
    fn alloc_frame(&self, flags: AllocFlags) -> Result<Frame, AllocError> {
        alloc_page(flags)
    }

    fn free_frame(&self, frame: Frame) {
        let _ = free_page(frame);
    }
}

impl FaultAllocator for UserFaultAllocator<'_> {
    #[inline]
    fn alloc_zeroed(&self) -> Result<Frame, AllocError> {
        alloc_page(AllocFlags::ZEROED)
    }

    #[inline]
    fn alloc_nonzeroed(&self) -> Result<Frame, AllocError> {
        alloc_page(AllocFlags::NONE)
    }

    #[inline]
    fn free_frame(&self, f: Frame) {
        let _ = free_page(f);
    }

    fn map_page(&self, virt: VirtAddr, frame: Frame, flags: PageFlags) -> Result<(), AllocError> {
        // SAFETY: do_page_fault constructs this allocator only for the current AS.
        unsafe { self.user_as.map_page(virt, frame, flags, self) }
    }

    fn remap_flags(&self, virt: VirtAddr, flags: PageFlags) -> Result<(), AllocError> {
        let mut walker = PageTableWalker::new(self.user_as.pml4_phys());
        walker.remap_flags(virt, flags)
    }

    #[inline]
    fn translate(&self, virt: VirtAddr) -> Option<PhysAddr> {
        self.user_as.translate(virt)
    }

    fn read_pte_raw(&self, virt: VirtAddr) -> u64 {
        let walker = PageTableWalker::new(self.user_as.pml4_phys());
        walker.read_pte_raw(virt)
    }

    fn compare_exchange_pte_raw(&self, virt: VirtAddr, current: u64, new: u64) -> Result<(), u64> {
        let walker = PageTableWalker::new(self.user_as.pml4_phys());
        // SAFETY: `virt` designates a leaf PTE in the current user space.
        unsafe { walker.compare_exchange_leaf_raw(virt, current, new) }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// KERNEL FAULT ALLOCATOR (KernelFaultAllocator)
// ─────────────────────────────────────────────────────────────────────────────

/// Implementation of `FaultAllocator` + `FrameAllocatorForWalk` for the kernel.
///
/// Used by `do_page_fault` (exceptions.rs) to dispatch to
/// `memory::virt::fault::handler::handle_page_fault()`.
///
/// ## Limitation
/// Maps only into the global kernel address space (`KERNEL_AS`).
/// Ring 3 faults go through `UserFaultAllocator`.
///
/// # Implementation Safety
/// - `map_page` calls `KERNEL_AS.map()` which is `unsafe` because it modifies
///   page tables. It is safe in this context because we always map a
///   canonical kernel address from the fault handler.
/// - `remap_flags` uses `PageTableWalker` directly (lock-free operation).
pub struct KernelFaultAllocator;

impl FrameAllocatorForWalk for KernelFaultAllocator {
    fn alloc_frame(&self, flags: AllocFlags) -> Result<Frame, AllocError> {
        alloc_page(flags)
    }

    fn free_frame(&self, frame: Frame) {
        let _ = free_page(frame);
    }
}

impl FaultAllocator for KernelFaultAllocator {
    #[inline]
    fn alloc_zeroed(&self) -> Result<Frame, AllocError> {
        alloc_page(AllocFlags::ZEROED)
    }

    #[inline]
    fn alloc_nonzeroed(&self) -> Result<Frame, AllocError> {
        alloc_page(AllocFlags::NONE)
    }

    #[inline]
    fn free_frame(&self, f: Frame) {
        let _ = free_page(f);
    }

    fn map_page(&self, virt: VirtAddr, frame: Frame, flags: PageFlags) -> Result<(), AllocError> {
        // SAFETY: virt must be a canonical address; the allocator is valid.
        unsafe { crate::memory::virt::address_space::KERNEL_AS.map(virt, frame, flags, self) }
    }

    fn remap_flags(&self, virt: VirtAddr, flags: PageFlags) -> Result<(), AllocError> {
        use crate::memory::virt::page_table::PageTableWalker;
        let pml4 = crate::memory::virt::address_space::KERNEL_AS.pml4_phys();
        let mut walker = PageTableWalker::new(pml4);
        walker.remap_flags(virt, flags)
    }

    #[inline]
    fn translate(&self, virt: VirtAddr) -> Option<PhysAddr> {
        crate::memory::virt::address_space::KERNEL_AS.translate(virt)
    }

    fn read_pte_raw(&self, virt: VirtAddr) -> u64 {
        let walker = crate::memory::virt::page_table::PageTableWalker::new(
            crate::memory::virt::address_space::KERNEL_AS.pml4_phys(),
        );
        walker.read_pte_raw(virt)
    }

    fn compare_exchange_pte_raw(&self, virt: VirtAddr, current: u64, new: u64) -> Result<(), u64> {
        let walker = crate::memory::virt::page_table::PageTableWalker::new(
            crate::memory::virt::address_space::KERNEL_AS.pml4_phys(),
        );
        // SAFETY: `virt` designates a leaf PTE already present in kernel space.
        unsafe { walker.compare_exchange_leaf_raw(virt, current, new) }
    }
}

/// Global kernel allocator instance for the fault handler.
///
/// Zero overhead: stateless, all operations delegate to
/// memory/ and KERNEL_AS global allocators.
pub static KERNEL_FAULT_ALLOC: KernelFaultAllocator = KernelFaultAllocator;

// ─────────────────────────────────────────────────────────────────────────────
// VMM HOOKS FOR SHM  —  P1-02
// ─────────────────────────────────────────────────────────────────────────────
//
// These two functions are registered with ipc::shared_memory::mapping
// via ipc_install_vmm_hooks() at boot (Phase 6, lib.rs).
//
// They allow shm_map() to map physical SHM pages into
// the address space of a target process identified by its PID.
//
// Signature required by MapPageFn / UnmapPageFn in mapping.rs:
//   map_page_fn  : unsafe fn(phys: u64, virt: u64, flags: u32, pid: u32) -> i32
//   unmap_page_fn: unsafe fn(virt: u64, pid: u32) -> i32

/// VMM Hook — maps `phys` → `virt` in the address space of process `pid`.
///
/// Used by shm_map() and map_shm_into_process() for SHM mappings.
///
/// # Safety
/// `phys` must be a valid SHM physical frame.
/// `virt` must be a canonical user address aligned to PAGE_SIZE.
/// `pid` must correspond to an active process with a valid addr_space_ptr.
pub unsafe fn shm_vmm_map_page(phys: u64, virt: u64, flags: u32, pid: u32) -> i32 {
    use crate::memory::core::AllocError;
    use crate::memory::core::{Frame, PageFlags, PhysAddr, VirtAddr};
    use crate::memory::physical::allocator::buddy;
    use crate::memory::virt::page_table::FrameAllocatorForWalk;
    use crate::process::core::pid::Pid;
    use crate::process::PROCESS_REGISTRY;

    // Retrieve target process address space by PID
    let pcb = match PROCESS_REGISTRY.find_by_pid(Pid(pid)) {
        Some(p) => p,
        None => return -3, // ESRCH
    };
    let as_ptr = pcb
        .address_space
        .load(core::sync::atomic::Ordering::Acquire);
    if as_ptr == 0 {
        return -12; // ENOMEM — uninitialized address space
    }
    let user_as =
        unsafe { &*(as_ptr as *const crate::memory::virt::address_space::UserAddressSpace) };

    // Translate flags (MapPageFn uses simple u32) into kernel PageFlags
    // flags & 0x1 = READ, flags & 0x2 = WRITE
    let page_flags = if flags & 0x2 != 0 {
        PageFlags::PRESENT | PageFlags::WRITABLE | PageFlags::USER | PageFlags::NO_EXECUTE
    } else {
        PageFlags::PRESENT | PageFlags::USER | PageFlags::NO_EXECUTE
    };

    // Intermediate table allocator only (frames come from the SHM pool)
    struct PtOnly;
    impl FrameAllocatorForWalk for PtOnly {
        fn alloc_frame(&self, f: crate::memory::AllocFlags) -> Result<Frame, AllocError> {
            buddy::alloc_pages(0, f)
        }
        fn free_frame(&self, fr: Frame) {
            let _ = buddy::free_pages(fr, 0);
        }
    }

    let frame = Frame::containing(PhysAddr::new(phys));
    let vaddr = VirtAddr::new(virt);

    match unsafe { user_as.map_page(vaddr, frame, page_flags, &PtOnly) } {
        Ok(()) => 0,
        Err(_) => -12, // ENOMEM
    }
}

/// VMM Hook — unmaps `virt` in the address space of process `pid`.
///
/// # Safety
/// `virt` must be a previously mapped canonical user address.
/// `pid` must correspond to an active process.
pub unsafe fn shm_vmm_unmap_page(virt: u64, pid: u32) -> i32 {
    use crate::memory::core::VirtAddr;
    use crate::process::core::pid::Pid;
    use crate::process::PROCESS_REGISTRY;

    let pcb = match PROCESS_REGISTRY.find_by_pid(Pid(pid)) {
        Some(p) => p,
        None => return -3, // ESRCH
    };
    let as_ptr = pcb
        .address_space
        .load(core::sync::atomic::Ordering::Acquire);
    if as_ptr == 0 {
        return -12;
    }
    let user_as =
        unsafe { &*(as_ptr as *const crate::memory::virt::address_space::UserAddressSpace) };

    unsafe { user_as.unmap_page(VirtAddr::new(virt)) };
    0
}
