// kernel/src/arch/x86_64/memory_iface.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// PONT ARCHITECTURE ↔ MÉMOIRE — x86_64
// ═══════════════════════════════════════════════════════════════════════════════
//
// Ce module est le point d'intégration bidirectionnel entre le sous-système
// arch/ et le sous-système memory/ (Couche 0).
//
// ## Points d'intégration
//
// ### arch/ → memory/
//   `exceptions::do_page_fault`
//       → `memory::virt::fault::handler::handle_page_fault(ctx, alloc)`
//   `exceptions::do_ipi_tlb_shootdown`
//       → `memory::virt::address_space::tlb::TLB_QUEUE.handle_remote(cpu_id)`
//
// ### memory/ → arch/
//   TLB IPI : `memory::virt::address_space::tlb::register_tlb_ipi_sender()`
//   ← arch enregistre `send_tlb_ipi_to_mask` au boot (via `init_memory_integration()`)
//   Pattern function-pointer : évite la dépendance circulaire Couche 0.
//
// ### arch boot → memory init
//   `boot::memory_map::init_memory_subsystem()` → démarre la séquence d'init
//   du sous-système mémoire avec la carte E820 du bootloader.
//
// ## Règles architecture (DOC2)
//   MEM-01 : memory/ peut importer arch/ pour instructions ASM (autorisé).
//   TLB-01 : flush_local → IPI synchrone ACK → free_pages (jamais l'inverse).
//   MEM-04 : Les frames NE sont JAMAIS libérées avant ACK de tous les CPUs cibles.


use core::sync::atomic::{AtomicBool, Ordering};

use crate::memory::core::{AllocError, AllocFlags, Frame, PageFlags, PhysAddr, VirtAddr};
use crate::memory::physical::{alloc_page, free_page};
use crate::memory::virt::fault::handler::FaultAllocator;
use crate::memory::virt::page_table::FrameAllocatorForWalk;

// ─────────────────────────────────────────────────────────────────────────────
// CONSTANTES
// ─────────────────────────────────────────────────────────────────────────────

/// Vecteur IPI TLB shootdown (0xF2 — voir IDT du kernel).
pub const IPI_TLB_SHOOTDOWN_VECTOR: u8 = 0xF2;

/// Nombre maximum de CPUs supportés (correspond à percpu::MAX_CPUS).
const MAX_CPUS: usize = super::smp::percpu::MAX_CPUS;

// ─────────────────────────────────────────────────────────────────────────────
// INSTRUCTIONS CPU BAS NIVEAU — exportées vers memory/
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne le contenu de CR2 (adresse virtuelle fautive du dernier #PF).
///
/// # Safety
/// CPL 0 uniquement.
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

/// Recharge CR3 avec la valeur actuelle (flush TLB non-global).
///
/// # Safety
/// CPL 0. Les page tables actives doivent rester valides.
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

/// Bascule vers un nouvel espace d'adressage en chargeant `pml4_phys` dans CR3.
///
/// # Safety
/// CPL 0. `pml4_phys` doit être la physaddr d'une PML4 valide, alignée sur 4 KiB.
#[inline(always)]
pub unsafe fn switch_cr3(pml4_phys: u64) {
    core::arch::asm!(
        "mov cr3, {v}",
        v = in(reg) pml4_phys & !0xFFFu64,
        options(nostack, nomem),
    );
}

/// Invalide une entrée TLB pour `addr` sur le CPU courant (INVLPG).
///
/// # Safety
/// `addr` doit être une adresse canonique x86_64.
#[inline(always)]
pub unsafe fn flush_tlb_single(addr: u64) {
    core::arch::asm!(
        "invlpg [{v}]",
        v = in(reg) addr,
        options(nostack),
    );
}

/// Invalide une plage d'entrées TLB [start, end) sur le CPU courant.
///
/// # Safety
/// Toutes les adresses dans [start, end) doivent être canoniques.
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

/// Flush TLB global (toggle CR4.PGE) — invalide même les entrées globales.
///
/// # Safety
/// CPL 0. Doit être exécuté avec interruptions désactivées.
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
    // Rétablir CR4.PGE — force le flush des entrées globales
    core::arch::asm!(
        "mov cr4, {v}",
        v = in(reg) cr4,
        options(nostack, nomem),
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// ENVOI D'IPI TLB SHOOTDOWN
// ─────────────────────────────────────────────────────────────────────────────

/// Envoie un IPI TLB shootdown (vecteur 0xF2) à tous les CPUs du masque.
///
/// Pour chaque bit `i` de `cpu_mask`, récupère le `lapic_id` du CPU `i`
/// depuis la table per-CPU et envoie un IPI via le Local APIC.
///
/// # Note
/// Le CPU courant est exclu : le flush local a déjà été effectué par l'émetteur
/// avant l'appel (règle TLB-01 DOC2 : flush_local PUIS IPI synchrone).
///
/// # Safety
/// - Doit être appelé depuis memory/ après dépôt de la requête dans `TLB_QUEUE`.
/// - `cpu_mask` doit représenter uniquement les CPUs actuellement en ligne.
unsafe fn send_tlb_ipi_to_mask(cpu_mask: u64) {
    use super::apic::local_apic;
    use super::smp::percpu;

    let current = percpu::current_cpu_id() as usize;

    // Itérer sur les 64 bits possibles du masque
    for cpu_idx in 0..64usize {
        if cpu_mask & (1u64 << cpu_idx) == 0 {
            continue;
        }
        // Pas de self-IPI TLB — le flush local a déjà été effectué par l'émetteur
        if cpu_idx == current || cpu_idx >= MAX_CPUS {
            continue;
        }
        let lapic_id = percpu::per_cpu(cpu_idx).lapic_id as u8;
        // Envoi IPI fixe vecteur 0xF2 (TLB shootdown, règle TLB-01 DOC2)
        local_apic::send_ipi(lapic_id, IPI_TLB_SHOOTDOWN_VECTOR, local_apic::ICR_DM_FIXED);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// INITIALISATION DE L'INTÉGRATION ARCH ↔ MEMORY
// ─────────────────────────────────────────────────────────────────────────────

static MEMORY_INTEGRATION_DONE: AtomicBool = AtomicBool::new(false);

/// Initialise l'intégration bidirectionnelle entre arch/ et memory/.
///
/// Doit être appelé UNE SEULE FOIS depuis le BSP, après l'initialisation
/// du Local APIC et avant le démarrage des APs.
///
/// ## Opérations
/// 1. Enregistre `send_tlb_ipi_to_mask` comme fonction d'envoi d'IPI TLB
///    auprès de `memory::virt::address_space::tlb::register_tlb_ipi_sender()`.
///    Ce pattern function-pointer rompt la dépendance circulaire car
///    memory/ (Couche 0) ne peut pas importer arch/ directement pour l'IPI.
/// 2. (Futur) Enregistre le callback arch pour le per-CPU frame pool init.
///
/// # Safety
/// CPL 0. APIC initialisé. memory::virt::address_space déjà initialisé.
pub unsafe fn init_memory_integration() {
    if MEMORY_INTEGRATION_DONE.swap(true, Ordering::SeqCst) {
        return; // Idempotent
    }

    // Enregistrer l'envoyeur d'IPI TLB auprès du sous-système TLB shootdown.
    crate::memory::virt::address_space::tlb::register_tlb_ipi_sender(
        send_tlb_ipi_to_mask,
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// ALLOCATEUR FAULT KERNEL (KernelFaultAllocator)
// ─────────────────────────────────────────────────────────────────────────────

/// Implémentation de `FaultAllocator` + `FrameAllocatorForWalk` pour le kernel.
///
/// Utilisée par `do_page_fault` (exceptions.rs) pour dispatcher vers
/// `memory::virt::fault::handler::handle_page_fault()`.
///
/// ## Limitations
/// Mappe uniquement dans l'espace d'adressage kernel global (`KERNEL_AS`).
/// Quand `process/` sera intégré, les faults utilisateur utiliseront
/// un allocateur lié à l'espace d'adressage du processus courant.
///
/// # Safety de l'implémentation
/// - `map_page` appelle `KERNEL_AS.map()` qui est `unsafe` car elle modifie
///   les page tables. Elle est safe dans ce contexte car on mappe toujours
///   une adresse canonique noyau depuis le fault handler.
/// - `remap_flags` utilise `PageTableWalker` directement (opération lock-free).
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

    fn map_page(
        &self,
        virt:  VirtAddr,
        frame: Frame,
        flags: PageFlags,
    ) -> Result<(), AllocError> {
        // SAFETY: virt doit être une adresse canonique ; l'allocateur est valide.
        unsafe {
            crate::memory::virt::address_space::KERNEL_AS.map(virt, frame, flags, self)
        }
    }

    fn remap_flags(
        &self,
        virt:  VirtAddr,
        flags: PageFlags,
    ) -> Result<(), AllocError> {
        use crate::memory::virt::page_table::PageTableWalker;
        let pml4 = crate::memory::virt::address_space::KERNEL_AS.pml4_phys();
        let mut walker = PageTableWalker::new(pml4);
        walker.remap_flags(virt, flags)
    }

    #[inline]
    fn translate(&self, virt: VirtAddr) -> Option<PhysAddr> {
        crate::memory::virt::address_space::KERNEL_AS.translate(virt)
    }
}

/// Instance globale de l'allocateur kernel pour le fault handler.
///
/// Zéro overhead : pas d'état, toutes les opérations délèguent vers les
/// allocateurs globaux memory/ et KERNEL_AS.
pub static KERNEL_FAULT_ALLOC: KernelFaultAllocator = KernelFaultAllocator;

// ─────────────────────────────────────────────────────────────────────────────
// VMM HOOKS POUR SHM  —  P1-02
// ─────────────────────────────────────────────────────────────────────────────
//
// Ces deux fonctions sont enregistrées auprès de ipc::shared_memory::mapping
// via ipc_install_vmm_hooks() au boot (Phase 6, lib.rs).
//
// Elles permettent à shm_map() de mapper des pages physiques SHM dans
// l'espace d'adressage d'un processus cible identifié par son PID.
//
// Signature imposée par MapPageFn / UnmapPageFn dans mapping.rs :
//   map_page_fn  : unsafe fn(phys: u64, virt: u64, flags: u32, pid: u32) -> i32
//   unmap_page_fn: unsafe fn(virt: u64, pid: u32) -> i32

/// Hook VMM — mappe `phys` → `virt` dans l'espace d'adressage du processus `pid`.
///
/// Utilisé par shm_map() et map_shm_into_process() pour les mappages SHM.
///
/// # Safety
/// `phys` doit être une frame physique SHM valide.
/// `virt` doit être une adresse user canonique alignée sur PAGE_SIZE.
/// `pid`  doit correspondre à un processus actif avec un addr_space_ptr valide.
pub unsafe fn shm_vmm_map_page(phys: u64, virt: u64, flags: u32, pid: u32) -> i32 {
    use crate::memory::core::{PhysAddr, VirtAddr, Frame, PageFlags};
    use crate::memory::physical::allocator::buddy;
    use crate::memory::virt::page_table::FrameAllocatorForWalk;
    use crate::memory::core::AllocError;
    use crate::process::PROCESS_REGISTRY;
    use crate::process::core::pid::Pid;

    // Retrouver l'espace d'adressage du processus cible par PID
    let pcb = match PROCESS_REGISTRY.find_by_pid(Pid(pid)) {
        Some(p) => p,
        None    => return -3, // ESRCH
    };
    let as_ptr = pcb.address_space.load(core::sync::atomic::Ordering::Acquire);
    if as_ptr == 0 {
        return -12; // ENOMEM — espace non initialisé
    }
    let user_as = unsafe {
        &*(as_ptr as *const crate::memory::virt::address_space::UserAddressSpace)
    };

    // Traduire les flags (MapPageFn utilise u32 simple) en PageFlags kernel
    // flags & 0x1 = READ, flags & 0x2 = WRITE
    let page_flags = if flags & 0x2 != 0 {
        PageFlags::PRESENT | PageFlags::WRITABLE | PageFlags::USER | PageFlags::NO_EXECUTE
    } else {
        PageFlags::PRESENT | PageFlags::USER | PageFlags::NO_EXECUTE
    };

    // Allocateur de tables intermédiaires uniquement (les frames viennent du pool SHM)
    struct PtOnly;
    impl FrameAllocatorForWalk for PtOnly {
        fn alloc_frame(&self, f: crate::memory::AllocFlags) -> Result<Frame, AllocError> {
            buddy::alloc_pages(0, f)
        }
        fn free_frame(&self, fr: Frame) { let _ = buddy::free_pages(fr, 0); }
    }

    let frame = Frame::containing(PhysAddr::new(phys));
    let vaddr = VirtAddr::new(virt);

    match unsafe { user_as.map_page(vaddr, frame, page_flags, &PtOnly) } {
        Ok(())  => 0,
        Err(_)  => -12, // ENOMEM
    }
}

/// Hook VMM — démappe `virt` dans l'espace d'adressage du processus `pid`.
///
/// # Safety
/// `virt` doit être une adresse user canonique préalablement mappée.
/// `pid`  doit correspondre à un processus actif.
pub unsafe fn shm_vmm_unmap_page(virt: u64, pid: u32) -> i32 {
    use crate::memory::core::VirtAddr;
    use crate::process::PROCESS_REGISTRY;
    use crate::process::core::pid::Pid;

    let pcb = match PROCESS_REGISTRY.find_by_pid(Pid(pid)) {
        Some(p) => p,
        None    => return -3, // ESRCH
    };
    let as_ptr = pcb.address_space.load(core::sync::atomic::Ordering::Acquire);
    if as_ptr == 0 {
        return -12;
    }
    let user_as = unsafe {
        &*(as_ptr as *const crate::memory::virt::address_space::UserAddressSpace)
    };

    unsafe { user_as.unmap_page(VirtAddr::new(virt)) };
    0
}
