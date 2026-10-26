#![allow(unused_imports, dead_code)]
use super::*;
use super::frame::*;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::arch::x86_64::{apic, halt_cpu, idt, memory_iface, paging, read_cr2, read_cr3, smp};

pub fn sync_kpti_user_fault_mapping(_fault_addr: u64, source_cr3: u64) {
    if !crate::arch::x86_64::spectre::kpti::kpti_enabled() {
        return;
    }

    let tcb_raw = current_tcb_raw_checked();
    if tcb_raw == 0 {
        return;
    }
    // SAFETY: tcb_raw != 0 vérifié ; pointe vers le TCB du thread courant publié
    // par le scheduler pour ce CPU, vivant pendant le handler.
    let tcb = unsafe { &*(tcb_raw as *const crate::scheduler::core::task::ThreadControlBlock) };
    let mut user_cr3 = tcb.kpti_user_cr3();
    if user_cr3 == 0 {
        user_cr3 = crate::arch::x86_64::spectre::kpti::current_user_cr3();
    }
    if user_cr3 == 0 {
        let cpu_id = crate::arch::x86_64::cpu::topology::current_cpu_id().0 as usize;
        user_cr3 =
            crate::memory::virt::page_table::kpti_split::user_cr3_for_cpu(cpu_id).unwrap_or(0);
    }
    let kernel_cr3 = if source_cr3 != 0 {
        source_cr3
    } else {
        tcb.cr3_phys
    };
    const PML4_PHYS_MASK: u64 = 0x000F_FFFF_FFFF_F000;
    let k = kernel_cr3 & PML4_PHYS_MASK;
    let u = user_cr3 & PML4_PHYS_MASK;
    if k == 0 || u == 0 || k == u {
        return;
    }
    // SAFETY: k/u = PML4 valides (ring0).
    unsafe {
        crate::memory::virt::page_table::kpti_split::sync_user_region_to_shadow(
            crate::memory::core::PhysAddr::new(k),
            crate::memory::core::PhysAddr::new(u),
        );
    }
}


#[cfg(all(debug_assertions, exo_kernel_trace))]
pub fn debug_user_page_fault(
    frame: &ExceptionFrame,
    fault_addr_raw: u64,
    error_code: u64,
    user_vma_found: bool,
    user_as_for_fault: *const crate::memory::virt::UserAddressSpace,
) {
    if USER_PF_DEBUG_COUNT.fetch_add(1, Ordering::Relaxed) >= 16 {
        return;
    }

    let tcb_ptr = current_tcb_raw_checked();
    let mut pid = 0u64;
    let mut tcb_cr3 = 0u64;
    if tcb_ptr != 0 {
        // SAFETY: tcb_ptr != 0 vérifié ; pointe vers le TCB du thread courant
        // publié par le scheduler pour ce CPU (chemin debug uniquement).
        unsafe {
            let tcb = &*(tcb_ptr as *const crate::scheduler::core::task::ThreadControlBlock);
            pid = tcb.pid.0 as u64;
            tcb_cr3 = tcb.cr3_phys;
        }
    }

    let mut pcb_ptr = 0u64;
    let mut pcb_as = 0u64;
    let mut pcb_cr3 = 0u64;
    if pid != 0 {
        if let Some(pcb) = crate::process::core::registry::PROCESS_REGISTRY
            .find_by_pid(crate::process::core::pid::Pid(pid as u32))
        {
            pcb_ptr = pcb as *const _ as u64;
            pcb_as = pcb.address_space_ptr() as u64;
            pcb_cr3 = pcb.cr3.load(Ordering::Relaxed);
        }
    }

    let walk_cr3 = if !user_as_for_fault.is_null() {
        // SAFETY: pointeur non-null (vérifié), désigne l'UserAddressSpace du
        // processus en faute, vivant le temps du handler (chemin debug).
        let user_as = unsafe { &*user_as_for_fault };
        user_as.pml4_phys().as_u64()
    } else {
        tcb_cr3
    };

    let mut rip_pte = 0u64;
    let mut rsp_pte = 0u64;
    let mut cr2_pte = 0u64;
    let mut rip_frame = 0u64;
    let mut pcb_frame = 0u64;
    if walk_cr3 != 0 {
        use crate::memory::core::{PhysAddr, VirtAddr, PAGE_SIZE};
        use crate::memory::virt::page_table::PageTableWalker;

        let walker = PageTableWalker::new(PhysAddr::new(walk_cr3));
        rip_pte = walker.read_pte_raw(VirtAddr::new(frame.rip));
        rsp_pte = walker.read_pte_raw(VirtAddr::new(frame.rsp));
        cr2_pte = walker.read_pte_raw(VirtAddr::new(fault_addr_raw));
        rip_frame = rip_pte & 0x000F_FFFF_FFFF_F000;
        pcb_frame = if pcb_ptr >= crate::memory::core::layout::PHYS_MAP_BASE.as_u64() {
            (pcb_ptr - crate::memory::core::layout::PHYS_MAP_BASE.as_u64())
                & !(PAGE_SIZE as u64 - 1)
        } else {
            0
        };
    }

    let mut rip_bytes = 0u64;
    let rip_bytes_valid =
        frame.rip < crate::memory::core::layout::USER_END.as_u64() && (rip_pte & 1) != 0;
    if rip_bytes_valid {
        for i in 0..8u64 {
            // SAFETY: rip_bytes_valid garantit frame.rip < USER_END et PTE présente
            // (rip_pte & 1) ; lecture volatile d'octets user mappés (chemin debug).
            let byte = unsafe { core::ptr::read_volatile((frame.rip + i) as *const u8) };
            rip_bytes |= (byte as u64) << (i * 8);
        }
    }

    let out = crate::arch::x86_64::terminal::debug_write;
    out(b"pf: user pid=0x");
    debug_hex64(pid);
    out(b" tcb=0x");
    debug_hex64(tcb_ptr);
    out(b" tcb_cr3=0x");
    debug_hex64(tcb_cr3);
    out(b" cur_cr3=0x");
    debug_hex64(debug_read_cr3());
    out(b" pcb=0x");
    debug_hex64(pcb_ptr);
    out(b" pcb_as=0x");
    debug_hex64(pcb_as);
    out(b" pcb_cr3=0x");
    debug_hex64(pcb_cr3);
    out(b" rip=0x");
    debug_hex64(frame.rip);
    out(b" rsp=0x");
    debug_hex64(frame.rsp);
    out(b" cr2=0x");
    debug_hex64(fault_addr_raw);
    out(b" err=0x");
    debug_hex64(error_code);
    out(b" vma=");
    out(if user_vma_found { b"1" } else { b"0" });
    out(b" bytes=0x");
    if rip_bytes_valid {
        debug_hex64(rip_bytes);
    } else {
        out(b"unmapped");
    }

    if walk_cr3 != 0 {
        out(b" walk_cr3=0x");
        debug_hex64(walk_cr3);
        out(b" rip_pte=0x");
        debug_hex64(rip_pte);
        out(b" rsp_pte=0x");
        debug_hex64(rsp_pte);
        out(b" cr2_pte=0x");
        debug_hex64(cr2_pte);
        out(b" rip_frame=0x");
        debug_hex64(rip_frame);
        out(b" pcb_frame=0x");
        debug_hex64(pcb_frame);
    }
    out(b"\n");
}


/// Handler #PF — Page Fault
///
/// Dispatcher vers les handlers de la couche memory/ :
/// - CoW break              → memory::virt::fault::cow
/// - Demand paging          → memory::virt::fault::demand_paging
/// - Swap-in                → memory::virt::fault::swap_in
/// - Violation d'accès      → SIGSEGV (Ring 3) ou KernelFault (Ring 0)
///
/// ## Intégration memory/ (RÈGLE MEM-01 DOC2)
/// `memory::virt::fault::handler::handle_page_fault()` est le seul point
/// d'entrée pour tous les faults. Il prend un `FaultContext` construit ici
/// et un `FaultAllocator` (implémenté par `KernelFaultAllocator`).
///
/// ## Intégration process/ (RÈGLE DOC1)
/// Quand process/ sera intégré, l'allocateur utilisera l'espace d'adressage
/// du processus courant. Pour l'instant, `KernelFaultAllocator` est utilisé.
/// # Safety
/// `frame` doit être un pointeur valide vers une ExceptionFrame allouée par le stub ASM.
#[no_mangle]
pub unsafe extern "C" fn do_page_fault(frame: *mut ExceptionFrame) {
    // SAFETY: `frame` est un pointeur non-null passé par le stub ASM, aligné 16 B,
    // unique pour ce contexte d'exception.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[14].fetch_add(1, Ordering::Relaxed);
    crate::arch::x86_64::paging::inc_page_fault();

    // CR2 contient l'adresse virtuelle qui a causé le fault.
    let fault_addr_raw = crate::arch::x86_64::read_cr2();
    let error_code = frame.error_code;
    // Décomposition de l'error code x86_64 :
    // bit 0 = P  (page présente — protection violation)
    // bit 1 = W  (écriture)
    // bit 2 = U  (depuis Ring 3)
    // bit 3 = RSVD (bit réservé corrompu dans le PTE)
    // bit 4 = I  (instruction fetch)
    // bit 5 = PK (protection key violation)
    let is_present = error_code & 1 != 0;
    let is_write = error_code & 2 != 0;
    let _is_user = error_code & 4 != 0;
    let is_instr_fetch = error_code & 16 != 0;
    let _ = is_present; // Utilisé implicitement via FaultCause + FaultContext

    // Construire le FaultContext
    use crate::arch::x86_64::memory_iface::{UserFaultAllocator, KERNEL_FAULT_ALLOC};
    use crate::memory::core::VirtAddr;
    use crate::memory::virt::fault::{FaultCause, FaultContext, FaultResult};
    use crate::memory::virt::UserAddressSpace;

    let cause = if is_instr_fetch {
        FaultCause::Execute
    } else if is_write {
        FaultCause::Write
    } else {
        FaultCause::Read
    };

    let fault_addr = VirtAddr::new(fault_addr_raw);
    let from_kernel = frame.from_kernel();
    let mut ctx = FaultContext::new(fault_addr, cause, from_kernel).with_present(is_present);
    let mut user_as_for_fault: *const UserAddressSpace = core::ptr::null();
    let mut user_vma_found = false;
    if fault_addr.is_user() {
        let tcb_raw = current_tcb_raw_checked();
        if tcb_raw != 0 {
            // SAFETY: tcb_raw != 0 vérifié ; TCB du thread courant publié par le
            // scheduler pour ce CPU, vivant pendant le handler de #PF.
            let tcb =
                unsafe { &*(tcb_raw as *const crate::scheduler::core::task::ThreadControlBlock) };
            // FIX-CR3-FAULT-RESYNC : au bootstrap, une faute Ring 3 peut survenir
            // alors que le CR3 matériel ne correspond pas à l'espace d'adressage du
            // thread courant (`tcb.cr3_phys`, source de vérité maintenue par
            // create/execve). On recharge alors le bon CR3 : la résolution de faute
            // (demand-paging dans l'AS du thread) ET la reprise (iretq, KPTI off) se
            // font dès lors dans la bonne table → fin de la boucle de #PF d'init.
            if frame.from_userspace() && tcb.cr3_phys != 0 {
                const CR3_MASK: u64 = 0x000F_FFFF_FFFF_F000;
                let hw_cr3: u64;
                // SAFETY: lecture CR3, ring0.
                unsafe {
                    core::arch::asm!("mov {}, cr3", out(reg) hw_cr3, options(nostack, nomem));
                }
                if hw_cr3 & CR3_MASK != tcb.cr3_phys & CR3_MASK {
                    // SAFETY: tcb.cr3_phys est une PML4 valide du thread courant ;
                    // les pages noyau (code/pile/handler) sont mappées dans toutes
                    // les AS, le switch est donc sûr pendant le handler.
                    unsafe {
                        core::arch::asm!("mov cr3, {}", in(reg) tcb.cr3_phys & CR3_MASK, options(nostack, nomem));
                    }
                }
            }
            if let Some(pcb) = crate::process::core::registry::PROCESS_REGISTRY
                .find_by_pid(crate::process::core::pid::Pid(tcb.pid.0))
            {
                let as_ptr = pcb.address_space_ptr();
                if !as_ptr.is_null() {
                    // SAFETY: as_ptr non-null (vérifié), publié par lifecycle/create
                    // comme UserAddressSpace du PCB trouvé ; le PCB est vivant dans
                    // PROCESS_REGISTRY pendant le handler.
                    let user_as = unsafe { &*(as_ptr as *const UserAddressSpace) };
                    user_as_for_fault = user_as as *const UserAddressSpace;
                    user_as.record_fault();
                    if let Some(vma) = user_as.find_vma(fault_addr) {
                        ctx = ctx.with_vma(vma);
                        user_vma_found = true;
                    }
                }
            }
        }
    }
    #[cfg(all(debug_assertions, exo_kernel_trace))]
    if frame.from_userspace() {
        debug_user_page_fault(
            frame,
            fault_addr_raw,
            error_code,
            user_vma_found,
            user_as_for_fault,
        );
    }

    // Dispatcher vers le sous-système memory/
    //
    // Une faute sur une adresse userspace peut arriver avec CS=kernel pendant
    // un copy_from_user/copy_to_user ou une lecture d'argv. Elle reste une faute
    // de l'espace utilisateur courant: si une VMA existe, on la résout via le
    // même chemin demand-paging/CoW qu'une faute Ring 3, puis on reprend
    // l'instruction noyau qui accédait au pointeur user.
    let resolve_as_user = fault_addr.is_user()
        && !user_as_for_fault.is_null()
        && (frame.from_userspace() || user_vma_found);
    if resolve_as_user {
        ctx.from_kernel = false;
    }

    let result = if resolve_as_user {
        // SAFETY: resolve_as_user implique user_as_for_fault non-null ; pointe vers
        // l'UserAddressSpace du processus en faute, vivant pendant le handler.
        let user_as = unsafe { &*user_as_for_fault };
        let user_alloc = UserFaultAllocator::new(user_as);
        crate::memory::virt::fault::handler::handle_page_fault(&ctx, &user_alloc)
    } else {
        crate::memory::virt::fault::handler::handle_page_fault(&ctx, &KERNEL_FAULT_ALLOC)
    };
    match result {
        FaultResult::Handled => {
            if resolve_as_user {
                let source_cr3 = if !user_as_for_fault.is_null() {
                    // SAFETY: pointeur non-null (vérifié) ; UserAddressSpace du
                    // processus en faute, vivant pendant le handler.
                    unsafe { (&*user_as_for_fault).pml4_phys().as_u64() }
                } else {
                    0
                };
                sync_kpti_user_fault_mapping(fault_addr_raw, source_cr3);
            }
            // Fault résolu (demand paging, CoW, swap-in) — reprendre l'exécution.
            if frame.from_userspace() {
                exception_return_to_user_after_recovered_fault(frame);
            }
            // En mode kernel : retour direct via IRETQ (stub ASM)
        }
        FaultResult::Segfault { addr } => {
            // Violation d'accès mémoire.
            let _ = addr;
            if frame.from_userspace() {
                // Notify ExoShield NGAV of the memory access violation.
                let tcb_seg = current_tcb_raw_checked();
                let seg_pid = if tcb_seg != 0 {
                    // SAFETY: tcb_seg != 0 checked; pointer published by scheduler for this CPU.
                    unsafe {
                        (*(tcb_seg as *const crate::scheduler::core::task::ThreadControlBlock))
                            .pid
                            .0
                    }
                } else {
                    0u32
                };
                // DIAG-25 : tracer le segfault userspace (PID + CR2 + RIP) pour
                // confirmer/localiser le crash post-fork d'ipc_router (#25).
                {
                    let out = crate::arch::x86_64::terminal::debug_write;
                    out(b"<SEGV pid=");
                    let mut v = seg_pid as u64;
                    if v == 0 {
                        out(b"0");
                    } else {
                        let mut d = [0u8; 20];
                        let mut i = d.len();
                        while v != 0 && i > 0 {
                            i -= 1;
                            d[i] = b'0' + (v % 10) as u8;
                            v /= 10;
                        }
                        out(&d[i..]);
                    }
                    let hex = |val: u64| {
                        let lut = b"0123456789abcdef";
                        let mut hb = [0u8; 16];
                        let mut x = val;
                        let mut k = 16;
                        while k > 0 {
                            k -= 1;
                            hb[k] = lut[(x & 0xf) as usize];
                            x >>= 4;
                        }
                        hb
                    };
                    out(b" cr2=");
                    out(&hex(fault_addr_raw));
                    out(b" rip=");
                    out(&hex(frame.rip));
                    out(b" rsp=");
                    out(&hex(frame.rsp));
                    out(b" err=");
                    out(&hex(error_code));
                    // DIAG-25 : octets in-memory à RIP (la page RIP est présente —
                    // l'instruction a été fetchée avant la faute data). Comparer au
                    // binaire statique → détecte une corruption du .text d'init.
                    out(b" ripb=");
                    let mut rb = 0u64;
                    // SAFETY: lecture GARDÉE — uniquement si RIP est une adresse user
                    // plausible. Sinon lire (p.ex. RIP=0x130) re-fault en KERNEL et
                    // déclenche une résurrection ExoPhoenix (perte du dump #25).
                    if (0x1000..0x0000_8000_0000_0000).contains(&frame.rip) {
                        for bi in 0..8u64 {
                            let byte =
                                unsafe { core::ptr::read_volatile((frame.rip + bi) as *const u8) };
                            rb |= (byte as u64) << (bi * 8);
                        }
                    }
                    out(&hex(rb));
                    // DIAG-25 : contexte GP complet — révèle si les registres d'init
                    // sont corrompus (zéro / stale / décalés) au resume post-fork.
                    out(b" rax=");
                    out(&hex(frame.rax));
                    out(b" rbx=");
                    out(&hex(frame.rbx));
                    out(b" rcx=");
                    out(&hex(frame.rcx));
                    out(b" rdx=");
                    out(&hex(frame.rdx));
                    out(b" rsi=");
                    out(&hex(frame.rsi));
                    out(b" rdi=");
                    out(&hex(frame.rdi));
                    out(b" rbp=");
                    out(&hex(frame.rbp));
                    // DIAG-V8 (SEGV run 9) : r8-r15 absents du dump — le
                    // registre BASE de l'instruction fautive est probablement
                    // parmi eux (ex: mov rax,[rbx-0x82] avec rbx rechargé à 0
                    // depuis une page de pile volée/vidée).
                    out(b" r8=");
                    out(&hex(frame.r8));
                    out(b" r9=");
                    out(&hex(frame.r9));
                    out(b" r10=");
                    out(&hex(frame.r10));
                    out(b" r11=");
                    out(&hex(frame.r11));
                    out(b" r12=");
                    out(&hex(frame.r12));
                    out(b" r13=");
                    out(&hex(frame.r13));
                    out(b" r14=");
                    out(&hex(frame.r14));
                    out(b" r15=");
                    out(&hex(frame.r15));
                    out(b" cs=");
                    out(&hex(frame.cs));
                    out(b" ss=");
                    out(&hex(frame.ss));
                    out(b" rfl=");
                    out(&hex(frame.rflags));
                    // DIAG-V9 (SEGV run 10 — console lossy) : le dump du run 10 a
                    // PERDU sa queue (mots 8-15 de stk + ret0= + cs16=) car la
                    // console E9 boude en fin de ligne longue. ret0 est LA donnée
                    // décisive (adresse de retour du call/jmp vers NULL, directement
                    // addr2line-able) → on l'imprime DÈS MAINTENANT, avant les 16
                    // mots de pile, pour qu'elle survive à toute troncature.
                    if frame.rip == 0 && (0x1000..0x0000_8000_0000_0000).contains(&frame.rsp) {
                        // SAFETY: RSP user plausible ; lecture volatile.
                        let ret0 = unsafe {
                            core::ptr::read_volatile(frame.rsp as *const u64)
                        };
                        out(b" ret0=");
                        out(&hex(ret0));
                        if (0x1000_0000_0000..0x1001_0000_0000).contains(&ret0) {
                            out(b" cs16=");
                            let base = ret0.saturating_sub(16);
                            for bi in 0..16u64 {
                                // SAFETY: ret0 dans la plage binaire user (mappée,
                                // exécutée juste avant le call).
                                let byte = unsafe {
                                    core::ptr::read_volatile((base + bi) as *const u8)
                                };
                                let lut = b"0123456789abcdef";
                                out(&[lut[(byte >> 4) as usize], lut[(byte & 0xf) as usize]]);
                            }
                        }
                    }
                    // DIAG-V9 : rdx du run 10 = 0x1000001ce5b (adresse .text) —
                    // si rdx pointe dans le binaire, dumper 8 octets à rdx : le
                    // décodage de ces octets identifie si le flux d'instructions
                    // était déjà corrompu (garbage) au moment du saut vers 0.
                    if (0x1000_0000_0000..0x1001_0000_0000).contains(&frame.rdx) {
                        out(b" rdx8=");
                        for bi in 0..8u64 {
                            // SAFETY: rdx dans la plage binaire user (mappée).
                            let byte = unsafe {
                                core::ptr::read_volatile((frame.rdx + bi) as *const u8)
                            };
                            let lut = b"0123456789abcdef";
                            out(&[lut[(byte >> 4) as usize], lut[(byte & 0xf) as usize]]);
                        }
                    }
                    // DIAG-25 : 8 mots de la pile user à RSP — révèle la chaîne de
                    // retour (d'où provient le saut vers le RIP corrompu).
                    out(b" stk=");
                    if (0x1000..0x0000_8000_0000_0000).contains(&frame.rsp) {
                        for wi in 0..16u64 {
                            // SAFETY: RSP user plausible ; KPTI off ; lecture volatile.
                            let w = unsafe {
                                core::ptr::read_volatile((frame.rsp + wi * 8) as *const u64)
                            };
                            out(&hex(w));
                            out(b" ");
                        }
                    }
                    // (DIAG-NULLCALL bisect run 6 — ret0/cs16 déplacés EN TÊTE du
                    // dump par FIX-V9 ci-dessus pour survivre à la troncature de la
                    // console lossy ; l'ancien bloc tail est retiré.)
                    out(b">\n");
                }
                crate::security::shield_feed::push_event(
                    seg_pid,
                    crate::security::shield_feed::event_type::MEMORY,
                    crate::security::shield_feed::severity::CRITICAL,
                    0xDE, // pseudo-opcode: SIGSEGV
                    fault_addr_raw,
                    0,
                );
                queue_signal_for_current(crate::process::signal::Signal::SIGSEGV);
                exception_return_to_user(frame);
            } else {
                kernel_panic_exception("#PF kernel : accès invalide", frame);
            }
        }
        FaultResult::Oom { addr } => {
            if frame.from_userspace() {
                // Out of memory — OOM killer notifié côté userspace.
                let _ = addr;
                queue_signal_for_current(crate::process::signal::Signal::SIGKILL);
                exception_return_to_user(frame);
            } else {
                kernel_panic_exception("#PF kernel : OOM", frame);
            }
        }
        FaultResult::KernelFault { addr } => {
            // Fault kernel — consulter la table de fixup P1-03.
            // Si copy_from/to_user_safe est en cours sur ce CPU, signaler
            // le fault plutôt que paniquer (la copie retournera EFAULT).
            let _ = addr;
            let cpu_id = crate::arch::x86_64::cpu::topology::current_cpu_id().0 as usize;
            if crate::syscall::fixup::fixup_lookup(cpu_id).is_some() {
                crate::syscall::fixup::fixup_signal_fault(cpu_id);
                // Retour normal — la boucle de copie verra FAULTED=true
                // et retournera false → EFAULT côté syscall.
            } else {
                kernel_panic_exception("#PF kernel : non récupérable", frame);
            }
        }
    }
}

