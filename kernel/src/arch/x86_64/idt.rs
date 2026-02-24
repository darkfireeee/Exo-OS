//! # arch/x86_64/idt.rs — Interrupt Descriptor Table
//!
//! Gère l'IDT partagée (256 entrées × 16 bytes = 4 KiB).
//! Enregistre les handlers pour :
//! - Vecteurs 0–31 : exceptions CPU
//! - Vecteurs 32–47: IRQ hardware (remappées depuis APIC)
//! - Vecteurs 48+  : interruptions logicielles et IPIs
//!
//! ## IST Assignments (voir tss.rs)
//! - #DF (vecteur 8)  → IST1 (Double Fault)
//! - #NMI(vecteur 2)  → IST2
//! - #MC (vecteur 18) → IST3 (Machine Check)
//! - #DB (vecteur 1)  → IST4 (Debug)

#![allow(dead_code)]

use core::sync::atomic::{AtomicBool, Ordering};

use super::tss::{IST_DOUBLE_FAULT, IST_NMI, IST_MACHINE_CHECK, IST_DEBUG};
use super::gdt::GDT_KERNEL_CS;

// ── Vecteurs d'exception ──────────────────────────────────────────────────────

pub const EXC_DIVIDE_ERROR:     u8  = 0;
pub const EXC_DEBUG:            u8  = 1;
pub const EXC_NMI:              u8  = 2;
pub const EXC_BREAKPOINT:       u8  = 3;
pub const EXC_OVERFLOW:         u8  = 4;
pub const EXC_BOUND_RANGE:      u8  = 5;
pub const EXC_INVALID_OPCODE:   u8  = 6;
pub const EXC_DEVICE_NOT_AVAIL: u8  = 7;   // #NM — Device Not Available (FPU)
pub const EXC_DOUBLE_FAULT:     u8  = 8;
pub const EXC_COPROCESSOR_SEG:  u8  = 9;
pub const EXC_INVALID_TSS:      u8  = 10;
pub const EXC_SEGMENT_NOT_PRES: u8  = 11;
pub const EXC_STACK_FAULT:      u8  = 12;
pub const EXC_GENERAL_PROT:     u8  = 13;
pub const EXC_PAGE_FAULT:       u8  = 14;
pub const EXC_X87_FP:           u8  = 16;
pub const EXC_ALIGNMENT_CHECK:  u8  = 17;
pub const EXC_MACHINE_CHECK:    u8  = 18;
pub const EXC_SIMD_FP:          u8  = 19;
pub const EXC_VIRT:             u8  = 20;  // Virtualization exception
pub const EXC_CTRL_PROT:        u8  = 21;  // Control Protection (#CP)
pub const EXC_HYPERVISOR_INJ:   u8  = 28;
pub const EXC_VMM_COMM:         u8  = 29;
pub const EXC_SECURITY:         u8  = 30;

/// Premier vecteur IRQ hardware (après les 32 exceptions)
pub const IRQ_BASE:             u8  = 32;

/// Vecteur IRQ timer (APIC Local Timer)
pub const VEC_IRQ_TIMER:        u8  = 0x20;

/// Vecteur IPI wakeup (scheduler)
pub const VEC_IPI_WAKEUP:       u8  = 0xF0;

/// Vecteur IPI reschedule
pub const VEC_IPI_RESCHEDULE:   u8  = 0xF1;

/// Vecteur IPI TLB shootdown
pub const VEC_IPI_TLB_SHOOTDOWN: u8 = 0xF2;

/// Vecteur IPI hotplug CPU online/offline
pub const VEC_IPI_CPU_HOTPLUG:  u8  = 0xF3;

/// Vecteur IPI panic broadcast
pub const VEC_IPI_PANIC:        u8  = 0xFE;

/// Vecteur spurious APIC (doit être 0xFF côté APIC)
pub const VEC_SPURIOUS:         u8  = 0xFF;

// ── Descripteur IDT ───────────────────────────────────────────────────────────

/// Drapeaux du descripteur IDT (Type + DPL + P)
#[derive(Clone, Copy)]
pub struct IdtEntryFlags(u8);

impl IdtEntryFlags {
    /// Interrupt gate 64-bit (IF=0 à l'entrée — interruptions désactivées)
    pub const INTERRUPT_GATE: Self = Self(0x8E); // P=1, DPL=0, Type=0xE
    /// Trap gate 64-bit (IF inchangé — interruptions restent actives)
    pub const TRAP_GATE:      Self = Self(0x8F); // P=1, DPL=0, Type=0xF
    /// Trap gate accessible depuis Ring 3 (pour INT3, syscall soft)
    pub const TRAP_GATE_USER: Self = Self(0xEF); // P=1, DPL=3, Type=0xF
}

/// Entrée IDT 64-bit (16 bytes)
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct IdtEntry {
    offset_lo:   u16,   // handler[15:0]
    selector:    u16,   // code segment selector
    ist:         u8,    // IST index (0 = pas d'IST, 1-7 = IST1-IST7)
    flags:       u8,    // type + DPL + P
    offset_mid:  u16,   // handler[31:16]
    offset_hi:   u32,   // handler[63:32]
    _reserved:   u32,
}

impl IdtEntry {
    pub const fn missing() -> Self {
        Self {
            offset_lo:  0,
            selector:   0,
            ist:        0,
            flags:      0, // P=0 → entrée non présente
            offset_mid: 0,
            offset_hi:  0,
            _reserved:  0,
        }
    }

    /// Crée une entrée IDT pour un handler
    ///
    /// `handler`  : pointeur vers la fonction ASM d'entrée
    /// `selector` : sélecteur CS (GDT_KERNEL_CS)
    /// `ist`      : index IST (0 = pile normale, 1-7 = IST dédié)
    /// `flags`    : IdtEntryFlags
    pub fn new(handler: u64, selector: u16, ist: u8, flags: IdtEntryFlags) -> Self {
        Self {
            offset_lo:  (handler & 0xFFFF) as u16,
            selector,
            ist:        ist & 0x7,
            flags:      flags.0,
            offset_mid: ((handler >> 16) & 0xFFFF) as u16,
            offset_hi:  (handler >> 32) as u32,
            _reserved:  0,
        }
    }

    /// Retourne l'adresse complète du handler
    pub fn handler_addr(&self) -> u64 {
        (self.offset_lo  as u64)
            | ((self.offset_mid as u64) << 16)
            | ((self.offset_hi  as u64) << 32)
    }

    /// Retourne `true` si l'entrée est présente (P=1)
    pub fn is_present(&self) -> bool {
        self.flags & 0x80 != 0
    }
}

// ── Table IDT ─────────────────────────────────────────────────────────────────

/// IDT globale (partagée entre les CPUs — les handlers sont identiques)
/// L'IDT est read-only après init, donc le partage est safe.
#[repr(C, align(16))]
pub struct InterruptDescriptorTable {
    entries: [IdtEntry; 256],
}

/// Registre IDTR
#[repr(C, packed)]
struct IdtRegister {
    limit: u16,
    base:  u64,
}

impl InterruptDescriptorTable {
    const fn new() -> Self {
        Self {
            entries: [IdtEntry::missing(); 256],
        }
    }

    /// Installe un handler pour un vecteur donné
    fn set_handler(&mut self, vector: u8, handler: u64, ist: u8, flags: IdtEntryFlags) {
        self.entries[vector as usize] = IdtEntry::new(handler, GDT_KERNEL_CS, ist, flags);
    }
}

static mut IDT: InterruptDescriptorTable = InterruptDescriptorTable::new();
static IDT_INITIALIZED: AtomicBool = AtomicBool::new(false);

// ── Handlers ASM externes ─────────────────────────────────────────────────────

// Ces fonctions sont définies dans les fichiers .s ou exceptions.rs
// via `core::arch::global_asm!` / `extern "C"`
extern "C" {
    // Exceptions
    fn exc_divide_error_handler();
    fn exc_debug_handler();
    fn exc_nmi_handler();
    fn exc_breakpoint_handler();
    fn exc_overflow_handler();
    fn exc_bound_range_handler();
    fn exc_invalid_opcode_handler();
    fn exc_device_not_avail_handler();
    fn exc_double_fault_handler();
    fn exc_invalid_tss_handler();
    fn exc_segment_not_present_handler();
    fn exc_stack_fault_handler();
    fn exc_general_protection_handler();
    fn exc_page_fault_handler();
    fn exc_x87_fp_handler();
    fn exc_alignment_check_handler();
    fn exc_machine_check_handler();
    fn exc_simd_fp_handler();
    fn exc_virtualization_handler();
    fn exc_ctrl_protection_handler();

    // IRQ hardware (timer, clavier, etc.)
    fn irq_timer_handler();
    fn irq_spurious_handler();

    // IPI handlers
    fn ipi_wakeup_handler();
    fn ipi_reschedule_handler();
    fn ipi_tlb_shootdown_handler();
    fn ipi_cpu_hotplug_handler();
    fn ipi_panic_handler();
}

// ── Initialisation IDT ────────────────────────────────────────────────────────

/// Initialise l'IDT globale avec tous les handlers
///
/// Doit être appelé une seule fois depuis le BSP.
pub fn init_idt() {
    // SAFETY: appelé une seule fois depuis le BSP en single-thread
    let idt = unsafe { &mut IDT };

    // ── Exceptions (vecteurs 0–31) ────────────────────────────────────────────
    idt.set_handler(EXC_DIVIDE_ERROR,     exc_divide_error_handler as *const () as u64,     0, IdtEntryFlags::TRAP_GATE);
    idt.set_handler(EXC_DEBUG,            exc_debug_handler as *const () as u64,            IST_DEBUG as u8 + 1, IdtEntryFlags::TRAP_GATE);
    idt.set_handler(EXC_NMI,             exc_nmi_handler as *const () as u64,              IST_NMI as u8 + 1,   IdtEntryFlags::INTERRUPT_GATE);
    idt.set_handler(EXC_BREAKPOINT,       exc_breakpoint_handler as *const () as u64,       0, IdtEntryFlags::TRAP_GATE_USER);
    idt.set_handler(EXC_OVERFLOW,         exc_overflow_handler as *const () as u64,         0, IdtEntryFlags::TRAP_GATE_USER);
    idt.set_handler(EXC_BOUND_RANGE,      exc_bound_range_handler as *const () as u64,      0, IdtEntryFlags::TRAP_GATE);
    idt.set_handler(EXC_INVALID_OPCODE,   exc_invalid_opcode_handler as *const () as u64,   0, IdtEntryFlags::TRAP_GATE);
    idt.set_handler(EXC_DEVICE_NOT_AVAIL, exc_device_not_avail_handler as *const () as u64, 0, IdtEntryFlags::TRAP_GATE);
    idt.set_handler(EXC_DOUBLE_FAULT,     exc_double_fault_handler as *const () as u64,     IST_DOUBLE_FAULT as u8 + 1, IdtEntryFlags::TRAP_GATE);
    idt.set_handler(EXC_INVALID_TSS,      exc_invalid_tss_handler as *const () as u64,      0, IdtEntryFlags::TRAP_GATE);
    idt.set_handler(EXC_SEGMENT_NOT_PRES, exc_segment_not_present_handler as *const () as u64, 0, IdtEntryFlags::TRAP_GATE);
    idt.set_handler(EXC_STACK_FAULT,      exc_stack_fault_handler as *const () as u64,      0, IdtEntryFlags::TRAP_GATE);
    idt.set_handler(EXC_GENERAL_PROT,     exc_general_protection_handler as *const () as u64, 0, IdtEntryFlags::TRAP_GATE);
    idt.set_handler(EXC_PAGE_FAULT,       exc_page_fault_handler as *const () as u64,       0, IdtEntryFlags::TRAP_GATE);
    idt.set_handler(EXC_X87_FP,           exc_x87_fp_handler as *const () as u64,           0, IdtEntryFlags::TRAP_GATE);
    idt.set_handler(EXC_ALIGNMENT_CHECK,  exc_alignment_check_handler as *const () as u64,  0, IdtEntryFlags::TRAP_GATE);
    idt.set_handler(EXC_MACHINE_CHECK,    exc_machine_check_handler as *const () as u64,    IST_MACHINE_CHECK as u8 + 1, IdtEntryFlags::TRAP_GATE);
    idt.set_handler(EXC_SIMD_FP,          exc_simd_fp_handler as *const () as u64,          0, IdtEntryFlags::TRAP_GATE);
    idt.set_handler(EXC_VIRT,             exc_virtualization_handler as *const () as u64,   0, IdtEntryFlags::TRAP_GATE);
    idt.set_handler(EXC_CTRL_PROT,        exc_ctrl_protection_handler as *const () as u64,  0, IdtEntryFlags::TRAP_GATE);

    // ── IRQ hardware (vecteurs 32+) ───────────────────────────────────────────
    idt.set_handler(IRQ_BASE,     irq_timer_handler as *const () as u64,    0, IdtEntryFlags::INTERRUPT_GATE);

    // ── IPIs ──────────────────────────────────────────────────────────────────
    idt.set_handler(VEC_IPI_WAKEUP,        ipi_wakeup_handler as *const () as u64,        0, IdtEntryFlags::INTERRUPT_GATE);
    idt.set_handler(VEC_IPI_RESCHEDULE,    ipi_reschedule_handler as *const () as u64,    0, IdtEntryFlags::INTERRUPT_GATE);
    idt.set_handler(VEC_IPI_TLB_SHOOTDOWN, ipi_tlb_shootdown_handler as *const () as u64, 0, IdtEntryFlags::INTERRUPT_GATE);
    idt.set_handler(VEC_IPI_CPU_HOTPLUG,   ipi_cpu_hotplug_handler as *const () as u64,   0, IdtEntryFlags::INTERRUPT_GATE);
    idt.set_handler(VEC_IPI_PANIC,         ipi_panic_handler as *const () as u64,         0, IdtEntryFlags::INTERRUPT_GATE);
    idt.set_handler(VEC_SPURIOUS,          irq_spurious_handler as *const () as u64,      0, IdtEntryFlags::INTERRUPT_GATE);

    IDT_INITIALIZED.store(true, Ordering::Release);
}

/// Charge l'IDT sur le CPU courant (LIDT)
///
/// Doit être appelé après `init_idt()` sur chaque CPU.
pub fn load_idt() {
    let idtr = IdtRegister {
        limit: (core::mem::size_of::<InterruptDescriptorTable>() - 1) as u16,
        // SAFETY: IDT est une static — son adresse est stable
        base:  unsafe { IDT.entries.as_ptr() as u64 },
    };

    // SAFETY: idtr pointe vers une IDT valide avec tous les handlers présents
    unsafe {
        core::arch::asm!(
            "lidt [{idtr}]",
            idtr = in(reg) &idtr as *const IdtRegister,
            options(nostack, nomem)
        );
    }
}

/// Retourne `true` si l'IDT a été initialisée
#[inline(always)]
pub fn idt_ready() -> bool {
    IDT_INITIALIZED.load(Ordering::Relaxed)
}

/// Retourne le handler address pour le vecteur `vector` (debugging)
pub fn get_handler_addr(vector: u8) -> Option<u64> {
    // SAFETY: IDT read-only après init
    let entry = unsafe { &IDT.entries[vector as usize] };
    if entry.is_present() { Some(entry.handler_addr()) } else { None }
}

// ── Instrumentation IDT ───────────────────────────────────────────────────────

/// Compteurs d'interruptions par vecteur (256 entrées)
static IRQ_COUNTERS: [core::sync::atomic::AtomicU64; 256] = {
    const ZERO: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    [ZERO; 256]
};

/// Incrémente le compteur pour le vecteur donné (appelé depuis les handlers)
#[inline(always)]
pub fn irq_counter_inc(vector: u8) {
    IRQ_COUNTERS[vector as usize].fetch_add(1, Ordering::Relaxed);
}

/// Retourne le compteur d'interruptions pour un vecteur donné
pub fn irq_counter(vector: u8) -> u64 {
    IRQ_COUNTERS[vector as usize].load(Ordering::Relaxed)
}
