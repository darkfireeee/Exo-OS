#![allow(unused_imports, dead_code)]
use super::*;
use super::frame::*;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::arch::x86_64::{apic, halt_cpu, idt, memory_iface, paging, read_cr2, read_cr3, smp};
use core::concat;

// ── Macros ASM pour les stubs d'interruption ──────────────────────────────────

macro_rules! define_exception_handler_errcode {
    ($name:ident, $rust_handler:ident) => {
        core::arch::global_asm!(
            concat!(".global ", stringify!($name)),
            concat!(".type ", stringify!($name), ", @function"),
            concat!(stringify!($name), ":"),
            // À l'entrée du handler, le CPU a pousé (de bas à haut) :
            // [rsp+ 0] = error_code
            // [rsp+ 8] = RIP
            // [rsp+16] = CS  ← RPL bits [1:0] = ring level
            // [rsp+24] = RFLAGS
            // [rsp+32] = RSP  (si changement Ring 3→Ring 0)
            // [rsp+40] = SS   (si changement Ring 3→Ring 0)
            "test qword ptr [rsp + 16], 3", // CS & 3 != 0 ⇒ Ring 3
            "jz   1f",                      // ZF=1 ⇒ Ring 0 (kernel), sauter swapgs
            "swapgs",
            "mov  gs:[0x50], rax", // préserver rax pendant le switch CR3
            "mov  rax, gs:[0x40]", // kpti_kernel_cr3
            "test rax, rax",
            "jz   3f",
            "mov  cr3, rax",
            "3:",
            "mov  rax, gs:[0x50]",
            "1:",
            // Sauvegarder registres callee-saved + scratch
            "push rax",
            "push rbx",
            "push rcx",
            "push rdx",
            "push rsi",
            "push rdi",
            "push rbp",
            "push r8",
            "push r9",
            "push r10",
            "push r11",
            "push r12",
            "push r13",
            "push r14",
            "push r15",
            // ExceptionFrame* dans rdi. Le CPU peut entrer ici avec une pile
            // dont l'alignement n'est pas celui attendu par l'ABI SysV; aligner
            // la pile d'appel Rust sans déplacer la frame que le handler reçoit.
            "mov  rdi, rsp",
            "mov  r12, rsp",
            "and  rsp, -16",
            concat!("call ", stringify!($rust_handler)),
            "mov  rsp, r12",
            // Restaurer registres
            "pop  r15",
            "pop  r14",
            "pop  r13",
            "pop  r12",
            "pop  r11",
            "pop  r10",
            "pop  r9",
            "pop  r8",
            "pop  rbp",
            "pop  rdi",
            "pop  rsi",
            "pop  rdx",
            "pop  rcx",
            "pop  rbx",
            "pop  rax",
            // Dépiler error_code
            "add  rsp, 8",
            // Après add rsp,8 : [rsp+0]=RIP, [rsp+8]=CS, [rsp+16]=RFLAGS
            // Vérifier CS RPL pour le SWAPGS de sortie
            "test qword ptr [rsp + 8], 3", // CS & 3 != 0 ⇒ retour Ring 3
            "jz   2f",
            "mov  gs:[0x50], rax",
            "mov  rax, gs:[0x48]", // kpti_user_cr3
            "test rax, rax",
            "jz   4f",
            "mov  cr3, rax",
            "4:",
            "mov  rax, gs:[0x50]",
            "swapgs",
            "2:",
            "iretq",
        );
    };
}

/// Génère un stub d'entrée exception SANS error code (pousse 0 synthétique)
///
/// Après "push 0" (synthetic error_code), la pile est identique au cas errcode :
/// [rsp+ 0] = 0  (erreur synthétique)
/// [rsp+ 8] = RIP
/// [rsp+16] = CS  ← mêmes offsets que le cas errcode
macro_rules! define_exception_handler_no_errcode {
    ($name:ident, $rust_handler:ident) => {
        core::arch::global_asm!(
            concat!(".global ", stringify!($name)),
            concat!(".type ", stringify!($name), ", @function"),
            concat!(stringify!($name), ":"),
            // Pousser 0 comme error_code synthétique
            "push 0",
            // [rsp+ 0]=0, [rsp+ 8]=RIP, [rsp+16]=CS, [rsp+24]=RFLAGS ...
            "test qword ptr [rsp + 16], 3", // CS & 3 != 0 ⇒ Ring 3
            "jz   1f",
            "swapgs",
            "mov  gs:[0x50], rax", // préserver rax pendant le switch CR3
            "mov  rax, gs:[0x40]", // kpti_kernel_cr3
            "test rax, rax",
            "jz   3f",
            "mov  cr3, rax",
            "3:",
            "mov  rax, gs:[0x50]",
            "1:",
            "push rax",
            "push rbx",
            "push rcx",
            "push rdx",
            "push rsi",
            "push rdi",
            "push rbp",
            "push r8",
            "push r9",
            "push r10",
            "push r11",
            "push r12",
            "push r13",
            "push r14",
            "push r15",
            "mov  rdi, rsp",
            "mov  r12, rsp",
            "and  rsp, -16",
            concat!("call ", stringify!($rust_handler)),
            "mov  rsp, r12",
            "pop  r15",
            "pop  r14",
            "pop  r13",
            "pop  r12",
            "pop  r11",
            "pop  r10",
            "pop  r9",
            "pop  r8",
            "pop  rbp",
            "pop  rdi",
            "pop  rsi",
            "pop  rdx",
            "pop  rcx",
            "pop  rbx",
            "pop  rax",
            "add  rsp, 8",
            // Après add rsp,8 : [rsp+0]=RIP, [rsp+8]=CS
            "test qword ptr [rsp + 8], 3", // CS RPL bits
            "jz   2f",
            "mov  gs:[0x50], rax",
            "mov  rax, gs:[0x48]", // kpti_user_cr3
            "test rax, rax",
            "jz   4f",
            "mov  cr3, rax",
            "4:",
            "mov  rax, gs:[0x50]",
            "swapgs",
            "2:",
            "iretq",
        );
    };
}

// Stubs ASM pour toutes les exceptions
define_exception_handler_no_errcode!(exc_divide_error_handler, do_divide_error);
define_exception_handler_no_errcode!(exc_debug_handler, do_debug);
define_exception_handler_no_errcode!(exc_nmi_handler, do_nmi);
define_exception_handler_no_errcode!(exc_breakpoint_handler, do_breakpoint);
define_exception_handler_no_errcode!(exc_overflow_handler, do_overflow);
define_exception_handler_no_errcode!(exc_bound_range_handler, do_bound_range);
define_exception_handler_no_errcode!(exc_invalid_opcode_handler, do_invalid_opcode);
define_exception_handler_no_errcode!(exc_device_not_avail_handler, do_device_not_avail);
define_exception_handler_errcode!(exc_double_fault_handler, do_double_fault);
define_exception_handler_errcode!(exc_invalid_tss_handler, do_invalid_tss);
define_exception_handler_errcode!(exc_segment_not_present_handler, do_segment_not_present);
define_exception_handler_errcode!(exc_stack_fault_handler, do_stack_fault);
define_exception_handler_errcode!(exc_general_protection_handler, do_general_protection);
define_exception_handler_errcode!(exc_page_fault_handler, do_page_fault);
define_exception_handler_no_errcode!(exc_x87_fp_handler, do_x87_fp);
define_exception_handler_errcode!(exc_alignment_check_handler, do_alignment_check);
define_exception_handler_no_errcode!(exc_machine_check_handler, do_machine_check);
define_exception_handler_no_errcode!(exc_simd_fp_handler, do_simd_fp);
define_exception_handler_no_errcode!(exc_virtualization_handler, do_virtualization);
define_exception_handler_errcode!(exc_ctrl_protection_handler, do_ctrl_protection);

// IRQ et IPI
define_exception_handler_no_errcode!(irq_timer_handler, do_irq_timer);
define_exception_handler_no_errcode!(irq_1_handler, do_irq_1);
define_exception_handler_no_errcode!(irq_2_handler, do_irq_2);
define_exception_handler_no_errcode!(irq_3_handler, do_irq_3);
define_exception_handler_no_errcode!(irq_4_handler, do_irq_4);
define_exception_handler_no_errcode!(irq_5_handler, do_irq_5);
define_exception_handler_no_errcode!(irq_6_handler, do_irq_6);
define_exception_handler_no_errcode!(irq_7_handler, do_irq_7);
define_exception_handler_no_errcode!(irq_8_handler, do_irq_8);
define_exception_handler_no_errcode!(irq_9_handler, do_irq_9);
define_exception_handler_no_errcode!(irq_10_handler, do_irq_10);
define_exception_handler_no_errcode!(irq_11_handler, do_irq_11);
define_exception_handler_no_errcode!(irq_12_handler, do_irq_12);
define_exception_handler_no_errcode!(irq_13_handler, do_irq_13);
define_exception_handler_no_errcode!(irq_14_handler, do_irq_14);
define_exception_handler_no_errcode!(irq_15_handler, do_irq_15);
define_exception_handler_no_errcode!(irq_spurious_handler, do_irq_spurious);
define_exception_handler_no_errcode!(ipi_wakeup_handler, do_ipi_wakeup);
define_exception_handler_no_errcode!(ipi_reschedule_handler, do_ipi_reschedule);
define_exception_handler_no_errcode!(ipi_tlb_shootdown_handler, do_ipi_tlb_shootdown);
define_exception_handler_no_errcode!(ipi_cpu_hotplug_handler, do_ipi_cpu_hotplug);
define_exception_handler_no_errcode!(ipi_panic_handler, do_ipi_panic);
define_exception_handler_no_errcode!(exophoenix_freeze_handler, do_exophoenix_freeze);
define_exception_handler_no_errcode!(exophoenix_pmc_handler, do_exophoenix_pmc);
define_exception_handler_no_errcode!(exophoenix_tlb_handler, do_exophoenix_tlb);

