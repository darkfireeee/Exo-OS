//! # arch/x86_64/idt.rs — Interrupt Descriptor Table
//!
//! Manages the shared IDT (256 entries × 16 bytes = 4 KiB).
//! Registers handlers for:
//! - Vectors 0–31: CPU exceptions
//! - Vectors 32–47: hardware IRQs (remapped from APIC)
//! - Vectors 48+: software interrupts and IPIs
//!
//! ## IST Assignments (see tss.rs)
//! - #DF (vector 8)  → IST4 (Double Fault)
//! - #NMI (vector 2) → IST3 (NMI fallback)
//! - 0xF0/0xF1/0xF2  → IST1 (critical ExoPhoenix)
//!
//! #PF and #DB use the current thread's kernel stack, not an IST.
//! Exception return may deliver a signal or trigger a reschedule; the
//! saved frame must therefore remain in the current TCB stack.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::gdt::GDT_KERNEL_CS;
use super::tss::{IST_DOUBLE_FAULT, IST_EXOPHOENIX_IPI, IST_MACHINE_CHECK, IST_NMI};

// ── Exception Vectors ────────────────────────────────────────────────────────

pub const EXC_DIVIDE_ERROR: u8 = 0;
pub const EXC_DEBUG: u8 = 1;
pub const EXC_NMI: u8 = 2;
pub const EXC_BREAKPOINT: u8 = 3;
pub const EXC_OVERFLOW: u8 = 4;
pub const EXC_BOUND_RANGE: u8 = 5;
pub const EXC_INVALID_OPCODE: u8 = 6;
pub const EXC_DEVICE_NOT_AVAIL: u8 = 7; // #NM — Device Not Available (FPU)
pub const EXC_DOUBLE_FAULT: u8 = 8;
pub const EXC_COPROCESSOR_SEG: u8 = 9;
pub const EXC_INVALID_TSS: u8 = 10;
pub const EXC_SEGMENT_NOT_PRES: u8 = 11;
pub const EXC_STACK_FAULT: u8 = 12;
pub const EXC_GENERAL_PROT: u8 = 13;
pub const EXC_PAGE_FAULT: u8 = 14;
pub const EXC_X87_FP: u8 = 16;
pub const EXC_ALIGNMENT_CHECK: u8 = 17;
pub const EXC_MACHINE_CHECK: u8 = 18;
pub const EXC_SIMD_FP: u8 = 19;
pub const EXC_VIRT: u8 = 20; // Virtualization exception
pub const EXC_CTRL_PROT: u8 = 21; // Control Protection (#CP)
pub const EXC_HYPERVISOR_INJ: u8 = 28;
pub const EXC_VMM_COMM: u8 = 29;
pub const EXC_SECURITY: u8 = 30;

/// First hardware IRQ vector (after the 32 exceptions)
pub const IRQ_BASE: u8 = 32;
pub const FIRST_USER_IRQ_VECTOR: u8 = IRQ_BASE;
pub const IDT_ENTRY_COUNT: usize = 256;

const _: () = assert!(
    FIRST_USER_IRQ_VECTOR >= 32,
    "first IRQ vector must not overlap CPU exceptions"
);
const _: () = assert!(
    IDT_ENTRY_COUNT == 256,
    "x86_64 IDT must contain 256 entries"
);

/// Timer IRQ vector (Local APIC Timer)
pub const VEC_IRQ_TIMER: u8 = IRQ_BASE;

/// Reschedule IPI vector (scheduler)
pub const VEC_IPI_RESCHEDULE: u8 = 0xE0;

/// Wakeup IPI vector (scheduler)
pub const VEC_IPI_WAKEUP: u8 = 0xE1;

/// TLB shootdown IPI vector
pub const VEC_IPI_TLB_SHOOTDOWN: u8 = 0xE2;

/// CPU hotplug IPI vector
pub const VEC_IPI_CPU_HOTPLUG: u8 = 0xE3;

/// ExoPhoenix Freeze vector (reserved)
pub const VEC_EXOPHOENIX_FREEZE: u8 = 0xF0;

/// ExoPhoenix PMC snapshot vector (reserved)
pub const VEC_EXOPHOENIX_PMC: u8 = 0xF1;

/// ExoPhoenix TLB flush vector (reserved)
pub const VEC_EXOPHOENIX_TLB: u8 = 0xF2;

/// Returns `true` if `vector` belongs to the ExoPhoenix reserved vectors.
#[inline(always)]
pub const fn is_exophoenix_reserved_vector(vector: u8) -> bool {
    vector == VEC_EXOPHOENIX_FREEZE || vector == VEC_EXOPHOENIX_PMC || vector == VEC_EXOPHOENIX_TLB
}

/// Panic broadcast IPI vector
pub const VEC_IPI_PANIC: u8 = 0xFE;

/// APIC spurious vector (must be 0xFF on the APIC side)
pub const VEC_SPURIOUS: u8 = 0xFF;

// ── IDT Descriptor ───────────────────────────────────────────────────────────

/// IDT descriptor flags (Type + DPL + P)
#[derive(Clone, Copy)]
pub struct IdtEntryFlags(u8);

impl IdtEntryFlags {
    /// 64-bit interrupt gate (IF=0 on entry — interrupts disabled)
    pub const INTERRUPT_GATE: Self = Self(0x8E); // P=1, DPL=0, Type=0xE
    /// 64-bit trap gate (IF unchanged — interrupts remain enabled)
    pub const TRAP_GATE: Self = Self(0x8F); // P=1, DPL=0, Type=0xF
    /// Trap gate accessible from Ring 3 (for INT3, software syscall)
    pub const TRAP_GATE_USER: Self = Self(0xEF); // P=1, DPL=3, Type=0xF
}

/// 64-bit IDT entry (16 bytes)
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct IdtEntry {
    offset_lo: u16,  // handler[15:0]
    selector: u16,   // code segment selector
    ist: u8,         // IST index (0 = no IST, 1-7 = IST1-IST7)
    flags: u8,       // type + DPL + P
    offset_mid: u16, // handler[31:16]
    offset_hi: u32,  // handler[63:32]
    _reserved: u32,
}

impl IdtEntry {
    pub const fn missing() -> Self {
        Self {
            offset_lo: 0,
            selector: 0,
            ist: 0,
            flags: 0, // P=0 → entry not present
            offset_mid: 0,
            offset_hi: 0,
            _reserved: 0,
        }
    }

    /// Creates an IDT entry for a handler
    ///
    /// `handler`: pointer to entry ASM function
    /// `selector`: CS selector (GDT_KERNEL_CS)
    /// `ist`: IST index (0 = normal stack, 1-7 = dedicated IST)
    /// `flags`: IdtEntryFlags
    pub fn new(handler: u64, selector: u16, ist: u8, flags: IdtEntryFlags) -> Self {
        Self {
            offset_lo: (handler & 0xFFFF) as u16,
            selector,
            ist: ist & 0x7,
            flags: flags.0,
            offset_mid: ((handler >> 16) & 0xFFFF) as u16,
            offset_hi: (handler >> 32) as u32,
            _reserved: 0,
        }
    }

    /// Returns the full handler address
    pub fn handler_addr(&self) -> u64 {
        (self.offset_lo as u64) | ((self.offset_mid as u64) << 16) | ((self.offset_hi as u64) << 32)
    }

    /// Returns `true` if the entry is present (P=1)
    pub fn is_present(&self) -> bool {
        self.flags & 0x80 != 0
    }
}

// ── IDT Table ─────────────────────────────────────────────────────────────────

/// Global IDT (shared between CPUs — handlers are identical)
/// The IDT is read-only after init, so sharing is safe.
#[repr(C, align(16))]
pub struct InterruptDescriptorTable {
    entries: [IdtEntry; IDT_ENTRY_COUNT],
}

/// IDTR register
#[repr(C, packed)]
struct IdtRegister {
    limit: u16,
    base: u64,
}

impl InterruptDescriptorTable {
    const fn new() -> Self {
        Self {
            entries: [IdtEntry::missing(); IDT_ENTRY_COUNT],
        }
    }

    /// Installs a handler for a given vector
    fn set_handler(&mut self, vector: u8, handler: u64, ist: u8, flags: IdtEntryFlags) {
        self.entries[vector as usize] = IdtEntry::new(handler, GDT_KERNEL_CS, ist, flags);
    }

    /// Updates the IST index of an existing entry.
    ///
    /// `ist` is the hardware index 1..=7 (0 = no IST).
    fn set_stack_index(&mut self, vector: u8, ist: u8) {
        self.entries[vector as usize].ist = ist & 0x7;
    }
}

static mut IDT: InterruptDescriptorTable = InterruptDescriptorTable::new();
static IDT_INITIALIZED: AtomicBool = AtomicBool::new(false);
static IDT_SEALED_FINGERPRINT: AtomicU64 = AtomicU64::new(0);

#[inline(always)]
fn idt_entries_ptr() -> *const IdtEntry {
    // SAFETY: raw address only; callers decide whether they read or load it.
    unsafe { core::ptr::addr_of!(IDT.entries).cast::<IdtEntry>() }
}

fn idt_fingerprint() -> u64 {
    const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    let ptr = idt_entries_ptr().cast::<u8>();
    let len = core::mem::size_of::<InterruptDescriptorTable>();
    let mut hash = FNV_OFFSET_BASIS;

    for offset in 0..len {
        // SAFETY: ptr points at the static IDT and offset stays inside it.
        let byte = unsafe { core::ptr::read_volatile(ptr.add(offset)) };
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }

    hash
}

// ── External ASM Handlers ─────────────────────────────────────────────────────

// These functions are defined in .s files or exceptions.rs
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

    // Hardware IRQs (timer, keyboard, etc.)
    fn irq_timer_handler();
    fn irq_1_handler();
    fn irq_2_handler();
    fn irq_3_handler();
    fn irq_4_handler();
    fn irq_5_handler();
    fn irq_6_handler();
    fn irq_7_handler();
    fn irq_8_handler();
    fn irq_9_handler();
    fn irq_10_handler();
    fn irq_11_handler();
    fn irq_12_handler();
    fn irq_13_handler();
    fn irq_14_handler();
    fn irq_15_handler();
    fn irq_spurious_handler();

    // IPI handlers
    fn ipi_wakeup_handler();
    fn ipi_reschedule_handler();
    fn ipi_tlb_shootdown_handler();
    fn ipi_cpu_hotplug_handler();
    fn ipi_panic_handler();

    // Dedicated ExoPhoenix handlers
    fn exophoenix_freeze_handler();
    fn exophoenix_pmc_handler();
    fn exophoenix_tlb_handler();
}

// ── IDT Initialization ───────────────────────────────────────────────────────

/// Initializes the global IDT with all handlers
///
/// Must be called once from the BSP.
pub fn init_idt() {
    // SAFETY: called once from the BSP in single-threaded mode
    let idt = unsafe { &mut *core::ptr::addr_of_mut!(IDT) };

    // ── Exceptions (vectors 0–31) ─────────────────────────────────────────────
    idt.set_handler(
        EXC_DIVIDE_ERROR,
        exc_divide_error_handler as *const () as u64,
        0,
        IdtEntryFlags::TRAP_GATE,
    );
    idt.set_handler(
        EXC_DEBUG,
        exc_debug_handler as *const () as u64,
        0,
        IdtEntryFlags::INTERRUPT_GATE,
    );
    idt.set_handler(
        EXC_NMI,
        exc_nmi_handler as *const () as u64,
        IST_NMI as u8 + 1,
        IdtEntryFlags::INTERRUPT_GATE,
    );
    idt.set_handler(
        EXC_BREAKPOINT,
        exc_breakpoint_handler as *const () as u64,
        0,
        IdtEntryFlags::TRAP_GATE_USER,
    );
    idt.set_handler(
        EXC_OVERFLOW,
        exc_overflow_handler as *const () as u64,
        0,
        IdtEntryFlags::TRAP_GATE_USER,
    );
    idt.set_handler(
        EXC_BOUND_RANGE,
        exc_bound_range_handler as *const () as u64,
        0,
        IdtEntryFlags::TRAP_GATE,
    );
    idt.set_handler(
        EXC_INVALID_OPCODE,
        exc_invalid_opcode_handler as *const () as u64,
        0,
        IdtEntryFlags::TRAP_GATE,
    );
    idt.set_handler(
        EXC_DEVICE_NOT_AVAIL,
        exc_device_not_avail_handler as *const () as u64,
        0,
        IdtEntryFlags::TRAP_GATE,
    );
    idt.set_handler(
        EXC_DOUBLE_FAULT,
        exc_double_fault_handler as *const () as u64,
        IST_DOUBLE_FAULT as u8 + 1,
        IdtEntryFlags::INTERRUPT_GATE,
    );
    debug_assert_eq!(
        idt.entries[EXC_DOUBLE_FAULT as usize].flags,
        IdtEntryFlags::INTERRUPT_GATE.0,
        "#DF doit utiliser une interrupt gate (IF masqué)"
    );
    idt.set_handler(
        EXC_INVALID_TSS,
        exc_invalid_tss_handler as *const () as u64,
        0,
        IdtEntryFlags::TRAP_GATE,
    );
    idt.set_handler(
        EXC_SEGMENT_NOT_PRES,
        exc_segment_not_present_handler as *const () as u64,
        0,
        IdtEntryFlags::TRAP_GATE,
    );
    idt.set_handler(
        EXC_STACK_FAULT,
        exc_stack_fault_handler as *const () as u64,
        0,
        IdtEntryFlags::TRAP_GATE,
    );
    idt.set_handler(
        EXC_GENERAL_PROT,
        exc_general_protection_handler as *const () as u64,
        0,
        IdtEntryFlags::TRAP_GATE,
    );
    // FIX-25 (CRITICAL): #PF uses the current thread's kernel stack (rsp0),
    // NOT a shared IST.
    //
    // The previous FIX-PF-IST (#PF → IST_PAGE_FAULT) introduced inter-thread
    // corruption = root cause of #25 (init crash post-fork): an IST is
    // PER-CPU, NOT per-thread, and NOT re-entrant. The CPU reloads rsp = top
    // of the IST on EVERY #PF, regardless of current rsp. Therefore:
    //   • a nested #PF (the handler touches user memory — signal delivery
    //     on user stack, copy_from_user — which faults) clobbers the frame of the
    //     outer #PF; and
    //   • if the #PF handler reschedules / is co-scheduled (init + fork child
    //     page-faulting daemon in parallel at boot), the other thread's #PF reuses
    //     the SAME IST stack from its top and overwrites the saved fault frame
    //     of the first → IRETQ restores garbage RIP/RSP/registers → SIGSEGV at
    //     variable sites (exact symptom of #25, reproduced 2026-06-21).
    // ISTs are reserved for #DF/#NMI/#MCE. With rsp0, #PF is re-entrant
    // (pushes onto the thread stack, which grows downward). The historical risk
    // (#PF on overflowing kernel stack → #DF) remains covered: #DF has ITS OWN
    // IST (IST_DOUBLE_FAULT) → handled, never triple faults.
    idt.set_handler(
        EXC_PAGE_FAULT,
        exc_page_fault_handler as *const () as u64,
        0,
        IdtEntryFlags::INTERRUPT_GATE,
    );
    idt.set_handler(
        EXC_X87_FP,
        exc_x87_fp_handler as *const () as u64,
        0,
        IdtEntryFlags::TRAP_GATE,
    );
    idt.set_handler(
        EXC_ALIGNMENT_CHECK,
        exc_alignment_check_handler as *const () as u64,
        0,
        IdtEntryFlags::TRAP_GATE,
    );
    idt.set_handler(
        EXC_MACHINE_CHECK,
        exc_machine_check_handler as *const () as u64,
        IST_MACHINE_CHECK as u8 + 1,
        IdtEntryFlags::INTERRUPT_GATE,
    );
    idt.set_handler(
        EXC_SIMD_FP,
        exc_simd_fp_handler as *const () as u64,
        0,
        IdtEntryFlags::TRAP_GATE,
    );
    idt.set_handler(
        EXC_VIRT,
        exc_virtualization_handler as *const () as u64,
        0,
        IdtEntryFlags::TRAP_GATE,
    );
    idt.set_handler(
        EXC_CTRL_PROT,
        exc_ctrl_protection_handler as *const () as u64,
        0,
        IdtEntryFlags::TRAP_GATE,
    );

    // ── Hardware IRQs (vectors 32+) ───────────────────────────────────────────
    idt.set_handler(
        VEC_IRQ_TIMER,
        irq_timer_handler as *const () as u64,
        0,
        IdtEntryFlags::INTERRUPT_GATE,
    );
    idt.set_handler(
        IRQ_BASE + 1,
        irq_1_handler as *const () as u64,
        0,
        IdtEntryFlags::INTERRUPT_GATE,
    );
    idt.set_handler(
        IRQ_BASE + 2,
        irq_2_handler as *const () as u64,
        0,
        IdtEntryFlags::INTERRUPT_GATE,
    );
    idt.set_handler(
        IRQ_BASE + 3,
        irq_3_handler as *const () as u64,
        0,
        IdtEntryFlags::INTERRUPT_GATE,
    );
    idt.set_handler(
        IRQ_BASE + 4,
        irq_4_handler as *const () as u64,
        0,
        IdtEntryFlags::INTERRUPT_GATE,
    );
    idt.set_handler(
        IRQ_BASE + 5,
        irq_5_handler as *const () as u64,
        0,
        IdtEntryFlags::INTERRUPT_GATE,
    );
    idt.set_handler(
        IRQ_BASE + 6,
        irq_6_handler as *const () as u64,
        0,
        IdtEntryFlags::INTERRUPT_GATE,
    );
    idt.set_handler(
        IRQ_BASE + 7,
        irq_7_handler as *const () as u64,
        0,
        IdtEntryFlags::INTERRUPT_GATE,
    );
    idt.set_handler(
        IRQ_BASE + 8,
        irq_8_handler as *const () as u64,
        0,
        IdtEntryFlags::INTERRUPT_GATE,
    );
    idt.set_handler(
        IRQ_BASE + 9,
        irq_9_handler as *const () as u64,
        0,
        IdtEntryFlags::INTERRUPT_GATE,
    );
    idt.set_handler(
        IRQ_BASE + 10,
        irq_10_handler as *const () as u64,
        0,
        IdtEntryFlags::INTERRUPT_GATE,
    );
    idt.set_handler(
        IRQ_BASE + 11,
        irq_11_handler as *const () as u64,
        0,
        IdtEntryFlags::INTERRUPT_GATE,
    );
    idt.set_handler(
        IRQ_BASE + 12,
        irq_12_handler as *const () as u64,
        0,
        IdtEntryFlags::INTERRUPT_GATE,
    );
    idt.set_handler(
        IRQ_BASE + 13,
        irq_13_handler as *const () as u64,
        0,
        IdtEntryFlags::INTERRUPT_GATE,
    );
    idt.set_handler(
        IRQ_BASE + 14,
        irq_14_handler as *const () as u64,
        0,
        IdtEntryFlags::INTERRUPT_GATE,
    );
    idt.set_handler(
        IRQ_BASE + 15,
        irq_15_handler as *const () as u64,
        0,
        IdtEntryFlags::INTERRUPT_GATE,
    );

    // ── IPIs ──────────────────────────────────────────────────────────────────
    idt.set_handler(
        VEC_IPI_WAKEUP,
        ipi_wakeup_handler as *const () as u64,
        0,
        IdtEntryFlags::INTERRUPT_GATE,
    );
    idt.set_handler(
        VEC_IPI_RESCHEDULE,
        ipi_reschedule_handler as *const () as u64,
        0,
        IdtEntryFlags::INTERRUPT_GATE,
    );
    idt.set_handler(
        VEC_IPI_TLB_SHOOTDOWN,
        ipi_tlb_shootdown_handler as *const () as u64,
        0,
        IdtEntryFlags::INTERRUPT_GATE,
    );
    idt.set_handler(
        VEC_IPI_CPU_HOTPLUG,
        ipi_cpu_hotplug_handler as *const () as u64,
        0,
        IdtEntryFlags::INTERRUPT_GATE,
    );
    idt.set_handler(
        VEC_EXOPHOENIX_FREEZE,
        exophoenix_freeze_handler as *const () as u64,
        IST_EXOPHOENIX_IPI as u8 + 1,
        IdtEntryFlags::INTERRUPT_GATE,
    );
    idt.set_handler(
        VEC_EXOPHOENIX_PMC,
        exophoenix_pmc_handler as *const () as u64,
        IST_EXOPHOENIX_IPI as u8 + 1,
        IdtEntryFlags::INTERRUPT_GATE,
    );
    idt.set_handler(
        VEC_EXOPHOENIX_TLB,
        exophoenix_tlb_handler as *const () as u64,
        IST_EXOPHOENIX_IPI as u8 + 1,
        IdtEntryFlags::INTERRUPT_GATE,
    );
    idt.set_stack_index(VEC_EXOPHOENIX_FREEZE, IST_EXOPHOENIX_IPI as u8 + 1);
    idt.set_stack_index(VEC_EXOPHOENIX_PMC, IST_EXOPHOENIX_IPI as u8 + 1);
    idt.set_stack_index(VEC_EXOPHOENIX_TLB, IST_EXOPHOENIX_IPI as u8 + 1);
    idt.set_handler(
        VEC_IPI_PANIC,
        ipi_panic_handler as *const () as u64,
        0,
        IdtEntryFlags::INTERRUPT_GATE,
    );
    idt.set_handler(
        VEC_SPURIOUS,
        irq_spurious_handler as *const () as u64,
        0,
        IdtEntryFlags::INTERRUPT_GATE,
    );

    IDT_SEALED_FINGERPRINT.store(idt_fingerprint(), Ordering::Release);
    IDT_INITIALIZED.store(true, Ordering::Release);
}

/// Loads the IDT on the current CPU (LIDT)
///
/// Must be called after `init_idt()` on each CPU.
pub fn load_idt() {
    let idtr = IdtRegister {
        limit: (core::mem::size_of::<InterruptDescriptorTable>() - 1) as u16,
        // SAFETY: IDT is a static — its address is stable
        base: idt_entries_ptr() as u64,
    };

    // SAFETY: idtr points to a valid IDT with all handlers present
    unsafe {
        core::arch::asm!(
            "lidt [{idtr}]",
            idtr = in(reg) &idtr as *const IdtRegister,
            options(nostack, readonly, preserves_flags)
        );
    }
}

/// Returns `true` if the IDT has been initialized
#[inline(always)]
pub fn idt_ready() -> bool {
    IDT_INITIALIZED.load(Ordering::Relaxed)
}

/// Verifies that the post-init IDT bytes still match the sealed fingerprint.
///
/// NMI callers use a volatile byte fingerprint so a descriptor rewrite is
/// observed without allocating or taking locks in the interrupt path.
#[inline]
pub fn idt_integrity_ok() -> bool {
    let sealed = IDT_SEALED_FINGERPRINT.load(Ordering::Acquire);
    sealed != 0 && idt_ready() && sealed == idt_fingerprint()
}

/// Returns the handler address for vector `vector` (debugging)
pub fn get_handler_addr(vector: u8) -> Option<u64> {
    // SAFETY: IDT is read-only after init
    let entry = unsafe { &*idt_entries_ptr().add(vector as usize) };
    if entry.is_present() {
        Some(entry.handler_addr())
    } else {
        None
    }
}

#[inline(always)]
pub fn idt_base_addr_for_kpti() -> u64 {
    idt_entries_ptr() as u64
}

#[inline(always)]
pub fn exophoenix_freeze_handler_addr() -> u64 {
    get_handler_addr(VEC_EXOPHOENIX_FREEZE).unwrap_or(0)
}

#[inline(always)]
pub fn exophoenix_pmc_handler_addr() -> u64 {
    get_handler_addr(VEC_EXOPHOENIX_PMC).unwrap_or(0)
}

#[inline(always)]
pub fn exophoenix_tlb_handler_addr() -> u64 {
    get_handler_addr(VEC_EXOPHOENIX_TLB).unwrap_or(0)
}

// ── IDT Instrumentation ──────────────────────────────────────────────────────

/// Interrupt counters per vector (256 entries)
static IRQ_COUNTERS: [core::sync::atomic::AtomicU64; IDT_ENTRY_COUNT] =
    [const { core::sync::atomic::AtomicU64::new(0) }; IDT_ENTRY_COUNT];

/// Increments counter for the given vector (called from handlers)
#[inline(always)]
pub fn irq_counter_inc(vector: u8) {
    IRQ_COUNTERS[vector as usize].fetch_add(1, Ordering::Relaxed);
}

/// Returns interrupt counter for a given vector
pub fn irq_counter(vector: u8) -> u64 {
    IRQ_COUNTERS[vector as usize].load(Ordering::Relaxed)
}
