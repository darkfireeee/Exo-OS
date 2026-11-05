// kernel/src/memory/virtual/page_table/kpti_split.rs
//
// KPTI (Kernel Page-Table Isolation) — separated kernel/user page tables.
// Meltdown mitigation: the kernel has two distinct PML4s.
//   - kernel_pml4: full kernel PML4 (active during syscalls/interrupts)
//   - user_pml4:   minimal user PML4 (active in user space, without kernel mappings)
// Layer 0 — no external dependency except `spin`.

use core::sync::atomic::{AtomicBool, Ordering};

use crate::arch::x86_64::cpu::features::{cpu_features_or_none, CpuVendor};
use crate::memory::core::{AllocError, AllocFlags, PhysAddr, PAGE_SIZE};
use crate::memory::physical::allocator::buddy;
use crate::memory::virt::page_table::x86_64::{write_cr3, PageTable, PageTableEntry};
use crate::memory::virt::page_table::{phys_to_table_mut, phys_to_table_ref};

extern "C" {
    fn syscall_entry_asm();
    fn syscall_cstar_noop();
}

// ─────────────────────────────────────────────────────────────────────────────
// KPTI STATE
// ─────────────────────────────────────────────────────────────────────────────

/// Per-CPU KPTI state.
#[repr(C, align(64))]
pub struct KptiState {
    /// Kernel PML4 (used during exceptions/syscalls).
    pub kernel_pml4: PhysAddr,
    /// Minimal user PML4 (used in user space).
    pub user_pml4: PhysAddr,
    /// Interprocessor return stub (trampoline).
    pub trampoline_phys: PhysAddr,
}

impl KptiState {
    pub const fn empty() -> Self {
        KptiState {
            kernel_pml4: PhysAddr::NULL,
            user_pml4: PhysAddr::NULL,
            trampoline_phys: PhysAddr::NULL,
        }
    }
}

/// Global KPTI table (one entry per CPU, MAX_CPUS=256).
pub struct KptiTable {
    states: [KptiState; 256],
    enabled: AtomicBool,
}

// SAFETY: KptiTable accesses states non-concurrently (each CPU
//         only accesses its own entry).
unsafe impl Sync for KptiTable {}

impl Default for KptiTable {
    fn default() -> Self {
        Self::new()
    }
}

impl KptiTable {
    pub const fn new() -> Self {
        KptiTable {
            states: {
                const EMPTY: KptiState = KptiState::empty();
                [EMPTY; 256]
            },
            enabled: AtomicBool::new(false),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    pub fn enable(&self) {
        self.enabled.store(true, Ordering::Release);
    }

    /// Registers the PML4s for a given CPU.
    ///
    /// # Safety
    /// `cpu_id < 256`, `kernel_pml4` and `user_pml4` are valid PML4s
    /// allocated before this call.
    pub unsafe fn register_cpu(
        &self,
        cpu_id: usize,
        kernel_pml4: PhysAddr,
        user_pml4: PhysAddr,
        trampoline: PhysAddr,
    ) {
        debug_assert!(cpu_id < 256);
        // SAFETY: Exclusive access during CPU init (single-CPU path).
        let state = &self.states[cpu_id] as *const KptiState as *mut KptiState;
        (*state).kernel_pml4 = kernel_pml4;
        (*state).user_pml4 = user_pml4;
        (*state).trampoline_phys = trampoline;
    }

    /// Switches to the kernel PML4 for this CPU.
    ///
    /// # Safety
    /// Must be called at syscall/exception entry with KPTI active.
    #[inline]
    pub unsafe fn switch_to_kernel(&self, cpu_id: usize) {
        if !self.is_enabled() {
            return;
        }
        let pml4 = self.states[cpu_id].kernel_pml4;
        if pml4.as_u64() == 0 {
            log::warn!("KPTI: kernel_pml4 non initialisee pour le CPU {}", cpu_id);
            return;
        }
        write_cr3(pml4);
    }

    /// Switches to the user PML4 for this CPU.
    ///
    /// # Safety
    /// Must be called at syscall/exception exit (before iret/sysret).
    #[inline]
    pub unsafe fn switch_to_user(&self, cpu_id: usize) {
        if !self.is_enabled() {
            return;
        }
        let pml4 = self.states[cpu_id].user_pml4;
        if pml4.as_u64() == 0 {
            log::warn!("KPTI: user_pml4 non initialisee pour le CPU {}", cpu_id);
            return;
        }
        write_cr3(pml4);
    }

    /// Returns the PML4s for a CPU.
    pub fn get_pml4s(&self, cpu_id: usize) -> Option<(PhysAddr, PhysAddr)> {
        if cpu_id >= 256 {
            return None;
        }
        let s = &self.states[cpu_id];
        if s.kernel_pml4.as_u64() == 0 {
            None
        } else {
            Some((s.kernel_pml4, s.user_pml4))
        }
    }

    /// Returns the user CR3 (physical) for a given CPU.
    #[inline]
    pub fn user_cr3_for_cpu(&self, cpu_id: usize) -> Option<u64> {
        if cpu_id >= 256 {
            return None;
        }
        let user = self.states[cpu_id].user_pml4.as_u64();
        if user == 0 {
            None
        } else {
            Some(user)
        }
    }
}

/// Global KPTI table.
pub static KPTI: KptiTable = KptiTable::new();

/// Public helper: returns the user CR3 (physical) of the given CPU.
#[inline]
pub fn user_cr3_for_cpu(cpu_id: usize) -> Option<u64> {
    KPTI.user_cr3_for_cpu(cpu_id)
}

/// Synchronizes the user PML4 entry covering `virt` into a KPTI shadow.
///
/// Lower levels remain shared with the source table for user space.
/// This operation is therefore sufficient when demand paging creates
/// a new PML4 entry after the initial construction of the shadow.
///
/// # Safety
/// `source_pml4_phys` and `user_pml4_phys` must point to valid PML4s.
pub unsafe fn sync_user_pml4_entry(
    source_pml4_phys: PhysAddr,
    user_pml4_phys: PhysAddr,
    virt: u64,
) -> Result<(), AllocError> {
    let (pml4_i, _, _, _) = virt_indices(virt);
    if pml4_i >= 256 {
        return Ok(());
    }

    let source_pml4 = phys_to_table_ref(source_pml4_phys);
    let user_pml4 = phys_to_table_mut(user_pml4_phys);
    user_pml4[pml4_i] = source_pml4[pml4_i];
    Ok(())
}

/// Synchronizes ALL PML4 entries in the user half (0..255) from the process PML4
/// to the per-CPU KPTI shadow.
///
/// Because the user shadow is SHARED across all processes on a CPU, its user
/// entries must reflect the current process at each CR3 change. Without this
/// resynchronization upon switching, a page demand-paged for process A
/// disappears from the shadow as soon as process B runs and yields back to A
/// → #PF loop (cf. init bootstrap after fork).
///
/// # Safety
/// `source_pml4_phys` and `user_pml4_phys` point to valid PML4s (ring0).
#[inline]
pub unsafe fn sync_user_region_to_shadow(source_pml4_phys: PhysAddr, user_pml4_phys: PhysAddr) {
    if source_pml4_phys.as_u64() == 0 || user_pml4_phys.as_u64() == 0 {
        return;
    }
    if source_pml4_phys.as_u64() == user_pml4_phys.as_u64() {
        return;
    }
    let source_pml4 = phys_to_table_ref(source_pml4_phys);
    let user_pml4 = phys_to_table_mut(user_pml4_phys);
    let mut i = 0usize;
    while i < 256 {
        user_pml4[i] = source_pml4[i];
        i += 1;
    }
}

/// Builds a shadow user PML4 from the thread's source PML4.
///
/// The user table retains:
/// - the user half (PML4 entries 0..255)
/// - the minimal supervisor pages necessary for ring3→ring0 transitions
///
/// No full upper kernel entry (including PML4[511]) is copied: only the
/// syscall/exception entry pages, IDT/TSS/per-CPU and strictly necessary
/// entry stacks are explicitly remapped, without the USER bit.
///
/// # Safety
/// Must be called in ring0 when `source_pml4_phys` references a valid PML4.
pub unsafe fn build_user_shadow_pml4(
    source_pml4_phys: PhysAddr,
    cpu_id: usize,
    trampoline_phys: PhysAddr,
    entry_stack_top: u64,
) -> Result<PhysAddr, AllocError> {
    let frame = buddy::alloc_page(AllocFlags::ZEROED)?;
    let user_pml4_phys = frame.start_address();

    let kernel_pml4 = phys_to_table_ref(source_pml4_phys);
    let user_pml4 = phys_to_table_mut(user_pml4_phys);

    // Copy user-space entries (0..255)
    for i in 0..256 {
        user_pml4[i] = kernel_pml4[i];
    }

    map_transition_page(
        source_pml4_phys,
        user_pml4,
        syscall_entry_asm as *const () as u64,
        false,
        true,
    )?;
    map_transition_page(
        source_pml4_phys,
        user_pml4,
        syscall_cstar_noop as *const () as u64,
        false,
        true,
    )?;
    // All handlers installed in the IDT must be executable while
    // CR3 still points to the user PML4: exceptions 0..31, IRQ 32..47,
    // IPIs, spurious IRQ and ExoPhoenix vectors. The canonical source is the IDT,
    // avoiding forgetting a new stub when adding a vector.
    map_idt_handler_pages(source_pml4_phys, user_pml4)?;

    map_transition_page(
        source_pml4_phys,
        user_pml4,
        crate::arch::x86_64::idt::idt_base_addr_for_kpti(),
        false,
        false,
    )?;
    map_transition_page(
        source_pml4_phys,
        user_pml4,
        crate::arch::x86_64::smp::percpu::current_per_cpu() as *const _ as u64,
        true,
        false,
    )?;

    let tss_ptr = crate::arch::x86_64::tss::tss_ptr(cpu_id) as u64;
    map_transition_page(source_pml4_phys, user_pml4, tss_ptr, true, false)?;
    map_current_entry_stacks(source_pml4_phys, user_pml4, cpu_id, entry_stack_top)?;

    if trampoline_phys.as_u64() != 0 {
        map_phys_page(
            user_pml4,
            trampoline_phys.as_u64(),
            trampoline_phys,
            false,
            true,
        )?;
    }

    Ok(user_pml4_phys)
}

const TABLE_FLAGS: u64 = PageTableEntry::FLAG_PRESENT | PageTableEntry::FLAG_WRITABLE;

fn virt_indices(virt: u64) -> (usize, usize, usize, usize) {
    (
        ((virt >> 39) & 0x1ff) as usize,
        ((virt >> 30) & 0x1ff) as usize,
        ((virt >> 21) & 0x1ff) as usize,
        ((virt >> 12) & 0x1ff) as usize,
    )
}

unsafe fn ensure_next_table(
    table: &mut PageTable,
    index: usize,
) -> Result<&'static mut PageTable, AllocError> {
    if !table[index].is_present() {
        let frame = buddy::alloc_page(AllocFlags::ZEROED)?;
        table[index] = PageTableEntry::from_raw(frame.start_address().as_u64() | TABLE_FLAGS);
    }
    if table[index].is_huge() {
        return Err(AllocError::InvalidParams);
    }
    Ok(phys_to_table_mut(table[index].phys_addr()))
}

unsafe fn map_phys_page(
    user_pml4: &mut PageTable,
    virt: u64,
    phys: PhysAddr,
    writable: bool,
    executable: bool,
) -> Result<(), AllocError> {
    let virt_page = virt & !(PAGE_SIZE as u64 - 1);
    let phys_page = PhysAddr::new(phys.as_u64() & !(PAGE_SIZE as u64 - 1));
    let (pml4_i, pdpt_i, pd_i, pt_i) = virt_indices(virt_page);

    let pdpt = ensure_next_table(user_pml4, pml4_i)?;
    let pd = ensure_next_table(pdpt, pdpt_i)?;
    let pt = ensure_next_table(pd, pd_i)?;

    let mut flags = PageTableEntry::FLAG_PRESENT | PageTableEntry::FLAG_GLOBAL;
    if writable {
        flags |= PageTableEntry::FLAG_WRITABLE;
    }
    if !executable {
        flags |= PageTableEntry::FLAG_NO_EXECUTE;
    }
    pt[pt_i] = PageTableEntry::from_raw(phys_page.as_u64() | flags);
    Ok(())
}

unsafe fn map_transition_page(
    kernel_pml4_phys: PhysAddr,
    user_pml4: &mut PageTable,
    virt: u64,
    writable: bool,
    executable: bool,
) -> Result<(), AllocError> {
    let phys = kernel_virt_to_phys(kernel_pml4_phys, virt).ok_or(AllocError::InvalidParams)?;
    map_phys_page(user_pml4, virt, phys, writable, executable)
}

unsafe fn map_idt_handler_pages(
    kernel_pml4_phys: PhysAddr,
    user_pml4: &mut PageTable,
) -> Result<(), AllocError> {
    for vector in 0u16..=255 {
        if let Some(handler) = crate::arch::x86_64::idt::get_handler_addr(vector as u8) {
            if handler != 0 {
                map_transition_page(kernel_pml4_phys, user_pml4, handler, false, true)?;
            }
        }
    }
    Ok(())
}

unsafe fn map_stack_window(
    kernel_pml4_phys: PhysAddr,
    user_pml4: &mut PageTable,
    stack_top: u64,
    pages: usize,
) -> Result<(), AllocError> {
    for page in 1..=pages {
        let Some(virt) = stack_top.checked_sub((page * PAGE_SIZE) as u64) else {
            break;
        };
        map_transition_page(kernel_pml4_phys, user_pml4, virt, true, false)?;
    }
    Ok(())
}

unsafe fn map_current_entry_stacks(
    kernel_pml4_phys: PhysAddr,
    user_pml4: &mut PageTable,
    cpu_id: usize,
    entry_stack_top: u64,
) -> Result<(), AllocError> {
    let tss = crate::arch::x86_64::tss::tss_ptr(cpu_id);
    let rsp: [u64; 3] = core::ptr::addr_of!((*tss).rsp).read_unaligned();
    if rsp[0] != 0 {
        map_stack_window(kernel_pml4_phys, user_pml4, rsp[0], 4)?;
    }
    let ist: [u64; 7] = core::ptr::addr_of!((*tss).ist).read_unaligned();
    for top in ist {
        if top != 0 {
            map_stack_window(kernel_pml4_phys, user_pml4, top, 4)?;
        }
    }

    let kernel_rsp = crate::arch::x86_64::smp::percpu::read_kernel_rsp();
    if kernel_rsp != 0 {
        map_stack_window(kernel_pml4_phys, user_pml4, kernel_rsp, 4)?;
    }
    if entry_stack_top != 0 && entry_stack_top != kernel_rsp && entry_stack_top != rsp[0] {
        map_stack_window(kernel_pml4_phys, user_pml4, entry_stack_top, 4)?;
    }
    Ok(())
}

unsafe fn kernel_virt_to_phys(kernel_pml4_phys: PhysAddr, virt: u64) -> Option<PhysAddr> {
    let (pml4_i, pdpt_i, pd_i, pt_i) = virt_indices(virt);
    let kernel_pml4 = phys_to_table_ref(kernel_pml4_phys);
    let pml4e = kernel_pml4[pml4_i];
    if !pml4e.is_present() {
        return None;
    }

    let pdpt = phys_to_table_ref(pml4e.phys_addr());
    let pdpte = pdpt[pdpt_i];
    if !pdpte.is_present() {
        return None;
    }
    if pdpte.is_huge() {
        let offset = virt & ((1u64 << 30) - 1);
        return Some(PhysAddr::new(pdpte.phys_addr().as_u64() + offset));
    }

    let pd = phys_to_table_ref(pdpte.phys_addr());
    let pde = pd[pd_i];
    if !pde.is_present() {
        return None;
    }
    if pde.is_huge() {
        let offset = virt & ((1u64 << 21) - 1);
        return Some(PhysAddr::new(pde.phys_addr().as_u64() + offset));
    }

    let pt = phys_to_table_ref(pde.phys_addr());
    let pte = pt[pt_i];
    if !pte.is_present() {
        return None;
    }
    Some(PhysAddr::new(
        pte.phys_addr().as_u64() + (virt & (PAGE_SIZE as u64 - 1)),
    ))
}

// ─────────────────────────────────────────────────────────────────────────────
// HELPER: KPTI CPU verification
// ─────────────────────────────────────────────────────────────────────────────

/// Checks if the current CPU supports and requires KPTI.
/// Returns false if Meltdown does not affect this CPU (AMD, post-2018 Intel).
pub fn should_enable_kpti() -> bool {
    let Some(features) = cpu_features_or_none() else {
        return true;
    };

    match features.vendor {
        CpuVendor::Amd => !(features.rdcl_no() || features.amd_noreplay()),
        CpuVendor::Intel => !(features.has_arch_cap() && features.rdcl_no()),
        CpuVendor::Unknown => true,
    }
}
