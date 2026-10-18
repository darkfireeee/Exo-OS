#!/usr/bin/env python3
"""
tools/split_exceptions.py
Découpe modulaire de kernel/src/arch/x86_64/exceptions.rs (1 692 lignes) en :
  - frame.rs       : ExceptionFrame, scheduler C-ABI bridges, retours user, panic, stats
  - stubs.rs       : Macros ASM et déclarations global_asm! des vecteurs 0-255
  - cpu_faults.rs  : Handlers CPU (divide error, GPF, double fault, invalid opcode, etc.)
  - page_fault.rs  : Handler #PF complet (CoW, demand paging, KPTI sync, diagnostics)
  - irq.rs         : IRQ matérielles (timer, spurious, générique) et IPI inter-CPU
  - mod.rs         : Façade et réexports complets
"""

import subprocess
import os
import re

OUT_DIR = os.path.join("kernel", "src", "arch", "x86_64", "exceptions")

def main():
    out = subprocess.check_output(
        ['git', 'show', 'HEAD:kernel/src/arch/x86_64/exceptions.rs'],
        text=True,
        encoding='utf-8'
    )
    lines = out.splitlines(keepends=True)

    os.makedirs(OUT_DIR, exist_ok=True)

    def slice_lines(start, end):
        return "".join(lines[start-1:end])

    def fix_arch_refs(code):
        for sym in ['idt', 'apic', 'paging', 'smp', 'halt_cpu', 'memory_iface', 'read_cr2', 'read_cr3']:
            code = code.replace(f'super::{sym}', f'crate::arch::x86_64::{sym}')
        return code

    def make_pub(code):
        out = []
        for line in code.splitlines():
            stripped = line.strip()
            indent = line[:len(line) - len(line.lstrip())]
            if stripped.startswith("fn "):
                line = indent + "pub fn " + stripped[3:]
            elif stripped.startswith("extern \"C\" fn "):
                line = indent + "pub extern \"C\" fn " + stripped[len("extern \"C\" fn "):]
            elif stripped.startswith("static ") and not stripped.startswith("static EXC_COUNTERS"):
                line = indent + "pub static " + stripped[len("static "):]
            out.append(line)
        return "\n".join(out) + "\n"

    COMMON_HEADER = """#![allow(unused_imports, dead_code)]
use super::*;
use super::frame::*;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::arch::x86_64::{apic, halt_cpu, idt, memory_iface, paging, read_cr2, read_cr3, smp};
"""

    FRAME_HEADER = """#![allow(unused_imports, dead_code)]
use super::*;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::arch::x86_64::{apic, halt_cpu, idt, memory_iface, paging, read_cr2, read_cr3, smp};
"""

    # 1. FRAME.RS :
    # 22..94   : Extern C bridges scheduler + struct ExceptionFrame + impl
    # 309..373 : current_tcb_raw_checked .. exception_return_to_user_after_recovered_fault
    # 415..582 : queue_signal_for_current .. diag_user_exception_dump .. debug_read_cr3
    # 1623..1692: kernel_panic_exception .. instrumentation stats (exc_count...)
    frame_raw = slice_lines(22, 94) + "\n" + slice_lines(309, 373) + "\n" + slice_lines(415, 582) + "\n" + slice_lines(1623, 1692)
    # Dans frame, déclarer les static EXC_COUNTERS etc. en pub
    frame_raw = frame_raw.replace("static EXC_COUNTERS:", "pub static EXC_COUNTERS:")
    frame_raw = frame_raw.replace("static NMI_COUNT:", "pub static NMI_COUNT:")
    frame_raw = frame_raw.replace("static MC_COUNT:", "pub static MC_COUNT:")
    frame_raw = frame_raw.replace("static GP_FAULT_COUNT:", "pub static GP_FAULT_COUNT:")
    frame_raw = frame_raw.replace("static TIMER_IRQ_COUNT:", "pub static TIMER_IRQ_COUNT:")
    frame_raw = frame_raw.replace("static SPURIOUS_IRQ_COUNT:", "pub static SPURIOUS_IRQ_COUNT:")
    frame_raw = frame_raw.replace("static FPU_DEVICE_NOT_AVAIL_COUNT:", "pub static FPU_DEVICE_NOT_AVAIL_COUNT:")
    # Ajouter USER_PF_DEBUG_COUNT toujours disponible
    frame_raw += "\npub static USER_PF_DEBUG_COUNT: AtomicU64 = AtomicU64::new(0);\n"
    frame_code = FRAME_HEADER + "\n" + fix_arch_refs(make_pub(frame_raw))
    with open(os.path.join(OUT_DIR, "frame.rs"), "w", encoding="utf-8") as f:
        f.write(frame_code)
    print("Wrote frame.rs")

    # 2. STUBS.RS :
    # 95..308  : define_exception_handler_* macros + define_irq_stub + global_asm! stubs
    stubs_raw = slice_lines(95, 308)
    stubs_code = COMMON_HEADER + "use core::concat;\n\n" + fix_arch_refs(stubs_raw)
    with open(os.path.join(OUT_DIR, "stubs.rs"), "w", encoding="utf-8") as f:
        f.write(stubs_code)
    print("Wrote stubs.rs")

    # 3. CPU_FAULTS.RS :
    # 714..970  : do_divide_error .. do_general_protection
    # 1344..1421: do_x87_fp .. do_ctrl_protection
    cpu_faults_raw = slice_lines(714, 970) + "\n" + slice_lines(1344, 1421)
    cpu_faults_code = COMMON_HEADER + "\n" + fix_arch_refs(make_pub(cpu_faults_raw))
    with open(os.path.join(OUT_DIR, "cpu_faults.rs"), "w", encoding="utf-8") as f:
        f.write(cpu_faults_code)
    print("Wrote cpu_faults.rs")

    # 4. PAGE_FAULT.RS :
    # 374..414  : sync_kpti_user_fault_mapping
    # 583..712  : debug_user_page_fault
    # 971..1342 : do_page_fault
    page_fault_raw = slice_lines(374, 414) + "\n" + slice_lines(583, 712) + "\n" + slice_lines(971, 1342)
    page_fault_code = COMMON_HEADER + "\n" + fix_arch_refs(make_pub(page_fault_raw))
    with open(os.path.join(OUT_DIR, "page_fault.rs"), "w", encoding="utf-8") as f:
        f.write(page_fault_code)
    print("Wrote page_fault.rs")

    # 5. IRQ.RS :
    # 1423..1622: do_irq_generic .. do_irq_timer .. do_ipi_* .. do_exophoenix_*
    irq_raw = slice_lines(1423, 1622)
    irq_code = COMMON_HEADER + "\n" + fix_arch_refs(make_pub(irq_raw))
    with open(os.path.join(OUT_DIR, "irq.rs"), "w", encoding="utf-8") as f:
        f.write(irq_code)
    print("Wrote irq.rs")

    # 6. MOD.RS :
    mod_code = """//! # arch/x86_64/exceptions — Sous-système de gestion des exceptions et interruptions CPU
//!
//! Organisation modulaire ségréguée :
//! - `frame`      : Registres sauvegardés ExceptionFrame, ponts scheduler, signaux, stats
//! - `stubs`      : Stubs d'assemblage bas niveau (global_asm!) pour IDT
//! - `cpu_faults` : Exceptions matérielles processeur (GPF, Double Fault, UD, NM...)
//! - `page_fault` : Gestionnaire #PF, résolution CoW, demand paging et synchronisation KPTI
//! - `irq`        : Routage des IRQ périphériques et IPIs inter-cœurs APIC

pub mod frame;
pub mod stubs;
pub mod cpu_faults;
pub mod page_fault;
pub mod irq;

pub use frame::*;
pub use cpu_faults::*;
pub use page_fault::*;
pub use irq::*;
"""
    with open(os.path.join(OUT_DIR, "mod.rs"), "w", encoding="utf-8") as f:
        f.write(mod_code)
    print("Wrote mod.rs")

if __name__ == "__main__":
    main()
