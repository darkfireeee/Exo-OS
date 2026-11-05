#![allow(unused_imports, dead_code, clippy::missing_safety_doc)]
use super::*;
use super::frame::*;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::arch::x86_64::{apic, halt_cpu, idt, memory_iface, paging, read_cr2, read_cr3, smp};


/// # Safety
/// `frame` must be a valid pointer to an ExceptionFrame allocated by the ASM stub.
#[inline(always)]
pub unsafe fn do_irq_generic(frame: *mut ExceptionFrame, vector: u8) {
    // SAFETY: identical to do_divide_error — valid pointer passed by ASM stub.
    let frame = unsafe { &mut *frame };
    crate::arch::x86_64::idt::irq_counter_inc(vector);

    // Routing towards GI-03 architecture
    crate::arch::x86_64::irq::routing::dispatch_irq(vector, None);

    // Preserve same userspace return semantics as other handlers.
    if frame.from_userspace() {
        exception_return_to_user(frame);
    }
}

macro_rules! define_generic_irq_handler {
    ($name:ident, $vector:expr) => {
        #[no_mangle]
        pub unsafe extern "C" fn $name(frame: *mut ExceptionFrame) {
            do_irq_generic(frame, $vector);
        }
    };
}

define_generic_irq_handler!(do_irq_1, 33);
define_generic_irq_handler!(do_irq_2, 34);
define_generic_irq_handler!(do_irq_3, 35);
define_generic_irq_handler!(do_irq_4, 36);
define_generic_irq_handler!(do_irq_5, 37);
define_generic_irq_handler!(do_irq_6, 38);
define_generic_irq_handler!(do_irq_7, 39);
define_generic_irq_handler!(do_irq_8, 40);
define_generic_irq_handler!(do_irq_9, 41);
define_generic_irq_handler!(do_irq_10, 42);
define_generic_irq_handler!(do_irq_11, 43);
define_generic_irq_handler!(do_irq_12, 44);
define_generic_irq_handler!(do_irq_13, 45);
define_generic_irq_handler!(do_irq_14, 46);
define_generic_irq_handler!(do_irq_15, 47);

/// Timer IRQ handler (vector 32, APIC timer)
#[no_mangle]
pub unsafe extern "C" fn do_irq_timer(frame: *mut ExceptionFrame) {
    // SAFETY: identical to do_divide_error — valid pointer passed by ASM stub.
    let frame = unsafe { &mut *frame };
    crate::arch::x86_64::idt::irq_counter_inc(32);
    TIMER_IRQ_COUNT.fetch_add(1, Ordering::Relaxed);

    // 1. APIC EOI — acknowledged first to minimize APIC latency.
    // SAFETY: LAPIC initialized before timer IRQs are enabled.
    crate::arch::x86_64::apic::eoi();

    if crate::arch::x86_64::apic::local_apic::lapic_timer_owner()
        == crate::arch::x86_64::apic::local_apic::LapicTimerOwner::ExoNmiWatchdog
    {
        crate::security::exonmi::tick();
        return;
    }

    crate::arch::x86_64::apic::local_apic::rearm_scheduler_timer_tick();

    // 2. Scheduler tick: advances CPU quanta and decides preemption.
    let cpu_id = current_cpu_id_checked();
    let tcb_ptr = current_tcb_raw_checked();
    // SAFETY: scheduler_tick is thread-safe; implicit cli in IRQ handler.
    unsafe {
        scheduler_tick(cpu_id, tcb_ptr as *mut u8);
    }

    // 3. If returning to Ring 3: check preemption + signals.
    if frame.from_userspace() {
        exception_return_to_user(frame);
    }
}

/// Spurious IRQ handler (vector 0xFF)
///
/// # Safety
/// Called exclusively from the interrupt ASM stub.
#[no_mangle]
pub unsafe extern "C" fn do_irq_spurious(_frame: *mut ExceptionFrame) {
    // Spurious IRQ: do NOT send EOI (it is a phantom interrupt)
    SPURIOUS_IRQ_COUNT.fetch_add(1, Ordering::Relaxed);
}

// ── IPI Handlers ──────────────────────────────────────────────────────────────

/// Thread wakeup IPI
///
/// # Safety
/// Called exclusively from the interrupt ASM stub.
#[no_mangle]
pub unsafe extern "C" fn do_ipi_wakeup(_frame: *mut ExceptionFrame) {
    crate::arch::x86_64::idt::irq_counter_inc(crate::arch::x86_64::idt::VEC_IPI_WAKEUP);
    // Reuse sched_ipi_reschedule: sets NEED_RESCHED on current
    // thread (RULE IPI-01) — actual reschedule occurs at IRET.
    let tcb_ptr = current_tcb_raw_checked();
    // SAFETY: sched_ipi_reschedule is thread-safe for current CPU.
    unsafe {
        sched_ipi_reschedule(tcb_ptr as *mut u8);
    }
    // Local APIC EOI — acknowledge IPI before interrupt return.
    // SAFETY: LAPIC initialized prior to any SMP IPI.
    crate::arch::x86_64::apic::eoi();
}

/// Reschedule IPI
///
/// # Safety
/// Called exclusively from the interrupt ASM stub.
#[no_mangle]
pub unsafe extern "C" fn do_ipi_reschedule(frame: *mut ExceptionFrame) {
    let _ = frame;
    crate::arch::x86_64::idt::irq_counter_inc(crate::arch::x86_64::idt::VEC_IPI_RESCHEDULE);

    let tcb_ptr = current_tcb_raw_checked();

    // Set NEED_RESCHED on current thread (RULE IPI-01 DOC3).
    // The actual reschedule will take place at interrupt return when kernel
    // code checks the flag (or at IRET to Ring 3).
    // If scheduler is not yet initialized (tcb_ptr == 0), ignored.
    // SAFETY: sched_ipi_reschedule() is thread-safe for current CPU.
    unsafe {
        sched_ipi_reschedule(tcb_ptr as *mut u8);
    }

    // Local APIC EOI — acknowledge IPI before interrupt return.
    // SAFETY: LAPIC initialized prior to any SMP IPI.
    crate::arch::x86_64::apic::eoi();
}

/// TLB shootdown IPI
///
/// ## memory/ Integration (RULE TLB-01 DOC2)
/// 1. Retrieve current CPU identifier from per-CPU GS.
/// 2. Call `TLB_QUEUE.handle_remote(cpu_id)` which:
///    - Reads flush request from shared TLB queue.
///    - Executes `invlpg` or reloads CR3 according to `TlbFlushType`.
///    - Clears corresponding bit in `completed` field.
/// 3. Local APIC EOI (acknowledge interrupt).
///
/// ## Precedence Rule
/// NEVER call free_pages() before ALL CPUs have acknowledged
/// (`tlb::shootdown()` waits for acknowledgment, not this handler).
///
/// # Safety
/// Called exclusively from the interrupt ASM stub.
#[no_mangle]
pub unsafe extern "C" fn do_ipi_tlb_shootdown(_frame: *mut ExceptionFrame) {
    crate::arch::x86_64::idt::irq_counter_inc(crate::arch::x86_64::idt::VEC_IPI_TLB_SHOOTDOWN);
    crate::arch::x86_64::paging::inc_tlb_shootdown();

    // Logical identifier of current CPU (0-based).
    let cpu_id = crate::arch::x86_64::smp::percpu::current_cpu_id() as u8;

    // Execute TLB flush requested by shootdown sender.
    // SAFETY: called from IRQ handler (implicit cli); TLB_QUEUE thread-safe via spinlock.
    unsafe {
        crate::memory::virt::address_space::tlb::TLB_QUEUE.handle_remote(cpu_id);
    }

    // Local APIC EOI — signals end of interrupt processing.
    // SAFETY: LAPIC initialized before any IPI; EOI is idempotent at this stage.
    crate::arch::x86_64::apic::eoi();
}

/// CPU hotplug IPI
///
/// # Safety
/// Called exclusively from the interrupt ASM stub.
#[no_mangle]
pub unsafe extern "C" fn do_ipi_cpu_hotplug(_frame: *mut ExceptionFrame) {
    crate::arch::x86_64::idt::irq_counter_inc(crate::arch::x86_64::idt::VEC_IPI_CPU_HOTPLUG);

    // EOI before halt so BSP does not hang waiting.
    // SAFETY: LAPIC initialized before any hotplug IPI.
    crate::arch::x86_64::apic::eoi();
    // Identify this CPU and take it offline (→ ! — this CPU does not return).
    let cpu_id = crate::arch::x86_64::smp::percpu::current_cpu_id();
    // SAFETY: hotplug_cpu_halt is idempotent and -> ! (irreversible stop).
    crate::arch::x86_64::smp::hotplug::hotplug_cpu_halt(cpu_id);
}

/// Panic broadcast IPI (0xFE)
///
/// Received by all APs during a kernel panic.
/// Immediate stop without attempting to save.
///
/// # Safety
/// Called exclusively from the interrupt ASM stub.
#[no_mangle]
pub unsafe extern "C" fn do_ipi_panic(_frame: *mut ExceptionFrame) {
    // SAFETY: response to a panic — irreversible halt required
    crate::arch::x86_64::halt_cpu();
}

/// # Safety
/// Called exclusively from the interrupt ASM stub.
#[no_mangle]
pub unsafe extern "C" fn do_exophoenix_freeze(_frame: *mut ExceptionFrame) {
    crate::arch::x86_64::idt::irq_counter_inc(crate::arch::x86_64::idt::VEC_EXOPHOENIX_FREEZE);
    // SAFETY: ExoPhoenix FREEZE IPI handler in Ring 0 interrupt context.
    unsafe { crate::exophoenix::interrupts::handle_freeze_ipi() };
}

/// # Safety
/// Called exclusively from the interrupt ASM stub.
#[no_mangle]
pub unsafe extern "C" fn do_exophoenix_pmc(_frame: *mut ExceptionFrame) {
    crate::arch::x86_64::idt::irq_counter_inc(crate::arch::x86_64::idt::VEC_EXOPHOENIX_PMC);
    // SAFETY: ExoPhoenix PMC snapshot IPI handler in Ring 0 interrupt context.
    unsafe { crate::exophoenix::interrupts::handle_pmc_snapshot_ipi() };
}

/// # Safety
/// Called exclusively from the interrupt ASM stub.
#[no_mangle]
pub unsafe extern "C" fn do_exophoenix_tlb(_frame: *mut ExceptionFrame) {
    crate::arch::x86_64::idt::irq_counter_inc(crate::arch::x86_64::idt::VEC_EXOPHOENIX_TLB);
    // SAFETY: ExoPhoenix TLB flush IPI handler in Ring 0 interrupt context.
    unsafe { crate::exophoenix::interrupts::handle_tlb_flush_ipi() };
}

