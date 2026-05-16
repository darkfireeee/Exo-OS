// kernel/src/memory/virtual/page_table/kpti_split.rs
//
// KPTI (Kernel Page-Table Isolation) — tables de pages noyau/user séparées.
// Mitigation Meltdown : le noyau a deux PML4 distincts.
//   - kernel_pml4 : PML4 complète noyau (active pendant les syscalls/interruptions)
//   - user_pml4   : PML4 minimale user (active en espace user, sans mappings kernel)
// Couche 0 — aucune dépendance externe sauf `spin`.

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
// ÉTAT KPTI
// ─────────────────────────────────────────────────────────────────────────────

/// État KPTI par CPU.
#[repr(C, align(64))]
pub struct KptiState {
    /// PML4 noyau (utilisée pendant les exceptions/syscalls).
    pub kernel_pml4: PhysAddr,
    /// PML4 user minimal (utilisée en espace user).
    pub user_pml4: PhysAddr,
    /// Stub de retour interprocesseur (trampoline).
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

/// Table KPTI globale (une entrée par CPU, MAX_CPUS=256).
pub struct KptiTable {
    states: [KptiState; 256],
    enabled: AtomicBool,
}

// SAFETY: KptiTable accède aux states de manière non concurrente (chaque CPU
//         accède uniquement à sa propre entrée).
unsafe impl Sync for KptiTable {}

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

    /// Enregistre les PML4 pour un CPU donné.
    ///
    /// SAFETY: `cpu_id < 256`, `kernel_pml4` et `user_pml4` sont des PML4
    ///         valides allouées avant cet appel.
    pub unsafe fn register_cpu(
        &self,
        cpu_id: usize,
        kernel_pml4: PhysAddr,
        user_pml4: PhysAddr,
        trampoline: PhysAddr,
    ) {
        debug_assert!(cpu_id < 256);
        // SAFETY: Accès exclusif pendant l'init CPU (single-CPU path).
        let state = &self.states[cpu_id] as *const KptiState as *mut KptiState;
        (*state).kernel_pml4 = kernel_pml4;
        (*state).user_pml4 = user_pml4;
        (*state).trampoline_phys = trampoline;
    }

    /// Switch vers la PML4 noyau pour ce CPU.
    ///
    /// SAFETY: Doit être appelé en entrée de syscall/exception avec KPTI actif.
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

    /// Switch vers la PML4 user pour ce CPU.
    ///
    /// SAFETY: Doit être appelé en sortie de syscall/exception (avant iret/sysret).
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

    /// Retourne les PML4 pour un CPU.
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

    /// Retourne le CR3 user (physique) pour un CPU donné.
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

/// Table KPTI globale.
pub static KPTI: KptiTable = KptiTable::new();

/// Helper public: retourne le CR3 user (physique) du CPU donné.
#[inline]
pub fn user_cr3_for_cpu(cpu_id: usize) -> Option<u64> {
    KPTI.user_cr3_for_cpu(cpu_id)
}

/// Construit une PML4 user shadow à partir de la PML4 source du thread.
///
/// La table user conserve:
/// - la moitié user (entrées PML4 0..255)
/// - les pages supervisor minimales nécessaires aux transitions ring3→ring0
///
/// Aucune entrée kernel haute entière (dont PML4[511]) n'est copiée : seules les
/// pages d'entrée syscall/exception, IDT/TSS/per-CPU et piles d'entrée strictement
/// nécessaires sont remappées explicitement, sans bit USER.
///
/// # Safety
/// Doit être appelé en ring0 quand `source_pml4_phys` référence une PML4 valide.
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

    // Copier les entrées user-space (0..255)
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
    // Tous les handlers installés dans l'IDT doivent être exécutables pendant
    // que CR3 pointe encore vers la PML4 user: exceptions 0..31, IRQ 32..47,
    // IPIs, spurious IRQ et vecteurs ExoPhoenix. La source canonique est l'IDT,
    // ce qui évite d'oublier un nouveau stub lors d'un ajout de vecteur.
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
// HELPER : Vérification CPU KPTI
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie si le CPU courant supporte et requiert KPTI.
/// Retourne false si Meltdown n'affecte pas ce CPU (AMD, post-2018 Intel).
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
