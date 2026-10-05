// kernel/src/memory/virtual/fault/cow.rs
//
// CoW fault handler — gère un write sur une page marquée Copy-on-Write.
// Couche 0 — aucune dépendance externe sauf `spin`.

use super::handler::FaultAllocator;
use super::{FaultContext, FaultResult};
use crate::memory::core::{PageFlags, VirtAddr, PAGE_SIZE};
use crate::memory::cow::tracker::COW_TRACKER;
use crate::memory::virt::address_space::tlb::flush_single;
use crate::memory::virt::page_table::PageTableEntry;
use crate::memory::virt::vma::VmaDescriptor;

/// Traite un CoW fault (write sur page en lecture seule avec flag COW).
pub fn handle_cow_fault<A: FaultAllocator>(
    ctx: &FaultContext,
    vma: &VmaDescriptor,
    alloc: &A,
) -> FaultResult {
    let page_addr = VirtAddr::new(ctx.fault_addr.as_u64() & !(PAGE_SIZE as u64 - 1));

    let old_raw = alloc.read_pte_raw(page_addr);
    let old_entry = PageTableEntry::from_raw(old_raw);
    let old_frame = match old_entry.frame() {
        Some(frame) => frame,
        None => match alloc.translate(page_addr) {
            Some(phys) => crate::memory::core::Frame::containing(phys),
            None => {
                // Page pas encore mappée + flag COW → demand paging d'abord.
                return super::demand_paging::handle_demand_paging(ctx, vma, alloc);
            }
        },
    };

    let writable_flags = old_entry
        .to_page_flags()
        .clear(PageFlags::COW)
        .set(PageFlags::WRITABLE)
        .set(PageFlags::PRESENT);

    let tracked_ref_count = COW_TRACKER.tracked_ref_count(old_frame);
    let mut can_restore_in_place =
        tracked_ref_count.is_some_and(|rc| rc <= 1) || !old_entry.is_cow();

    // FIX-V9 (SEGV rip=0 runs 4→10, écriture aliène cross-processus) : le
    // refcount du COW_TRACKER peut MENTIR (tombstone posé par un double-dec,
    // entrée perdue, compte décrémenté par un chemin parallèle). Restaurer
    // « in place » sur la foi d'un rc<=1 alors que la frame est ENCORE mappée
    // dans l'AS de PID 1 rend la PTE du fautif WRITABLE sur une page VIVANTE
    // d'init : chaque écriture user du fautif atterrit physiquement dans init
    // (run 4 : pile d'init réécrite 1139 octets pendant le nanosleep ; run 10 :
    // .text d'init décodé en garbage → call/jmp vers 0 → SEGV rip=0, rax=1,
    // rcx=1, rbx=rbp=r8-r15=0). Réalité > refcount : on vérifie la PTE de
    // PID 1 AVANT de restaurer in place ; si la frame y est encore mappée et
    // que le fautif n'est PAS PID 1 lui-même → on FORCE le chemin copie
    // (frame privée neuve) — toujours sûr, jamais une corruption.
    #[cfg(target_arch = "x86_64")]
    if can_restore_in_place {
        let old_phys = old_frame.start_address().as_u64();
        // SAFETY: lecture du TCB courant depuis le stockage per-CPU publié par
        // le scheduler pendant le traitement de la faute.
        let tcb_raw = unsafe {
            crate::arch::x86_64::smp::percpu::try_read_current_tcb()
        }
        .unwrap_or(0);
        let cur_pid = if tcb_raw != 0 {
            // SAFETY: TCB courant publié par le scheduler, vivant pendant le
            // traitement de la faute sur ce CPU.
            unsafe {
                (*(tcb_raw as *const crate::scheduler::core::task::ThreadControlBlock))
                    .pid
                    .0
            }
        } else {
            0
        };
        if cur_pid != 1 && crate::syscall::table::diag_pid1_frame_mapped(old_phys) {
            can_restore_in_place = false;
            use crate::arch::x86_64::terminal::debug_write;
            use crate::memory::physical::allocator::buddy::{diag25_dec, diag25_hex};
            debug_write(b"<V9COW-COPY f=");
            diag25_hex(old_phys);
            debug_write(b" pid=");
            diag25_dec(cur_pid as u64);
            debug_write(b">\n");
        }
    }

    if can_restore_in_place {
        let new_raw = PageTableEntry::from_page_flags(old_frame, writable_flags).raw();
        match alloc.compare_exchange_pte_raw(page_addr, old_raw, new_raw) {
            Ok(_) => {
                if tracked_ref_count.is_some() {
                    let _ = COW_TRACKER.dec(old_frame);
                }
                vma.record_cow_break();
                // SAFETY: adresse canonique.
                unsafe {
                    flush_single(page_addr);
                }
                return FaultResult::Handled;
            }
            Err(actual_raw) => {
                let actual = PageTableEntry::from_raw(actual_raw);
                if actual.is_present() && !actual.is_cow() {
                    // Un autre CPU a déjà restauré l'écriture en place.
                    unsafe {
                        flush_single(page_addr);
                    }
                    return FaultResult::Handled;
                }
            }
        }
    }

    // La page est encore partagée : copier vers un nouveau frame puis publier
    // le PTE par CAS pour sérialiser les fautes concurrentes.
    let new_frame = match alloc.alloc_nonzeroed() {
        Ok(f) => f,
        Err(_) => {
            return FaultResult::Oom {
                addr: ctx.fault_addr,
            }
        }
    };

    // Copier les données de l'ancien frame vers le nouveau.
    // SAFETY: Les deux frames sont mappés dans le physmap kernel.
    //
    // FIX #25 (Audit 5.3) — Diagnostic défensif : vérifier que new_frame n'est
    // PAS une frame de pile d'init vivante. Si new_frame (alloué via
    // alloc_nonzeroed) a été corrompu, ou si le buddy a retourné une frame
    // vivante d'init, cette copie écraserait le contenu de la page d'init. On
    // panic pour capturer le coupable.
    #[cfg(target_arch = "x86_64")]
    {
        use core::sync::atomic::Ordering;
        let dst_phys = new_frame.start_address().as_u64();
        let mut i = 0usize;
        while i < 24 {
            let initf =
                crate::memory::physical::allocator::buddy::DIAG25_INITF[i].load(Ordering::Relaxed);
            if initf != 0 && initf == dst_phys {
                panic!(
                    "#25 CAUGHT handle_cow_fault: new_frame={:#x} == DIAG25_INITF[{}]",
                    dst_phys, i
                );
            }
            i += 1;
        }
    }
    unsafe {
        let src = (crate::memory::core::layout::PHYS_MAP_BASE.as_u64()
            + old_frame.start_address().as_u64()) as *const u8;
        let dst = (crate::memory::core::layout::PHYS_MAP_BASE.as_u64()
            + new_frame.start_address().as_u64()) as *mut u8;
        // SAFETY: src et dst sont des frames physiques distincts, taille PAGE_SIZE.
        core::ptr::copy_nonoverlapping(src, dst, PAGE_SIZE);
    }

    // Remap la page avec les nouveaux flags (writable, supprimer COW).
    let new_raw = PageTableEntry::from_page_flags(new_frame, writable_flags).raw();
    match alloc.compare_exchange_pte_raw(page_addr, old_raw, new_raw) {
        Ok(_) => {
            let remaining = COW_TRACKER.dec(old_frame);
            if remaining == 0 {
                alloc.free_frame(old_frame);
            }
            vma.record_cow_break();
            // SAFETY: adresse canonique.
            unsafe {
                flush_single(page_addr);
            }
            // DIAG #25 (Bochs watchpoint) : émet le frame physique (F') des pages
            // de pile USER cassées-CoW, pour poser un watchpoint physique Bochs sur
            // F'+0xae8 (slot return-address corrompu). Plage pile user uniquement.
            #[cfg(target_arch = "x86_64")]
            if page_addr.as_u64() >= 0x7fff_0000_0000 {
                use crate::arch::x86_64::terminal::debug_write;
                debug_write(b"<F25 p=");
                diag_f25_hex(page_addr.as_u64());
                debug_write(b" f=");
                diag_f25_hex(new_frame.start_address().as_u64());
                debug_write(b" cr3=");
                // SAFETY: lecture du registre CR3 — aucun accès mémoire (nomem correct,
                // contrairement à gs:[..]). Permet de distinguer la cassure CoW d'init
                // (cr3 = AS d'init) de celle de l'enfant pour cibler le watchpoint #25.
                let cr3v: u64;
                unsafe {
                    core::arch::asm!("mov {}, cr3", out(reg) cr3v, options(nomem, nostack));
                }
                diag_f25_hex(cr3v);
                debug_write(b">");
                // FIX-#25-PIN (boot-hang bisect) : épingler la NOUVELLE frame de la
                // pile d'init (PID 1). C'est exactement la frame F dont le CKP a
                // prouvé la réécriture pendant le nanosleep (mod=1139). L'épingler
                // garantit que même une libération erronée (buddy déséquilibré par
                // un chemin inconnu) sera REFUSÉE par free_pages (<25RSVD-REFUSE>)
                // au lieu de remettre F en circulation pour les allocations
                // d'ipc_router (pile/demand-paging) — ce qui était le mécanisme de
                // l'aliasing observé.
                // SAFETY: lecture du TCB courant depuis le stockage per-CPU
                // publié par le scheduler pendant le traitement de la faute.
                let tcb_raw = unsafe {
                    crate::arch::x86_64::smp::percpu::try_read_current_tcb()
                }
                .unwrap_or(0);
                if tcb_raw != 0 {
                    // SAFETY: TCB courant publié par le scheduler, vivant pendant
                    // le traitement du fault sur ce CPU.
                    let faulting_pid = unsafe {
                        (*(tcb_raw as *const crate::scheduler::core::task::ThreadControlBlock))
                            .pid
                            .0
                    };
                    if faulting_pid == 1 {
                        crate::memory::physical::allocator::buddy::reserve_frame(new_frame);
                        // FIX-V8 (limitation v7 : accumulation des pins 25PIN)
                        // : enregistrer la nouvelle frame dans un slot libre
                        // de DIAG25_INITF pour qu'elle soit (a) SURVEILLÉE par
                        // les détecteurs 25CORRUPT/25FREEINIT/WR-CKP et (b)
                        // LIBÉRABLE au NS2 suivant si elle n'est plus mappée
                        // dans l'AS d'init (remplacée par un CoW break plus
                        // récent). Sans cet enregistrement, le pin posé ici
                        // n'est jamais suivi par le NS2-UNP → accumulation à
                        // chaque fork → dérive OOM (scénario (b) documenté
                        // dans ANALYSE_V7).
                        {
                            use core::sync::atomic::Ordering;
                            let np = new_frame.start_address().as_u64();
                            let mut k = 0usize;
                            while k < 24 {
                                let cur =
                                    crate::memory::physical::allocator::buddy::DIAG25_INITF[k]
                                        .load(Ordering::Relaxed);
                                if cur == np {
                                    break; // déjà enregistrée
                                }
                                if cur == 0 {
                                    crate::memory::physical::allocator::buddy::DIAG25_INITF[k]
                                        .store(np, Ordering::Relaxed);
                                    // Re-baseline : le checksum couvre désormais cette
                                    // frame (pile vivante d'init — son contenu évolue
                                    // normalement entre les jalons execve enfant).
                                    crate::memory::physical::allocator::buddy::diag25_init_baseline();
                                    break;
                                }
                                k += 1;
                            }
                        }
                        debug_write(b"<25PIN f=");
                        diag_f25_hex(new_frame.start_address().as_u64());
                        debug_write(b">\n");
                    }
                }
            }
            FaultResult::Handled
        }
        Err(actual_raw) => {
            alloc.free_frame(new_frame);
            let actual = PageTableEntry::from_raw(actual_raw);
            if actual.is_present() {
                // Un autre CPU a probablement gagné la course et a déjà cassé le CoW.
                unsafe {
                    flush_single(page_addr);
                }
                FaultResult::Handled
            } else {
                super::demand_paging::handle_demand_paging(ctx, vma, alloc)
            }
        }
    }
}

/// DIAG #25 : émission hex 16 digits sur le port debug E9 (Bochs/QEMU).
#[cfg(target_arch = "x86_64")]
fn diag_f25_hex(mut v: u64) {
    use crate::arch::x86_64::terminal::debug_write;
    let mut buf = [0u8; 16];
    let mut i = 16usize;
    while i > 0 {
        i -= 1;
        let nib = (v & 0xf) as u8;
        buf[i] = if nib < 10 {
            b'0' + nib
        } else {
            b'a' + nib - 10
        };
        v >>= 4;
    }
    debug_write(&buf);
}
