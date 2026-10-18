#![allow(unused_imports, dead_code)]
use super::*;
use super::frame::*;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::arch::x86_64::{apic, halt_cpu, idt, memory_iface, paging, read_cr2, read_cr3, smp};

#[no_mangle]
pub extern "C" fn do_divide_error(frame: *mut ExceptionFrame) {
    // SAFETY: `frame` est un pointeur non-null passé par le stub ASM, aligné 16 B,
    // unique pour ce contexte d'exception. Sa durée de vie est garantie par la
    // frame de pile que le CPU a poussée avant l'appel au handler.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[0].fetch_add(1, Ordering::Relaxed);

    if frame.from_userspace() {
        // Envoyer SIGFPE au processus courant via exception_return_to_user
        // (proc_signal_on_exception_return → process::signal::delivery — RÈGLE SIGNAL-01)
        exception_return_to_user(frame);
    } else {
        kernel_panic_exception("Division par zéro kernel", frame);
    }
}

/// Handler #DB — Debug exception
#[no_mangle]
pub extern "C" fn do_debug(frame: *mut ExceptionFrame) {
    // SAFETY: identique à do_divide_error — pointeur valide passé par le stub ASM.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[1].fetch_add(1, Ordering::Relaxed);

    if frame.from_userspace() {
        // SIGTRAP → ptrace/GDB
        exception_return_to_user(frame);
    } else {
        kernel_panic_exception("#DB kernel", frame);
    }
}

/// Handler #NMI — Non-Maskable Interrupt
///
/// NMI utilise IST2 (pile dédiée indépendante de la pile courante).
/// NE PAS appeler de code qui pourrait provoquer une exception.
#[no_mangle]
pub extern "C" fn do_nmi(frame: *mut ExceptionFrame) {
    // SAFETY: `frame` est fourni par le stub d'interruption NMI sur IST2, pointe
    // vers une ExceptionFrame valide construite par le CPU à l'entrée du handler.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[2].fetch_add(1, Ordering::Relaxed);
    NMI_COUNT.fetch_add(1, Ordering::Relaxed);

    if crate::exophoenix::resurrection::handle_nmi(frame) {
        return;
    }

    crate::security::exonmi::exonmi_check_idt_integrity();
}

/// Handler #BP — Breakpoint (INT3)
#[no_mangle]
pub extern "C" fn do_breakpoint(frame: *mut ExceptionFrame) {
    // SAFETY: identique à do_divide_error — pointeur valide passé par le stub ASM.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[3].fetch_add(1, Ordering::Relaxed);

    if frame.from_userspace() {
        // SIGTRAP
        exception_return_to_user(frame);
    }
    // Kernel : kprobe ou debug noyau
}

/// Handler #OF — Overflow (INTO)
#[no_mangle]
pub extern "C" fn do_overflow(frame: *mut ExceptionFrame) {
    // SAFETY: identique à do_divide_error — pointeur valide passé par le stub ASM.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[4].fetch_add(1, Ordering::Relaxed);
    if frame.from_userspace() {
        exception_return_to_user(frame);
    } else {
        kernel_panic_exception("Overflow kernel", frame);
    }
}

/// Handler #BR — Bound Range Exceeded
#[no_mangle]
pub extern "C" fn do_bound_range(frame: *mut ExceptionFrame) {
    // SAFETY: identique à do_divide_error — pointeur valide passé par le stub ASM.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[5].fetch_add(1, Ordering::Relaxed);
    if frame.from_userspace() {
        exception_return_to_user(frame);
    } else {
        kernel_panic_exception("Bound Range kernel", frame);
    }
}

/// Handler #UD — Invalid Opcode
#[no_mangle]
pub extern "C" fn do_invalid_opcode(frame: *mut ExceptionFrame) {
    // SAFETY: identique à do_divide_error — pointeur valide passé par le stub ASM.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[6].fetch_add(1, Ordering::Relaxed);

    if frame.from_userspace() {
        // DIAG-UD1 (bisect mort d'init) : dump complet AVANT de livrer SIGILL —
        // rip + ripb (octets in-memory : `0f 0b` = ud2 compilé, octets aléatoires
        // = saut corrompu #25) + pile (chaîne de return addresses).
        diag_user_exception_dump(b"<UD1 ", frame);
        // FIX-UD-SIGILL : livrer réellement SIGILL. Sans ça, on retournait au RIP
        // fautif tel quel → l'instruction invalide (typiquement `ud2` d'un panic
        // Rust côté serveur) re-déclenche immédiatement #UD → livelock noyau
        // (boucle do_invalid_opcode → iretq → même opcode) gelant tout le système.
        // Comme #PF/#GP, on met SIGILL en attente puis on repasse en Ring 3 : le
        // signal est délivré (handler ou terminaison) au lieu de boucler.
        queue_signal_for_current(crate::process::signal::Signal::SIGILL);
        exception_return_to_user(frame);
    } else {
        // Vérifier si c'est une instruction XTEST (TSX) — retpoline parfois génère #UD
        // Pour l'instant : kernel panic
        kernel_panic_exception("Invalid Opcode kernel", frame);
    }
}

/// Handler #NM — Device Not Available (FPU lazy)
///
/// Déclenché quand CR0.TS=1 et qu'un thread tente d'utiliser la FPU.
/// scheduler/fpu/lazy.rs gère la logique — arch/ gère l'exception.
#[no_mangle]
pub extern "C" fn do_device_not_avail(frame: *mut ExceptionFrame) {
    let _ = frame;
    EXC_COUNTERS[7].fetch_add(1, Ordering::Relaxed);
    FPU_DEVICE_NOT_AVAIL_COUNT.fetch_add(1, Ordering::Relaxed);

    let tcb_ptr = current_tcb_raw_checked();

    // Déléguer au scheduler (RÈGLE FPU-02 DOC3) :
    //   • Efface CR0.TS (autorise l'accès FPU/SIMD).
    //   • Appelle xrstor pour restaurer le contexte FPU du thread courant.
    //   • Met à jour fpu_loaded = true dans le TCB.
    // Si tcb_ptr == 0 (boot / idle) : simple clts sans xrstor.
    // SAFETY: sched_fpu_handle_nm() est thread-safe pour le CPU courant.
    unsafe {
        sched_fpu_handle_nm(tcb_ptr as *mut u8);
    }
}

/// Handler #DF — Double Fault
///
/// Utilise IST1 (pile dédiée). Si on arrive ici, la pile kernel principale
/// est probablement corrompue ou overflow.
///
/// NE PAS allouer, NE PAS prendre de verrous.
#[no_mangle]
pub extern "C" fn do_double_fault(frame: *mut ExceptionFrame) {
    // DIAG: marqueur brut inconditionnel à l'entrée (#DF sur IST1).
    // SAFETY: port E9, sans effet mémoire.
    unsafe { core::arch::asm!("out 0xE9, al", in("al") b'D', options(nomem, nostack)) };
    EXC_COUNTERS[8].fetch_add(1, Ordering::Relaxed);
    df_debug_write(b"\n[#DF] double fault");
    if !frame.is_null() {
        // SAFETY: frame non-null (vérifié) ; ExceptionFrame construite par le CPU
        // sur la pile IST du #DF, valide pour lecture pendant le handler.
        let frame = unsafe { &*frame };
        df_debug_field(b" rip=0x", frame.rip);
        df_debug_field(b" cs=0x", frame.cs);
        df_debug_field(b" rsp=0x", frame.rsp);
        df_debug_field(b" rflags=0x", frame.rflags);
        df_debug_field(b" err=0x", frame.error_code);
    }
    df_debug_field(
        b" cpu=0x",
        crate::arch::x86_64::cpu::topology::current_cpu_id().0 as u64,
    );
    df_debug_write(b"\n");

    // RÈGLE NO-ALLOC : cette fonction ne peut faire aucune allocation
    // Elle ne peut qu'arrêter le CPU
    // SAFETY: situation non-récupérable — halt immédiat
    crate::arch::x86_64::halt_cpu();
}

#[inline(always)]
pub fn df_debug_write(bytes: &[u8]) {
    crate::arch::x86_64::terminal::debug_write(bytes);
}

pub fn df_debug_hex64(value: u64) {
    let mut out = [0u8; 16];
    for (idx, byte) in out.iter_mut().enumerate() {
        let shift = 60 - (idx as u32 * 4);
        let nibble = ((value >> shift) & 0x0f) as u8;
        *byte = if nibble < 10 {
            b'0' + nibble
        } else {
            b'a' + (nibble - 10)
        };
    }
    df_debug_write(&out);
}

pub fn df_debug_field(label: &[u8], value: u64) {
    df_debug_write(label);
    df_debug_hex64(value);
}

/// Handler #TS — Invalid TSS
#[no_mangle]
pub extern "C" fn do_invalid_tss(frame: *mut ExceptionFrame) {
    // SAFETY: identique à do_divide_error — pointeur valide passé par le stub ASM.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[10].fetch_add(1, Ordering::Relaxed);
    kernel_panic_exception("Invalid TSS", frame);
}

/// Handler #NP — Segment Not Present
#[no_mangle]
pub extern "C" fn do_segment_not_present(frame: *mut ExceptionFrame) {
    // SAFETY: identique à do_divide_error — pointeur valide passé par le stub ASM.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[11].fetch_add(1, Ordering::Relaxed);
    if frame.from_userspace() {
        // SIGBUS
        exception_return_to_user(frame);
    } else {
        kernel_panic_exception("Segment Not Present kernel", frame);
    }
}

/// Handler #SS — Stack Fault
#[no_mangle]
pub extern "C" fn do_stack_fault(frame: *mut ExceptionFrame) {
    // SAFETY: identique à do_divide_error — pointeur valide passé par le stub ASM.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[12].fetch_add(1, Ordering::Relaxed);
    if frame.from_userspace() {
        exception_return_to_user(frame);
    } else {
        kernel_panic_exception("Stack Fault kernel", frame);
    }
}

/// Handler #GP — General Protection Fault
#[no_mangle]
pub extern "C" fn do_general_protection(frame: *mut ExceptionFrame) {
    // SAFETY: identique à do_divide_error — pointeur valide passé par le stub ASM.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[13].fetch_add(1, Ordering::Relaxed);
    GP_FAULT_COUNT.fetch_add(1, Ordering::Relaxed);

    if frame.from_userspace() {
        // DIAG-GP1 (bisect mort d'init) : dump complet du #GP userspace.
        diag_user_exception_dump(b"<GP1 ", frame);
        // FIX-GP-SIGSEGV : un #GP Ring 3 doit livrer SIGSEGV. L'ancien code ne
        // posait AUCUN signal et retournait à la même instruction → #GP en
        // boucle (livelock silencieux). Même traitement que #PF/#UD.
        queue_signal_for_current(crate::process::signal::Signal::SIGSEGV);
        exception_return_to_user(frame);
    } else {
        kernel_panic_exception("#GP kernel", frame);
    }
}


#[no_mangle]
pub extern "C" fn do_x87_fp(frame: *mut ExceptionFrame) {
    // SAFETY: identique à do_divide_error — pointeur valide passé par le stub ASM.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[16].fetch_add(1, Ordering::Relaxed);
    if frame.from_userspace() {
        exception_return_to_user(frame);
    } else {
        kernel_panic_exception("#MF x87 kernel", frame);
    }
}

/// Handler #AC — Alignment Check
#[no_mangle]
pub extern "C" fn do_alignment_check(frame: *mut ExceptionFrame) {
    // SAFETY: identique à do_divide_error — pointeur valide passé par le stub ASM.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[17].fetch_add(1, Ordering::Relaxed);
    if frame.from_userspace() {
        exception_return_to_user(frame);
    } else {
        kernel_panic_exception("#AC kernel", frame);
    }
}

/// Handler #MC — Machine Check
///
/// Utilise IST3 (pile dédiée).
/// Ne peut pas se récupérer. Arrêt immédiat.
#[no_mangle]
pub extern "C" fn do_machine_check(_frame: *mut ExceptionFrame) {
    EXC_COUNTERS[18].fetch_add(1, Ordering::Relaxed);
    MC_COUNT.fetch_add(1, Ordering::Relaxed);
    // SAFETY: machine check = hardware non-récupérable
    crate::arch::x86_64::halt_cpu();
}

/// Handler #XM — SIMD FP Exception
#[no_mangle]
pub extern "C" fn do_simd_fp(frame: *mut ExceptionFrame) {
    // SAFETY: identique à do_divide_error — pointeur valide passé par le stub ASM.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[19].fetch_add(1, Ordering::Relaxed);
    if frame.from_userspace() {
        exception_return_to_user(frame);
    } else {
        kernel_panic_exception("#XM SIMD FP kernel", frame);
    }
}

/// Handler #VE — Virtualization Exception
#[no_mangle]
pub extern "C" fn do_virtualization(frame: *mut ExceptionFrame) {
    // SAFETY: identique à do_divide_error — pointeur valide passé par le stub ASM.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[20].fetch_add(1, Ordering::Relaxed);
    if frame.from_userspace() {
        exception_return_to_user(frame);
    } else {
        kernel_panic_exception("#VE virtualization kernel", frame);
    }
}

/// Handler #CP — Control Protection Exception (CET)
#[no_mangle]
pub extern "C" fn do_ctrl_protection(frame: *mut ExceptionFrame) {
    // SAFETY: identique à do_divide_error — pointeur valide passé par le stub ASM.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[21].fetch_add(1, Ordering::Relaxed);
    crate::security::exocage::cp_handler(frame as *mut ExceptionFrame as usize, frame.error_code);

    if frame.from_userspace() {
        exception_return_to_user(frame);
    } else {
        kernel_panic_exception("#CP Control Protection kernel (post ExoCage)", frame);
    }
}

