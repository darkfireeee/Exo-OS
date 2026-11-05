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
    // SAFETY: tcb_raw != 0 verified; points to the current thread's TCB published
    // by the scheduler for this CPU, alive during the handler.
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
    // SAFETY: k/u = valid PML4s (ring0).
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
        // SAFETY: tcb_ptr != 0 verified; points to the current thread's TCB
        // published by the scheduler for this CPU (debug path only).
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
        // SAFETY: non-null pointer (verified), designates the UserAddressSpace of
        // the faulting process, alive for the duration of the handler (debug path).
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
            // SAFETY: rip_bytes_valid guarantees frame.rip < USER_END and PTE present
            // (rip_pte & 1); volatile read of mapped user bytes (debug path).
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


/// #PF Handler — Page Fault
///
/// Dispatcher to memory/ subsystem handlers:
/// - CoW break              → memory::virt::fault::cow
/// - Demand paging          → memory::virt::fault::demand_paging
/// - Swap-in                → memory::virt::fault::swap_in
/// - Access violation       → SIGSEGV (Ring 3) or KernelFault (Ring 0)
///
/// ## Memory/ integration (RULE MEM-01 DOC2)
/// `memory::virt::fault::handler::handle_page_fault()` is the single entry
/// point for all faults. It takes a `FaultContext` constructed here
/// and a `FaultAllocator` (implemented by `KernelFaultAllocator`).
///
/// ## Process/ integration (RULE DOC1)
/// When process/ is integrated, the allocator will use the address space
/// of the current process. Currently, `KernelFaultAllocator` is used.
/// # Safety
/// `frame` must be a valid pointer to an ExceptionFrame allocated by the ASM stub.
#[no_mangle]
pub unsafe extern "C" fn do_page_fault(frame: *mut ExceptionFrame) {
    // SAFETY: `frame` is a non-null pointer passed by the ASM stub, 16-byte aligned,
    // unique for this exception context.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[14].fetch_add(1, Ordering::Relaxed);
    crate::arch::x86_64::paging::inc_page_fault();

    // CR2 contains the virtual address that caused the fault.
    let fault_addr_raw = crate::arch::x86_64::read_cr2();
    let error_code = frame.error_code;
    // Decomposition of the x86_64 error code:
    // bit 0 = P  (page present — protection violation)
    // bit 1 = W  (write)
    // bit 2 = U  (from Ring 3)
    // bit 3 = RSVD (reserved bit corrupted in the PTE)
    // bit 4 = I  (instruction fetch)
    // bit 5 = PK (protection key violation)
    let is_present = error_code & 1 != 0;
    let is_write = error_code & 2 != 0;
    let _is_user = error_code & 4 != 0;
    let is_instr_fetch = error_code & 16 != 0;
    let _ = is_present; // Used implicitly via FaultCause + FaultContext

    // Build FaultContext
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
            // SAFETY: tcb_raw != 0 verified; current thread's TCB published by the
            // scheduler for this CPU, alive during the #PF handler.
            let tcb =
                unsafe { &*(tcb_raw as *const crate::scheduler::core::task::ThreadControlBlock) };
            // FIX-CR3-FAULT-RESYNC: at bootstrap, a Ring 3 fault can occur
            // while the hardware CR3 does not correspond to the address space of the
            // current thread (`tcb.cr3_phys`, source of truth maintained by
            // create/execve). We then reload the proper CR3: fault resolution
            // (demand-paging in the thread's AS) AND resumption (iretq, KPTI off)
            // will then happen in the right table → terminates the init #PF loop.
            if frame.from_userspace() && tcb.cr3_phys != 0 {
                const CR3_MASK: u64 = 0x000F_FFFF_FFFF_F000;
                let hw_cr3: u64;
                // SAFETY: read CR3, ring0.
                unsafe {
                    core::arch::asm!("mov {}, cr3", out(reg) hw_cr3, options(nostack, nomem));
                }
                if hw_cr3 & CR3_MASK != tcb.cr3_phys & CR3_MASK {
                    // SAFETY: tcb.cr3_phys is a valid PML4 of the current thread;
                    // kernel pages (code/stack/handler) are mapped in all
                    // ASes, so the switch is safe during the handler.
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
                    // SAFETY: as_ptr non-null (verified), published by lifecycle/create
                    // as UserAddressSpace of the found PCB; the PCB is alive in
                    // PROCESS_REGISTRY during the handler.
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

    // Dispatch to the memory/ subsystem
    //
    // A fault on a userspace address can occur with CS=kernel during
    // a copy_from_user/copy_to_user or an argv read. It remains a fault
    // of the current user address space: if a VMA exists, we resolve it via
    // the same demand-paging/CoW path as a Ring 3 fault, then resume
    // the kernel instruction that accessed the user pointer.
    let resolve_as_user = fault_addr.is_user()
        && !user_as_for_fault.is_null()
        && (frame.from_userspace() || user_vma_found);
    if resolve_as_user {
        ctx.from_kernel = false;
    }

    let result = if resolve_as_user {
        // SAFETY: resolve_as_user implies non-null user_as_for_fault; points to
        // the UserAddressSpace of the faulting process, alive during the handler.
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
                    // SAFETY: non-null pointer (verified); UserAddressSpace of the
                    // faulting process, alive during the handler.
                    unsafe { (&*user_as_for_fault).pml4_phys().as_u64() }
                } else {
                    0
                };
                sync_kpti_user_fault_mapping(fault_addr_raw, source_cr3);
            }
            // Fault resolved (demand paging, CoW, swap-in) — resume execution.
            if frame.from_userspace() {
                exception_return_to_user_after_recovered_fault(frame);
            }
            // In kernel mode: direct return via IRETQ (ASM stub)
        }
        FaultResult::Segfault { addr } => {
            // Memory access violation.
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
                // DIAG-25: trace userspace segfault (PID + CR2 + RIP) to
                // confirm/locate post-fork crash in ipc_router (#25).
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
                    // DIAG-25: in-memory bytes at RIP (RIP page is present —
                    // instruction was fetched before the data fault). Compare to
                    // static binary → detects corruption of init .text.
                    out(b" ripb=");
                    let mut rb = 0u64;
                    // SAFETY: GUARDED read — only if RIP is a plausible user
                    // address. Otherwise reading (e.g. RIP=0x130) re-faults in KERNEL
                    // and triggers an ExoPhoenix resurrection (loss of dump #25).
                    if (0x1000..0x0000_8000_0000_0000).contains(&frame.rip) {
                        for bi in 0..8u64 {
                            let byte =
                                unsafe { core::ptr::read_volatile((frame.rip + bi) as *const u8) };
                            rb |= (byte as u64) << (bi * 8);
                        }
                    }
                    out(&hex(rb));
                    // DIAG-25: full GP context — reveals if init registers
                    // are corrupted (zero / stale / shifted) at post-fork resume.
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
                    // DIAG-V8 (SEGV run 9): r8-r15 absent from dump — the
                    // BASE register of the faulting instruction is probably
                    // among them (e.g.: mov rax,[rbx-0x82] with rbx reloaded to 0
                    // from a stolen/cleared stack page).
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
                    // DIAG-V9 (SEGV run 10 — lossy console): run 10 dump
                    // LOST its tail (words 8-15 of stk + ret0= + cs16=) because the
                    // E9 console drops characters at the end of long lines. ret0 is
                    // THE decisive datum (return address of the call/jmp to NULL,
                    // directly addr2line-able) → printed RIGHT NOW, before the 16
                    // stack words, so it survives any truncation.
                    if frame.rip == 0 && (0x1000..0x0000_8000_0000_0000).contains(&frame.rsp) {
                        // SAFETY: plausible user RSP; volatile read.
                        let ret0 = unsafe {
                            core::ptr::read_volatile(frame.rsp as *const u64)
                        };
                        out(b" ret0=");
                        out(&hex(ret0));
                        if (0x1000_0000_0000..0x1001_0000_0000).contains(&ret0) {
                            out(b" cs16=");
                            let base = ret0.saturating_sub(16);
                            for bi in 0..16u64 {
                                // SAFETY: ret0 within user binary range (mapped,
                                // executed right before the call).
                                let byte = unsafe {
                                    core::ptr::read_volatile((base + bi) as *const u8)
                                };
                                let lut = b"0123456789abcdef";
                                out(&[lut[(byte >> 4) as usize], lut[(byte & 0xf) as usize]]);
                            }
                        }
                    }
                    // DIAG-V9: run 10 rdx = 0x1000001ce5b (.text address) —
                    // if rdx points into the binary, dump 8 bytes at rdx: decoding
                    // these bytes identifies if the instruction stream was already
                    // corrupted (garbage) when jumping to 0.
                    if (0x1000_0000_0000..0x1001_0000_0000).contains(&frame.rdx) {
                        out(b" rdx8=");
                        for bi in 0..8u64 {
                            // SAFETY: rdx within user binary range (mapped).
                            let byte = unsafe {
                                core::ptr::read_volatile((frame.rdx + bi) as *const u8)
                            };
                            let lut = b"0123456789abcdef";
                            out(&[lut[(byte >> 4) as usize], lut[(byte & 0xf) as usize]]);
                        }
                    }
                    // DIAG-25: 8 words from user stack at RSP — reveals the return
                    // chain (where the jump to corrupted RIP originates).
                    out(b" stk=");
                    if (0x1000..0x0000_8000_0000_0000).contains(&frame.rsp) {
                        for wi in 0..16u64 {
                            // SAFETY: plausible user RSP; KPTI off; volatile read.
                            let w = unsafe {
                                core::ptr::read_volatile((frame.rsp + wi * 8) as *const u64)
                            };
                            out(&hex(w));
                            out(b" ");
                        }
                    }
                    // (DIAG-NULLCALL bisect run 6 — ret0/cs16 moved to the HEAD of the
                    // dump by FIX-V9 above to survive truncation on lossy consoles;
                    // former tail block is removed.)
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
                // Out of memory — OOM killer notified userspace side.
                let _ = addr;
                queue_signal_for_current(crate::process::signal::Signal::SIGKILL);
                exception_return_to_user(frame);
            } else {
                kernel_panic_exception("#PF kernel : OOM", frame);
            }
        }
        FaultResult::KernelFault { addr } => {
            // Kernel fault — consult fixup table P1-03.
            // If copy_from/to_user_safe is in progress on this CPU, signal
            // the fault rather than panic (copy will return EFAULT).
            let _ = addr;
            let cpu_id = crate::arch::x86_64::cpu::topology::current_cpu_id().0 as usize;
            if crate::syscall::fixup::fixup_lookup(cpu_id).is_some() {
                crate::syscall::fixup::fixup_signal_fault(cpu_id);
                // Normal return — copy loop will see FAULTED=true
                // and return false → EFAULT on syscall side.
            } else {
                kernel_panic_exception("#PF kernel : non récupérable", frame);
            }
        }
    }
}

