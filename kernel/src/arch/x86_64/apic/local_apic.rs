//! # arch/x86_64/apic/local_apic.rs — Local APIC (xAPIC MMIO)
//!
//! Implements Local APIC access via MMIO (xAPIC mode, base 0xFEE00000)
//! and primitives shared with x2APIC (via MSR).
//!
//! ## LAPIC Registers (offset from LAPIC_BASE)
//! Registers are all 32-bit, 16-byte aligned.

use super::super::cpu::{msr, tsc};
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicU32, AtomicU8, AtomicUsize, Ordering};

// ── LAPIC MMIO Address ────────────────────────────────────────────────────────

/// Default Local APIC MMIO address (can be re-read from MSR_IA32_APICBASE)
pub const LAPIC_DEFAULT_BASE: u64 = 0xFEE0_0000;

/// Current LAPIC physical base (may differ after ACPI remapping).
static LAPIC_PHYS_BASE: AtomicUsize = AtomicUsize::new(LAPIC_DEFAULT_BASE as usize);
/// Virtual base used by MMIO accesses.
///
/// Before full memory init, it remains the low physical address inherited
/// from boot mappings. After `init_lapic_fixmap_post_memory()`, it points into
/// the high kernel fixmap, shared across userspace CR3s.
static LAPIC_MMIO_BASE: AtomicUsize = AtomicUsize::new(LAPIC_DEFAULT_BASE as usize);
static LAPIC_TIMER_OWNER: AtomicU8 = AtomicU8::new(LapicTimerOwner::None as u8);
static SCHEDULER_TIMER_USES_TSC_DEADLINE: AtomicU8 = AtomicU8::new(0);
const SCHEDULER_TICK_US: u64 = 1_000;

#[inline(always)]
fn lapic_uses_fixmap() -> bool {
    LAPIC_MMIO_BASE.load(Ordering::Acquire)
        >= crate::memory::core::layout::FIXMAP_BASE.as_u64() as usize
}

// ── LAPIC Register Offsets ───────────────────────────────────────────────────

pub const LAPIC_ID: u32 = 0x020; // LAPIC ID
pub const LAPIC_VER: u32 = 0x030; // Version
pub const LAPIC_TPR: u32 = 0x080; // Task Priority Register
pub const LAPIC_APR: u32 = 0x090; // Arbitration Priority Register
pub const LAPIC_PPR: u32 = 0x0A0; // Processor Priority Register
pub const LAPIC_EOI: u32 = 0x0B0; // End Of Interrupt
pub const LAPIC_RRD: u32 = 0x0C0; // Remote Read Register
pub const LAPIC_LDR: u32 = 0x0D0; // Logical Destination Register
pub const LAPIC_DFR: u32 = 0x0E0; // Destination Format Register
pub const LAPIC_SIVR: u32 = 0x0F0; // Spurious Interrupt Vector Register
pub const LAPIC_ISR0: u32 = 0x100; // In-Service Register [0..7] x 0x10
pub const LAPIC_TMR0: u32 = 0x180; // Trigger Mode Register
pub const LAPIC_IRR0: u32 = 0x200; // Interrupt Request Register
pub const LAPIC_ESR: u32 = 0x280; // Error Status Register
pub const LAPIC_LVT_CMCI: u32 = 0x2F0; // LVT CMCI
pub const LAPIC_ICR_LOW: u32 = 0x300; // Interrupt Command Register (low)
pub const LAPIC_ICR_HIGH: u32 = 0x310; // Interrupt Command Register (high)
pub const LAPIC_LVT_TIMER: u32 = 0x320; // LVT Timer
pub const LAPIC_LVT_THERMAL: u32 = 0x330; // LVT Thermal Sensor
pub const LAPIC_LVT_PERF: u32 = 0x340; // LVT Performance Counter
pub const LAPIC_LVT_LINT0: u32 = 0x350; // LVT Local Interrupt 0
pub const LAPIC_LVT_LINT1: u32 = 0x360; // LVT Local Interrupt 1
pub const LAPIC_LVT_ERROR: u32 = 0x370; // LVT Error
pub const LAPIC_TIMER_ICR: u32 = 0x380; // Timer Initial Count
pub const LAPIC_TIMER_CCR: u32 = 0x390; // Timer Current Count
pub const LAPIC_TIMER_DCR: u32 = 0x3E0; // Timer Divide Configuration

// ── SIVR Register Bits ────────────────────────────────────────────────────────

pub const SIVR_APIC_ENABLE: u32 = 1 << 8;
pub const SIVR_FOCUS_DISABLE: u32 = 1 << 9;

// ── Timer modes (bits 17:18 of LVT_TIMER) ─────────────────────────────────────

pub const TIMER_MODE_ONESHOT: u32 = 0b00 << 17;
pub const TIMER_MODE_PERIODIC: u32 = 0b01 << 17;
pub const TIMER_MODE_TSC_DEADLINE: u32 = 0b10 << 17;

// ── ICR delivery modes ────────────────────────────────────────────────────────

pub const ICR_DM_FIXED: u32 = 0b000 << 8;
pub const ICR_DM_NMI: u32 = 0b100 << 8;
pub const ICR_DM_INIT: u32 = 0b101 << 8;
pub const ICR_DM_STARTUP: u32 = 0b110 << 8;

pub const ICR_DEST_SELF: u32 = 0b01 << 18;
pub const ICR_DEST_ALL: u32 = 0b10 << 18;
pub const ICR_DEST_OTHER: u32 = 0b11 << 18;

pub const ICR_LEVEL_ASSERT: u32 = 1 << 14;
pub const ICR_TRIGGER_LEVEL: u32 = 1 << 15;

pub const ICR_PENDING: u32 = 1 << 12; // Delivery Status

// ── Operating Mode ────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ApicMode {
    XApic,
    X2Apic,
}

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LapicTimerOwner {
    None = 0,
    BootWatchdog = 1,
    SchedulerTick = 2,
    ExoNmiWatchdog = 3,
}

impl LapicTimerOwner {
    #[inline]
    fn from_raw(raw: u8) -> Self {
        match raw {
            x if x == Self::BootWatchdog as u8 => Self::BootWatchdog,
            x if x == Self::SchedulerTick as u8 => Self::SchedulerTick,
            x if x == Self::ExoNmiWatchdog as u8 => Self::ExoNmiWatchdog,
            _ => Self::None,
        }
    }
}

// ── MMIO Read / Write ─────────────────────────────────────────────────────────

/// Reads a LAPIC register (xAPIC MMIO)
#[inline(always)]
pub fn lapic_read(reg: u32) -> u32 {
    let base = LAPIC_MMIO_BASE.load(Ordering::Relaxed);
    // SAFETY: LAPIC base validated during init, register is a known offset
    unsafe { read_volatile((base + reg as usize) as *const u32) }
}

/// Writes a LAPIC register (xAPIC MMIO)
#[inline(always)]
pub fn lapic_write(reg: u32, val: u32) {
    let base = LAPIC_MMIO_BASE.load(Ordering::Relaxed);
    // SAFETY: LAPIC base validated during init, write to known register
    unsafe {
        write_volatile((base + reg as usize) as *mut u32, val);
    }
}

// ── MSR IA32_APICBASE ─────────────────────────────────────────────────────────

const MSR_IA32_APICBASE: u32 = 0x001B;
#[allow(dead_code)]
const APICBASE_BSP: u64 = 1 << 8;
const APICBASE_EXTD: u64 = 1 << 10; // x2APIC
const APICBASE_ENABLE: u64 = 1 << 11;
const APICBASE_ADDR_MASK: u64 = 0xFFFF_FFFF_F000;

// ── Initialization ───────────────────────────────────────────────────────────

/// Enables LAPIC in xAPIC mode (MMIO)
pub fn enable_xapic() {
    // 1. Read APICBASE to confirm address and enable
    // SAFETY: MSR IA32_APICBASE is readable from Ring 0
    let apicbase = unsafe { msr::read_msr(MSR_IA32_APICBASE) };
    let lapic_addr = apicbase & APICBASE_ADDR_MASK;

    if lapic_addr != 0 {
        LAPIC_PHYS_BASE.store(lapic_addr as usize, Ordering::Release);
        if !lapic_uses_fixmap() {
            LAPIC_MMIO_BASE.store(lapic_addr as usize, Ordering::Release);
        }
    }

    // 2. Ensure APIC is enabled (bit 11) without x2APIC (bit 10)
    let new_base = (apicbase & !APICBASE_EXTD) | APICBASE_ENABLE;
    // SAFETY: APICBASE write to enable LAPIC in xAPIC mode
    unsafe {
        msr::write_msr(MSR_IA32_APICBASE, new_base);
    }

    // 3. Soft-enable via SIVR (bit 8)
    // configure_spurious() will be called separately
}

/// Configures LAPIC base (ACPI remapping possible)
pub fn set_lapic_base(phys_addr: u64) {
    LAPIC_PHYS_BASE.store(phys_addr as usize, Ordering::Release);
    if !lapic_uses_fixmap() {
        LAPIC_MMIO_BASE.store(phys_addr as usize, Ordering::Release);
    }
}

/// Remaps the LAPIC into high kernel fixmap after memory init.
///
/// IRQ/IPI paths can execute under a userspace CR3. Using the low physical
/// address 0xFEE0_0000 then depends on a low identity-map that is not an
/// architectural guarantee of the userspace model. The fixmap makes the
/// LAPIC a global kernel mapping copied into all new CR3s.
pub fn init_lapic_fixmap_post_memory() -> bool {
    use crate::arch::x86_64::memory_iface::KERNEL_FAULT_ALLOC;
    use crate::memory::core::{fixmap_slot_addr, Frame, PageFlags, PhysAddr, FIXMAP_LAPIC};
    use crate::memory::virt::address_space::kernel::KERNEL_AS;
    use crate::memory::virt::address_space::tlb;

    let phys_base = LAPIC_PHYS_BASE.load(Ordering::Acquire) as u64;
    if phys_base == 0 || KERNEL_AS.pml4_phys().as_u64() == 0 {
        return false;
    }

    let page_phys = PhysAddr::new(phys_base & !0xFFF);
    let page_off = phys_base & 0xFFF;
    let fix_virt = fixmap_slot_addr(FIXMAP_LAPIC);
    let flags = PageFlags::KERNEL_DMA | PageFlags::WRITE_THROUGH;

    if KERNEL_AS.translate(fix_virt).is_none() {
        let frame = Frame::containing(page_phys);
        // SAFETY: kernel MMIO mapping into a reserved high fixmap address.
        if unsafe { KERNEL_AS.map(fix_virt, frame, flags, &KERNEL_FAULT_ALLOC) }.is_err() {
            return false;
        }
        // SAFETY: local invalidation of the newly installed fixmap slot.
        unsafe {
            tlb::flush_single(fix_virt);
        }
    }

    LAPIC_MMIO_BASE.store(
        fix_virt.as_u64().saturating_add(page_off) as usize,
        Ordering::Release,
    );
    true
}

/// Returns current CPU's LAPIC ID
#[inline]
pub fn lapic_id() -> u32 {
    lapic_read(LAPIC_ID) >> 24
}

/// Configures spurious interrupt vector and enables LAPIC (soft-enable)
pub fn set_spurious_vector(vector: u8) {
    let svr = lapic_read(LAPIC_SIVR);
    lapic_write(
        LAPIC_SIVR,
        (svr & 0xFFFF_FF00) | SIVR_APIC_ENABLE | (vector as u32),
    );
}

// ── EOI ───────────────────────────────────────────────────────────────────────

/// Sends End Of Interrupt (EOI) to the LAPIC
///
/// MUST be called from the IRQ handler before any lengthy operation.
#[inline(always)]
pub fn eoi() {
    lapic_write(LAPIC_EOI, 0);
}

// ── LAPIC Timer ───────────────────────────────────────────────────────────────

/// Initializes the LAPIC timer in TSC-Deadline mode
///
/// LAPIC raises interrupt when TSC >= IA32_TSC_DEADLINE.
/// Explicit reload in each handler.
pub fn timer_init_tsc_deadline(vector: u8) {
    // Mode TSC-Deadline (bits 17:18 = 0b10) + unmasked
    lapic_write(LAPIC_LVT_TIMER, TIMER_MODE_TSC_DEADLINE | (vector as u32));
    // No divider or initial counter in TSC-Deadline mode
}

/// Initializes LAPIC timer in one-shot mode (fallback if TSC-Deadline unavailable)
pub fn timer_init_oneshot(vector: u8) {
    lapic_write(LAPIC_LVT_TIMER, TIMER_MODE_ONESHOT | (vector as u32));
    lapic_write(LAPIC_TIMER_DCR, 0x3); // /16 divider
    TIMER_CALIBRATED.store(false, Ordering::Release);
}

/// Programs the next TSC-Deadline deadline
#[inline]
pub fn timer_set_deadline(deadline_tsc: u64) {
    // SAFETY: WRMSR IA32_TSC_DEADLINE is safe from Ring 0 if CPUID validated support
    unsafe {
        msr::write_msr(msr::MSR_TSC_DEADLINE, deadline_tsc);
    }
}

/// Programs the one-shot timer for N µs
pub fn timer_oneshot_us(us: u64) {
    let ticks_per_us = TIMER_TICKS_PER_US.load(Ordering::Relaxed);
    if ticks_per_us == 0 {
        return;
    }
    let count = us.saturating_mul(ticks_per_us as u64);
    let count = count.min(u32::MAX as u64) as u32;
    lapic_write(LAPIC_TIMER_ICR, count);
}

#[inline]
fn has_tsc_deadline_timer() -> bool {
    super::super::cpu::features::cpu_features_or_none()
        .is_some_and(|features| features.has_tsc_deadline())
}

/// Returns true when the LAPIC timer is reserved for the scheduler tick.
#[inline]
pub fn scheduler_timer_active() -> bool {
    lapic_timer_owner() == LapicTimerOwner::SchedulerTick
}

#[inline]
pub fn lapic_timer_owner() -> LapicTimerOwner {
    LapicTimerOwner::from_raw(LAPIC_TIMER_OWNER.load(Ordering::Acquire))
}

pub fn claim_lapic_timer(owner: LapicTimerOwner) -> bool {
    let owner_raw = owner as u8;
    loop {
        let current = LAPIC_TIMER_OWNER.load(Ordering::Acquire);
        if current == owner_raw {
            return true;
        }
        if current != LapicTimerOwner::None as u8 {
            return false;
        }
        if LAPIC_TIMER_OWNER
            .compare_exchange(
                LapicTimerOwner::None as u8,
                owner_raw,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            return true;
        }
    }
}

/// Configures the LAPIC timer as the scheduler tick source.
///
/// The LAPIC provides only one local timer. After this activation, watchdogs
/// must be serviced by the scheduler tick or by another vector,
/// without rewriting LVT_TIMER.
pub fn init_scheduler_timer(vector: u8) {
    LAPIC_TIMER_OWNER.store(LapicTimerOwner::SchedulerTick as u8, Ordering::Release);
    let uses_tsc_deadline = has_tsc_deadline_timer();
    if uses_tsc_deadline {
        timer_init_tsc_deadline(vector);
    } else {
        timer_init_oneshot(vector);
    }
    SCHEDULER_TIMER_USES_TSC_DEADLINE.store(uses_tsc_deadline as u8, Ordering::Release);
    rearm_scheduler_timer_tick();
}

/// Rearms the next scheduler tick in one-shot/TSC-deadline mode.
#[inline]
pub fn rearm_scheduler_timer_tick() {
    if !scheduler_timer_active() {
        return;
    }

    if SCHEDULER_TIMER_USES_TSC_DEADLINE.load(Ordering::Acquire) != 0 {
        let hz = tsc::tsc_hz();
        if hz == 0 {
            return;
        }
        let cycles = ((SCHEDULER_TICK_US as u128) * (hz as u128) / 1_000_000u128)
            .max(1)
            .min(u64::MAX as u128) as u64;
        timer_set_deadline(tsc::read_tsc().wrapping_add(cycles));
    } else {
        timer_oneshot_us(SCHEDULER_TICK_US);
    }
}

/// Reads current LAPIC timer counter (one-shot/periodic mode)
#[inline]
pub fn timer_current_count() -> u32 {
    lapic_read(LAPIC_TIMER_CCR)
}

/// Calibrates LAPIC timer via TSC (called after TSC init)
///
/// Measures LAPIC ticks / µs by running for ~1ms.
pub fn calibrate_lapic_timer() {
    if TIMER_CALIBRATED.load(Ordering::Acquire) {
        return;
    }

    // 1. Configure timer in one-shot with max count
    lapic_write(LAPIC_LVT_TIMER, TIMER_MODE_ONESHOT | 0xFF); // vector 0xFF (masked)
    lapic_write(LAPIC_TIMER_DCR, 0x3); // /16
    lapic_write(LAPIC_TIMER_ICR, 0xFFFF_FFFF);

    // 2. Wait 1ms via TSC
    super::super::cpu::tsc::tsc_delay_ms(1);

    // 3. Read count
    let remaining = lapic_read(LAPIC_TIMER_CCR);
    let elapsed_ticks = 0xFFFF_FFFFu32.saturating_sub(remaining);
    // ticks_per_ms = elapsed_ticks; ticks_per_us = elapsed_ticks / 1000
    let ticks_per_us = elapsed_ticks / 1000;
    TIMER_TICKS_PER_US.store(ticks_per_us, Ordering::Release);
    TIMER_TICKSPER_MS.store(elapsed_ticks, Ordering::Release);
    TIMER_CALIBRATED.store(true, Ordering::Release);
}

// ── Send IPI via ICR (xAPIC) ─────────────────────────────────────────────────

/// Waits for previous IPI to be delivered (Poll Delivery Status bit 12)
#[inline]
fn icr_wait_delivery() {
    loop {
        let icr_low = lapic_read(LAPIC_ICR_LOW);
        if icr_low & ICR_PENDING == 0 {
            break;
        }
        core::hint::spin_loop();
    }
}

/// Sends an IPI to a specific APIC ID
pub fn send_ipi(dest_apic_id: u8, vector: u8, delivery_mode: u32) {
    icr_wait_delivery();
    // Write high part first (destination)
    lapic_write(LAPIC_ICR_HIGH, (dest_apic_id as u32) << 24);
    // Then low part (trigger)
    lapic_write(
        LAPIC_ICR_LOW,
        ICR_LEVEL_ASSERT | delivery_mode | (vector as u32),
    );
}

/// Broadcasts IPI to all CPUs EXCEPT self
pub fn broadcast_ipi_except_self(vector: u8) {
    icr_wait_delivery();
    lapic_write(LAPIC_ICR_HIGH, 0);
    lapic_write(
        LAPIC_ICR_LOW,
        ICR_DEST_OTHER | ICR_LEVEL_ASSERT | ICR_DM_FIXED | (vector as u32),
    );
}

/// Sends INIT IPI (for SMP AP startup)
pub fn send_init_ipi(dest_apic_id: u8) {
    icr_wait_delivery();
    lapic_write(LAPIC_ICR_HIGH, (dest_apic_id as u32) << 24);
    lapic_write(
        LAPIC_ICR_LOW,
        ICR_LEVEL_ASSERT | ICR_TRIGGER_LEVEL | ICR_DM_INIT,
    );
}

/// Sends STARTUP IPI (for SMP AP startup at address `page`)
pub fn send_startup_ipi(dest_apic_id: u8, page: u8) {
    icr_wait_delivery();
    lapic_write(LAPIC_ICR_HIGH, (dest_apic_id as u32) << 24);
    lapic_write(
        LAPIC_ICR_LOW,
        ICR_LEVEL_ASSERT | ICR_DM_STARTUP | (page as u32),
    );
}

/// Initializes complete LAPIC for current CPU
///
/// Called both by BSP and each AP.
pub fn init_local_apic() {
    enable_xapic();
    // Mask all LVT entries except what will be configured
    lapic_write(LAPIC_LVT_THERMAL, 0x0001_0000); // masked
    lapic_write(LAPIC_LVT_PERF, 0x0001_0000); // masked
    lapic_write(LAPIC_LVT_CMCI, 0x0001_0000); // masked
                                              // LINT0 / LINT1: depend on topology (ACPI MADT)
    lapic_write(LAPIC_LVT_LINT0, 0x0001_0000); // masked by default
    lapic_write(LAPIC_LVT_LINT1, 0x0000_0400); // NMI unmasked
    lapic_write(LAPIC_LVT_ERROR, 0x0000_00FE); // error vector 0xFE
                                               // Clear ESR (write 0 first for some chipsets)
    lapic_write(LAPIC_ESR, 0);
    lapic_write(LAPIC_ESR, 0);
    // Task Priority: accept all interrupts
    lapic_write(LAPIC_TPR, 0);
}

// ── Instrumentation ───────────────────────────────────────────────────────────

static TIMER_TICKS_PER_US: AtomicU32 = AtomicU32::new(0);
static TIMER_TICKSPER_MS: AtomicU32 = AtomicU32::new(0);
static TIMER_CALIBRATED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// LAPIC timer frequency in ticks/µs (after calibration)
pub fn lapic_timer_freq_us() -> u32 {
    TIMER_TICKS_PER_US.load(Ordering::Relaxed)
}
pub fn lapic_timer_freq_ms() -> u32 {
    TIMER_TICKSPER_MS.load(Ordering::Relaxed)
}
pub fn lapic_timer_calibrated() -> bool {
    TIMER_CALIBRATED.load(Ordering::Relaxed)
}
