// kernel/src/memory/virtual/address_space/fork_impl.rs
//
// Implémentation de AddressSpaceCloner pour l'espace utilisateur.
// CORRECTION P0-01 : débloque fork() en clonant l'espace d'adressage en CoW.
//
// Cette implémentation :
//   1. Alloue un nouveau PML4 pour le processus fils
//   2. Copie les entrées kernel (PML4[256:512]) depuis le parent
//   3. Copie les pages userspace et les marque CoW
//   4. Fournit les primitives de libération pour les chemins d'erreur

use crate::memory::core::{AllocError, AllocFlags, Frame, PhysAddr};
use crate::memory::cow::tracker::COW_TRACKER;
use crate::memory::physical::allocator::buddy;
use crate::memory::virt::address_space::tlb::flush_all;
use crate::memory::virt::page_table::builder::PageTableBuilder;
use crate::memory::virt::page_table::walker::FrameAllocatorForWalk;
use crate::memory::virt::page_table::x86_64::{
    phys_to_table_mut, phys_to_table_ref, PageTableEntry,
};
use crate::memory::virt::UserAddressSpace;
use alloc::alloc::{alloc, Layout};
use alloc::boxed::Box;

fn try_box_new<T>(value: T) -> Option<Box<T>> {
    let layout = Layout::new::<T>();
    if layout.size() == 0 {
        return None;
    }

    // SAFETY: `layout` matches T. Null is converted to None, and a non-null
    // allocation is initialized exactly once before Box takes ownership.
    let raw = unsafe { alloc(layout) as *mut T };
    if raw.is_null() {
        return None;
    }
    // SAFETY: `raw` is a unique allocation large enough for T.
    unsafe {
        raw.write(value);
        Some(Box::from_raw(raw))
    }
}

#[inline]
fn track_cow_frame(frame: Frame) -> Result<(), AddrSpaceCloneError> {
    COW_TRACKER
        .try_inc(frame)
        .map(|_| {
            // FIX-#25-PIN (boot-hang bisect, CKP flagrant délit) : épingler chaque
            // frame CoW-partagée contre toute libération/réallocation buddy.
            //
            // Preuve CKP (last_output_4) : la page de pile d'init (frame privée
            // post-CoW-break) est RÉÉCRITE pendant son nanosleep (~1139 octets,
            // zéros + struct — l'empreinte de la pile d'ipc_router). Pour que les
            // écritures d'ipc_router atterrissent physiquement sur une frame
            // vivante d'init, il faut que cette frame ait été rendue au buddy par
            // erreur puis réattribuée (H2-buddy), ou que la PTE d'init ait été
            // remappée (H1 — détecté par <CKPpte>).
            //
            // L'épinglage ferme H2-buddy : reserve_frame() pose PINNED|RESERVED →
            // buddy::free_pages() REFUSE toute libération ultérieure de ces frames
            // (marqueur <25RSVD-REFUSE>) → elles ne peuvent jamais être réattribuées
            // à un autre processus. Coût : quelques frames « orphelines de CoW »
            // fuient à chaque spawn (~2 pages/service) — négligeable au boot, et
            // chaque refus est un marqueur diagnostic qui désigne le fautif.
            crate::memory::physical::allocator::buddy::reserve_frame(frame);
        })
        .map_err(|_| AddrSpaceCloneError::OutOfMemory)
}

struct ForkWalkAllocator;

impl FrameAllocatorForWalk for ForkWalkAllocator {
    fn alloc_frame(&self, flags: AllocFlags) -> Result<Frame, AllocError> {
        buddy::alloc_pages(0, flags)
    }

    fn free_frame(&self, frame: Frame) {
        let _ = buddy::free_pages(frame, 0);
    }
}

/// Résultat de la duplication CoW de l'espace d'adressage.
pub struct ClonedAddressSpace {
    /// CR3 du nouvel espace d'adressage (fils).
    pub cr3: u64,
    /// Pointeur opaque vers le UserAddressSpace fils.
    pub addr_space_ptr: usize,
}

/// Erreur de clonage de l'espace d'adressage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddrSpaceCloneError {
    OutOfMemory,
    InvalidSource,
}

/// Trait injecté dans process/ pour dupliquer un espace d'adressage en CoW.
pub trait AddressSpaceCloner: Send + Sync {
    /// Clone l'espace d'adressage référencé par `src_cr3`.
    fn clone_cow(
        &self,
        src_cr3: u64,
        src_space_ptr: usize,
    ) -> Result<ClonedAddressSpace, AddrSpaceCloneError>;

    /// Flush le TLB d'un espace d'adressage après marquage CoW.
    fn flush_tlb_after_fork(&self, cr3: u64);

    /// Libère un espace d'adressage cloné (appelé sur erreur post-clone).
    fn free_addr_space(&self, addr_space_ptr: usize);
}

pub struct KernelAddressSpaceCloner;

unsafe impl Send for KernelAddressSpaceCloner {}
unsafe impl Sync for KernelAddressSpaceCloner {}

impl AddressSpaceCloner for KernelAddressSpaceCloner {
    fn clone_cow(
        &self,
        src_cr3: u64,
        src_space_ptr: usize,
    ) -> Result<ClonedAddressSpace, AddrSpaceCloneError> {
        if src_cr3 == 0 {
            return Err(AddrSpaceCloneError::InvalidSource);
        }
        let child_pml4_frame = buddy::alloc_pages(0, crate::memory::AllocFlags::ZEROED)
            .map_err(|_| AddrSpaceCloneError::OutOfMemory)?;
        let child_cr3 = child_pml4_frame.start_address().as_u64();
        let child_pml4_phys = PhysAddr::new(child_cr3);

        unsafe {
            let src_pml4 = phys_to_table_ref(PhysAddr::new(src_cr3));
            let dst_pml4 = phys_to_table_mut(child_pml4_phys);
            for i in 256..512 {
                dst_pml4[i] = src_pml4[i];
            }
            if clone_userspace_tables(PhysAddr::new(src_cr3), child_pml4_phys).is_err() {
                free_userspace_tables(child_pml4_phys);
                return Err(AddrSpaceCloneError::OutOfMemory);
            }
            let fork_alloc = ForkWalkAllocator;
            if PageTableBuilder::from_existing(PhysAddr::new(src_cr3), &fork_alloc)
                .remap_low_kernel_identity()
                .is_err()
            {
                free_userspace_tables(child_pml4_phys);
                return Err(AddrSpaceCloneError::OutOfMemory);
            }
            if PageTableBuilder::from_existing(child_pml4_phys, &fork_alloc)
                .remap_low_kernel_identity()
                .is_err()
            {
                free_userspace_tables(child_pml4_phys);
                return Err(AddrSpaceCloneError::OutOfMemory);
            }
        }

        let parent_as = if src_space_ptr != 0 {
            Some(unsafe { &*(src_space_ptr as *const UserAddressSpace) })
        } else {
            None
        };
        if let Some(parent_as) = parent_as {
            parent_as.mark_all_writable_vmas_cow();
        }

        let inherited_heap_start = parent_as
            .map(|parent_as| {
                parent_as
                    .heap_start
                    .load(core::sync::atomic::Ordering::Acquire)
            })
            .unwrap_or(0);
        let inherited_heap_end = parent_as
            .map(|parent_as| {
                parent_as
                    .heap_end
                    .load(core::sync::atomic::Ordering::Acquire)
            })
            .unwrap_or(0);

        let child_as = match try_box_new(UserAddressSpace::new(child_pml4_phys, 0)) {
            Some(addr_space) => addr_space,
            None => {
                unsafe {
                    free_userspace_tables(child_pml4_phys);
                }
                return Err(AddrSpaceCloneError::OutOfMemory);
            }
        };
        if let Some(parent_as) = parent_as {
            if !parent_as.clone_inner_for_fork(&child_as) {
                unsafe {
                    free_userspace_tables(child_pml4_phys);
                }
                return Err(AddrSpaceCloneError::OutOfMemory);
            }
        }
        if inherited_heap_start != 0 || inherited_heap_end != 0 {
            child_as
                .heap_start
                .store(inherited_heap_start, core::sync::atomic::Ordering::Release);
            child_as
                .heap_end
                .store(inherited_heap_end, core::sync::atomic::Ordering::Release);
        }

        Ok(ClonedAddressSpace {
            cr3: child_cr3,
            addr_space_ptr: Box::into_raw(child_as) as usize,
        })
    }

    fn flush_tlb_after_fork(&self, _parent_cr3: u64) {
        unsafe {
            flush_all();
        }
    }

    fn free_addr_space(&self, addr_space_ptr: usize) {
        if addr_space_ptr == 0 {
            return;
        }
        let addr_space = unsafe { Box::from_raw(addr_space_ptr as *mut UserAddressSpace) };
        unsafe {
            free_userspace_tables(addr_space.pml4_phys());
        }
    }
}

/// Instance statique du clonage d'espace d'adressage.
pub static KERNEL_AS_CLONER: KernelAddressSpaceCloner = KernelAddressSpaceCloner;

unsafe fn clone_userspace_tables(
    src_pml4_phys: PhysAddr,
    dst_pml4_phys: PhysAddr,
) -> Result<(), AddrSpaceCloneError> {
    let src_pml4 = phys_to_table_ref(src_pml4_phys);
    let dst_pml4 = phys_to_table_mut(dst_pml4_phys);

    for l4_idx in 0..256 {
        let src_entry = src_pml4[l4_idx];
        if !src_entry.is_present() {
            continue;
        }
        if !src_entry.is_user() {
            dst_pml4[l4_idx] = src_entry;
            continue;
        }

        let dst_pdpt_phys = alloc_zeroed_table()?;
        if let Err(err) = clone_pdpt(src_entry.phys_addr(), dst_pdpt_phys) {
            free_pdpt_tree(dst_pdpt_phys);
            return Err(err);
        }
        dst_pml4[l4_idx] = repoint_table_entry(src_entry, dst_pdpt_phys);
    }

    Ok(())
}

unsafe fn clone_pdpt(
    src_pdpt_phys: PhysAddr,
    dst_pdpt_phys: PhysAddr,
) -> Result<(), AddrSpaceCloneError> {
    let src_pdpt = phys_to_table_mut(src_pdpt_phys);
    let dst_pdpt = phys_to_table_mut(dst_pdpt_phys);

    for l3_idx in 0..512 {
        let src_entry = src_pdpt[l3_idx];
        if !src_entry.is_present() {
            continue;
        }
        if !src_entry.is_user() {
            dst_pdpt[l3_idx] = src_entry;
            continue;
        }
        if src_entry.is_huge() {
            if let Some(frame) = src_entry.frame() {
                track_cow_frame(frame)?;
            }
            let shared = shared_leaf_entry(src_entry);
            src_pdpt[l3_idx] = shared;
            dst_pdpt[l3_idx] = shared;
            continue;
        }

        let dst_pd_phys = alloc_zeroed_table()?;
        if let Err(err) = clone_pd(src_entry.phys_addr(), dst_pd_phys) {
            free_pd_tree(dst_pd_phys);
            return Err(err);
        }
        dst_pdpt[l3_idx] = repoint_table_entry(src_entry, dst_pd_phys);
    }

    Ok(())
}

unsafe fn clone_pd(
    src_pd_phys: PhysAddr,
    dst_pd_phys: PhysAddr,
) -> Result<(), AddrSpaceCloneError> {
    let src_pd = phys_to_table_mut(src_pd_phys);
    let dst_pd = phys_to_table_mut(dst_pd_phys);

    for l2_idx in 0..512 {
        let src_entry = src_pd[l2_idx];
        if !src_entry.is_present() {
            continue;
        }
        if !src_entry.is_user() {
            dst_pd[l2_idx] = src_entry;
            continue;
        }
        if src_entry.is_huge() {
            if let Some(frame) = src_entry.frame() {
                track_cow_frame(frame)?;
            }
            let shared = shared_leaf_entry(src_entry);
            src_pd[l2_idx] = shared;
            dst_pd[l2_idx] = shared;
            continue;
        }

        let dst_pt_phys = alloc_zeroed_table()?;
        if let Err(err) = clone_pt(src_entry.phys_addr(), dst_pt_phys) {
            free_pt_tree(dst_pt_phys);
            return Err(err);
        }
        dst_pd[l2_idx] = repoint_table_entry(src_entry, dst_pt_phys);
    }

    Ok(())
}

unsafe fn clone_pt(
    src_pt_phys: PhysAddr,
    dst_pt_phys: PhysAddr,
) -> Result<(), AddrSpaceCloneError> {
    let src_pt = phys_to_table_mut(src_pt_phys);
    let dst_pt = phys_to_table_mut(dst_pt_phys);

    for l1_idx in 0..512 {
        let src_entry = src_pt[l1_idx];
        if src_entry.is_present() {
            if !src_entry.is_user() {
                dst_pt[l1_idx] = src_entry;
                continue;
            }
            if let Some(frame) = src_entry.frame() {
                #[cfg(target_arch = "x86_64")]
                crate::memory::physical::allocator::buddy::diag25_trace(
                    b"<25CLPT ",
                    frame.phys_addr().as_u64(),
                    1,
                );
                track_cow_frame(frame)?;
            }
            let shared = shared_leaf_entry(src_entry);
            src_pt[l1_idx] = shared;
            dst_pt[l1_idx] = shared;
        }
    }
    Ok(())
}

#[inline]
fn shared_leaf_entry(src_entry: PageTableEntry) -> PageTableEntry {
    // Une entrée devient CoW seulement si elle était inscriptible (ou déjà CoW).
    // Les pages ELF RO ne doivent jamais porter FLAG_COW : le gestionnaire #PF
    // traite ce flag comme une autorisation de copie puis rend la nouvelle page
    // inscriptible. Le poser sur le code/rodata contournerait donc la protection
    // de la VMA après un fork.
    //
    // Chaque frame cloné reste suivi par COW_TRACKER, y compris les pages RO.
    // `release_leaf_frame` s'appuie sur ce compteur, pas sur FLAG_COW, pour
    // savoir si la dernière référence connue peut être libérée.
    if src_entry.is_present()
        && src_entry.is_user()
        && (src_entry.is_writable() || src_entry.is_cow())
    {
        PageTableEntry::from_raw(
            (src_entry.raw() & !PageTableEntry::FLAG_WRITABLE) | PageTableEntry::FLAG_COW,
        )
    } else {
        src_entry
    }
}

fn alloc_zeroed_table() -> Result<PhysAddr, AddrSpaceCloneError> {
    buddy::alloc_pages(0, crate::memory::AllocFlags::ZEROED)
        .map(|frame| frame.start_address())
        .map_err(|_| AddrSpaceCloneError::OutOfMemory)
}

fn repoint_table_entry(src_entry: PageTableEntry, new_phys: PhysAddr) -> PageTableEntry {
    const ENTRY_FLAG_MASK: u64 = !0x000F_FFFF_FFFF_F000u64;
    PageTableEntry::from_raw(new_phys.as_u64() | (src_entry.raw() & ENTRY_FLAG_MASK))
}

unsafe fn free_userspace_tables(root_pml4_phys: PhysAddr) {
    #[cfg(target_arch = "x86_64")]
    crate::memory::physical::allocator::buddy::diag25_tag_hex(
        b"<25TD pml4=",
        root_pml4_phys.as_u64(),
    );
    let pml4 = phys_to_table_ref(root_pml4_phys);
    for l4_idx in 0..256 {
        let entry = pml4[l4_idx];
        if entry.is_present() && entry.is_user() && !entry.is_huge() {
            free_pdpt_tree(entry.phys_addr());
        }
    }
    let _ = buddy::free_pages(Frame::containing(root_pml4_phys), 0);
}

unsafe fn free_pdpt_tree(pdpt_phys: PhysAddr) {
    let pdpt = phys_to_table_ref(pdpt_phys);
    for l3_idx in 0..512 {
        let entry = pdpt[l3_idx];
        if !entry.is_present() {
            continue;
        }
        if !entry.is_user() {
            continue;
        }
        if entry.is_huge() {
            release_huge_frame(entry, 18);
        } else {
            free_pd_tree(entry.phys_addr());
        }
    }
    let _ = buddy::free_pages(Frame::containing(pdpt_phys), 0);
}

unsafe fn free_pd_tree(pd_phys: PhysAddr) {
    let pd = phys_to_table_ref(pd_phys);
    for l2_idx in 0..512 {
        let entry = pd[l2_idx];
        if !entry.is_present() {
            continue;
        }
        if !entry.is_user() {
            continue;
        }
        if entry.is_huge() {
            release_huge_frame(entry, 9);
        } else {
            free_pt_tree(entry.phys_addr());
        }
    }
    let _ = buddy::free_pages(Frame::containing(pd_phys), 0);
}

unsafe fn free_pt_tree(pt_phys: PhysAddr) {
    let pt = phys_to_table_ref(pt_phys);
    for l1_idx in 0..512 {
        let entry = pt[l1_idx];
        if !entry.is_present() {
            continue;
        }
        if !entry.is_user() {
            continue;
        }
        release_leaf_frame(entry);
    }
    let _ = buddy::free_pages(Frame::containing(pt_phys), 0);
}

fn release_leaf_frame(entry: PageTableEntry) {
    let Some(frame) = entry.frame() else {
        return;
    };
    let remaining = COW_TRACKER.dec(frame);
    // `remaining == 0` est la dernière référence explicitement suivie : la
    // frame doit être libérée. `u32::MAX` signifie « non suivi » et n'est
    // libérable que pour une entrée non-CoW ; une entrée CoW non suivie est
    // volontairement conservée, car son partage ne peut pas être prouvé.
    let will_free = remaining == 0 || (remaining == u32::MAX && !entry.is_cow());
    // FIX-V9 (SEGV rip=0 run 10) — Garde UNIVERSELLE anti-vol de page vivante :
    // les gardes DIAG25_INITF ne couvrent que les 24 frames de pile capturées
    // au fork. Or le crash du run 10 désigne une page .text d'init (décodage
    // garbage de boot_services : rax=1, rcx=1, rbx=rbp=r8-r15=0 puis
    // call/jmp → rip=0), hors de toute surveillance. AVANT tout free_pages,
    // on vérifie la RÉALITÉ des tables de pages de PID 1 : si la frame y est
    // encore mappée et que le teardown n'est pas celui d'init lui-même, la
    // libérer = remettre une page VIVANTE en circulation (le prochain alloc
    // ZEROED/ELF la zéroée/l'écrase via physmap → corruption d'init). On
    // refuse (fuite volontaire d'une frame par boot, négligeable) et on
    // désigne le fautif. Couvre TOUTES les frames (.text/.rodata/.data/heap),
    // pas seulement DIAG25_INITF.
    #[cfg(target_arch = "x86_64")]
    if will_free {
        let fp_live = frame.phys_addr().as_u64();
        // SAFETY: lecture du TCB courant depuis le stockage per-CPU publié par
        // le scheduler pendant le teardown du thread courant.
        let tcb_raw = unsafe {
            crate::arch::x86_64::smp::percpu::try_read_current_tcb()
        }
        .unwrap_or(0);
        let cur_pid = if tcb_raw != 0 {
            // SAFETY: TCB courant publié par le scheduler ; le teardown
            // s'exécute dans le contexte syscall du processus mourant.
            unsafe {
                (*(tcb_raw as *const crate::scheduler::core::task::ThreadControlBlock))
                    .pid
                    .0
            }
        } else {
            0
        };
        if cur_pid != 1 && crate::syscall::table::diag_pid1_frame_mapped(fp_live) {
            use crate::arch::x86_64::terminal::debug_write;
            use crate::memory::physical::allocator::buddy::{diag25_dec, diag25_hex};
            debug_write(b"<V9FREE-LIVE f=");
            diag25_hex(fp_live);
            debug_write(b" rem=");
            diag25_dec(remaining as u64);
            debug_write(b" pid=");
            diag25_dec(cur_pid as u64);
            debug_write(b">\n");
            return; // REFUS : frame vivante d'init — fuir plutôt que corrompre.
        }
    }
    // #25 : signaler le cas anormal « une entrée CoW arrive à zéro pendant le
    // teardown ». La garde DIAG25_INITF ci-dessous reste l'autorité finale :
    // elle bloque toute libération d'une frame de pile init surveillée.
    #[cfg(target_arch = "x86_64")]
    if cow_teardown_underflow(entry, remaining) {
        crate::memory::physical::allocator::buddy::diag25_hex_always(
            b"<25TDCF f=",
            frame.phys_addr().as_u64(),
        );
        crate::memory::physical::allocator::buddy::diag25_hex_always(b" rem=", remaining as u64);
    }
    // FIX #25 (Audit 3.5) — Guard DIAG25_INITF avant buddy::free_pages.
    // Defense-in-depth runtime : si, malgré les fixes 2.1/2.2/3.3, un
    // déséquilibre de refcount rendait un frame init « éligible » au free,
    // ce guard l'empêche physiquement. On émet `<25GUARD-LEAK>` et on fuit
    // volontairement le frame (non-fatal, détectable dans les stats buddy).
    //
    // ⚠️ FIX-V6 (OOM post-Phoenix, bisect run 7) : la stratégie de fuite
    // volontaire drainait le pool buddy après quelques forks (chaque fork
    // ajoute ≥1 frame à DIAG25_INITF, jamais libérée → après 2-3 forks,
    // pool épuisé → #PF kernel : OOM → ExoPhoenix reboot).
    // NOUVEAU comportement : on LOG encore (détection) MAIS on unreserve le
    // frame puis on laisse le free_pages se faire — le pin (posé par
    // cow.rs `<25PIN>` pour le frame pile d'init / track_cow_frame pour les
    // frames partagées) reste l'autorité finale pendant la fenêtre du
    // nanosleep. Hors nanosleep, libérer le frame est LÉGITIME (le CoW a
    // été cassé, le frame n'est plus dans l'AS d'init).
    #[cfg(target_arch = "x86_64")]
    if will_free {
        use core::sync::atomic::Ordering;
        let fp = frame.phys_addr().as_u64();
        let mut j = 0usize;
        while j < 24 {
            let initf =
                crate::memory::physical::allocator::buddy::DIAG25_INITF[j].load(Ordering::Relaxed);
            if initf != 0 && initf == fp {
                use crate::arch::x86_64::terminal::debug_write;
                use crate::memory::physical::allocator::buddy::{diag25_dec, diag25_hex};
                // FIX-V8 (SEGV run 9, vol de page vivante) : ne libérer cette
                // frame QUE si elle n'est plus mappée dans l'AS de PID 1. Si
                // elle y est ENCORE mappée (page de pile VIVANTE d'init), la
                // libérer = vol de page : le prochain alloc la réattribue et
                // la zéro-page → init retourne en user avec une pile vidée
                // (run 9 : SEGV [base-0x82], rbx=rbp=0, stk=0×16). On garde
                // le pin (free_pages refusera via <25RSVD-REFUSE>) et la
                // watch, et on REFUSE la libération — la frame est allouée et
                // vivante, ce n'est pas une fuite.
                if crate::syscall::table::diag_pid1_frame_mapped(fp) {
                    debug_write(b"<25GUARD-LIVE f=");
                    diag25_hex(fp);
                    debug_write(b" rem=");
                    diag25_dec(remaining as u64);
                    debug_write(b">\n");
                    return;
                }
                debug_write(b"<25GUARD-FREE f=");
                diag25_hex(fp);
                debug_write(b" rem=");
                diag25_dec(remaining as u64);
                debug_write(b">\n");
                // FIX-V6 : clear PINNED+RESERVED avant de libérer, sinon
                // free_pages émet <25RSVD-REFUSE> et le frame reste leaké.
                crate::memory::physical::allocator::buddy::unreserve_frame(frame);
                // FIX-V7 (post-v6 stale DIAG25_INITF → faux positif REALLOCINIT) :
                // Le frame 0x5757000 (= DIAG25_INITF[1] après fork d'init) est
                // libéré LÉGITIMEMENT ici (le CoW a été cassé par init, puis
                // restauré in-place par l'enfant via cow.rs L42-56, le refcount
                // COW_TRACKER est tombstoné, will_free=true via la branche
                // `remaining==u32::MAX && !entry.is_cow()`). Sans ce clear,
                // DIAG25_INITF[j] reste pointé sur 0x5757000 même après que le
                // frame retourne au pool buddy. La prochaine alloc_pages qui
                // tombe sur ce frame déclenche le check diag25_on_alloc
                // (buddy.rs ~L1341) → panic REALLOCINIT (observé run 8/v6).
                //
                // On clear DIAG25_INITF[j] + DIAG25_WATCH_FRAME (si elle pointe
                // sur fp) puis on re-baseline le checksum (la frame libérée va
                // être modifiée par free list metadata / zero_pages, donc le
                // checksum actuel ne sera plus valide — sans re-baseline,
                // diag25_check_init émettrait des <25CORRUPT> faux positifs
                // sur les étapes postTD suivantes).
                crate::memory::physical::allocator::buddy::DIAG25_INITF[j]
                    .store(0, Ordering::Relaxed);
                let wf = crate::memory::physical::allocator::buddy::DIAG25_WATCH_FRAME
                    .load(Ordering::Relaxed);
                if wf == fp {
                    crate::memory::physical::allocator::buddy::DIAG25_WATCH_FRAME
                        .store(0, Ordering::Relaxed);
                }
                crate::memory::physical::allocator::buddy::diag25_init_baseline();
                debug_write(b"<25INITFCLEAR j=");
                diag25_dec(j as u64);
                debug_write(b">\n");
                break; // Sort du while j<24, tombe dans free_pages ci-dessous.
            }
            j += 1;
        }
    }
    if will_free {
        let _ = buddy::free_pages(frame, 0);
    }
}

/// Un teardown ne doit jamais faire tomber à zéro le compteur d'une feuille
/// encore marquée CoW. Cette condition pilote uniquement la sonde E9 ; la
/// décision de libération reste gouvernée par `will_free` ci-dessus.
#[inline]
fn cow_teardown_underflow(entry: PageTableEntry, remaining: u32) -> bool {
    remaining == 0 && entry.is_cow()
}

fn release_huge_frame(entry: PageTableEntry, order: usize) {
    let Some(frame) = entry.frame() else {
        return;
    };
    let remaining = COW_TRACKER.dec(frame);
    let will_free = remaining == 0 || (remaining == u32::MAX && !entry.is_cow());
    // FIX-V9 : garde universelle anti-vol de page vivante, identique à
    // release_leaf_frame — la page de BASE du huge block suffit à détecter
    // l'alias le plus courant (le reste de la plage reste couvert par la
    // boucle DIAG25_INITF ci-dessous, qui vérifie chaque entrée chevauchante
    // contre diag_pid1_frame_mapped).
    #[cfg(target_arch = "x86_64")]
    if will_free {
        let fp_live = frame.phys_addr().as_u64();
        // SAFETY: lecture du TCB courant depuis le stockage per-CPU publié par
        // le scheduler pendant le teardown du thread courant.
        let tcb_raw = unsafe {
            crate::arch::x86_64::smp::percpu::try_read_current_tcb()
        }
        .unwrap_or(0);
        let cur_pid = if tcb_raw != 0 {
            // SAFETY: TCB courant publié par le scheduler ; teardown du mourant.
            unsafe {
                (*(tcb_raw as *const crate::scheduler::core::task::ThreadControlBlock))
                    .pid
                    .0
            }
        } else {
            0
        };
        if cur_pid != 1 && crate::syscall::table::diag_pid1_frame_mapped(fp_live) {
            use crate::arch::x86_64::terminal::debug_write;
            use crate::memory::physical::allocator::buddy::{diag25_dec, diag25_hex};
            debug_write(b"<V9FREE-LIVE-HUGE f=");
            diag25_hex(fp_live);
            debug_write(b" ord=");
            diag25_dec(order as u64);
            debug_write(b" pid=");
            diag25_dec(cur_pid as u64);
            debug_write(b">\n");
            return; // REFUS : page de base vivante d'init dans ce huge block.
        }
    }
    if will_free {
        // FIX #25 (Audit 3.5) — Même guard DIAG25_INITF pour les huge frames.
        // FIX-V6 : idem que release_leaf_frame — on LOG+unreserve au lieu de
        // fuite permanente, sinon OOM après N forks.
        // FIX-V7 : clear DIAG25_INITF[j] + DIAG25_WATCH_FRAME + re-baseline,
        // sinon faux positif REALLOCINIT à la prochaine alloc du frame (cf.
        // release_leaf_frame commentaire FIX-V7). Pour les huge frames, on
        // ne break pas : on clear TOUTES les entrées DIAG25_INITF qui
        // chevauchent la plage du huge frame.
        #[cfg(target_arch = "x86_64")]
        {
            use core::sync::atomic::Ordering;
            let fp = frame.phys_addr().as_u64();
            let span = (4096usize << order) as u64;
            // FIX-V8 : si AU MOINS une entrée DIAG25_INITF chevauchant ce huge
            // block est encore mappée dans PID 1, libérer le block ENTIER
            // recyclerait une page VIVANTE d'init → on saute le free_pages
            // final (le/les entrées concernées restent pinnées + surveillées).
            let mut live_block = false;
            let mut j = 0usize;
            while j < 24 {
                let initf = crate::memory::physical::allocator::buddy::DIAG25_INITF[j]
                    .load(Ordering::Relaxed);
                if initf != 0 && initf >= fp && initf < fp + span {
                    use crate::arch::x86_64::terminal::debug_write;
                    use crate::memory::physical::allocator::buddy::{diag25_dec, diag25_hex};
                    // FIX-V8 : même garde que release_leaf_frame — une frame
                    // encore mappée dans PID 1 ne doit JAMAIS être libérée par
                    // un teardown enfant (vol de page vivante, cf. run 9).
                    if crate::syscall::table::diag_pid1_frame_mapped(initf) {
                        debug_write(b"<25GUARD-LIVE-HUGE f=");
                        diag25_hex(fp);
                        debug_write(b" ord=");
                        diag25_dec(order as u64);
                        debug_write(b">\n");
                        live_block = true;
                        j += 1;
                        continue;
                    }
                    debug_write(b"<25GUARD-FREE-HUGE f=");
                    diag25_hex(fp);
                    debug_write(b" ord=");
                    diag25_dec(order as u64);
                    debug_write(b" rem=");
                    diag25_dec(remaining as u64);
                    debug_write(b">\n");
                    crate::memory::physical::allocator::buddy::unreserve_frame(frame);
                    // FIX-V7 : clear DIAG25_INITF[j] + DIAG25_WATCH_FRAME + re-baseline.
                    crate::memory::physical::allocator::buddy::DIAG25_INITF[j]
                        .store(0, Ordering::Relaxed);
                    let wf = crate::memory::physical::allocator::buddy::DIAG25_WATCH_FRAME
                        .load(Ordering::Relaxed);
                    if wf >= fp && wf < fp + span {
                        crate::memory::physical::allocator::buddy::DIAG25_WATCH_FRAME
                            .store(0, Ordering::Relaxed);
                    }
                    crate::memory::physical::allocator::buddy::diag25_init_baseline();
                    debug_write(b"<25INITFCLEAR-HUGE j=");
                    diag25_dec(j as u64);
                    debug_write(b">\n");
                    // Pas de break : continuer pour clearer toute autre
                    // entrée DIAG25_INITF qui chevauche ce huge frame.
                }
                j += 1;
            }
            if live_block {
                return;
            }
        }
        let _ = buddy::free_pages(frame, order);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_entry_turns_writable_page_into_cow() {
        let frame = Frame::containing(PhysAddr::new(0x20_000));
        let entry = PageTableEntry::new(
            frame,
            PageTableEntry::FLAG_PRESENT
                | PageTableEntry::FLAG_WRITABLE
                | PageTableEntry::FLAG_USER,
        );

        let shared = shared_leaf_entry(entry);

        assert!(shared.is_present());
        assert!(shared.is_user());
        assert!(shared.is_cow());
        assert!(!shared.is_writable());
        assert_eq!(shared.phys_addr().as_u64(), entry.phys_addr().as_u64());
    }

    #[test]
    fn cow_teardown_underflow_reports_only_zero_count_cow_entries() {
        let frame = Frame::containing(PhysAddr::new(0x22_000));
        let cow = PageTableEntry::new(
            frame,
            PageTableEntry::FLAG_PRESENT | PageTableEntry::FLAG_USER | PageTableEntry::FLAG_COW,
        );
        let plain = PageTableEntry::new(
            frame,
            PageTableEntry::FLAG_PRESENT | PageTableEntry::FLAG_USER,
        );

        assert!(cow_teardown_underflow(cow, 0));
        assert!(!cow_teardown_underflow(cow, 1));
        assert!(!cow_teardown_underflow(plain, 0));
    }

    #[test]
    fn shared_entry_preserves_read_only_mapping_without_cow_flag() {
        // Une page RO reste RO : FLAG_COW autoriserait par erreur une écriture
        // via le handler de faute CoW après le fork.
        let frame = Frame::containing(PhysAddr::new(0x24_000));
        let entry = PageTableEntry::new(
            frame,
            PageTableEntry::FLAG_PRESENT | PageTableEntry::FLAG_USER,
        );

        let shared = shared_leaf_entry(entry);

        assert!(shared.is_present());
        assert!(shared.is_user());
        assert!(!shared.is_cow());
        assert!(!shared.is_writable());
        assert_eq!(shared.phys_addr().as_u64(), entry.phys_addr().as_u64());
    }

    #[test]
    fn shared_entry_preserves_existing_cow_mapping() {
        let frame = Frame::containing(PhysAddr::new(0x28_000));
        let entry = PageTableEntry::new(
            frame,
            PageTableEntry::FLAG_PRESENT | PageTableEntry::FLAG_USER | PageTableEntry::FLAG_COW,
        );

        let shared = shared_leaf_entry(entry);

        assert!(shared.is_present());
        assert!(shared.is_user());
        assert!(shared.is_cow());
        assert!(!shared.is_writable());
        assert_eq!(shared.phys_addr().as_u64(), entry.phys_addr().as_u64());
    }
}
