//! # arch/x86_64/acpi/hpet.rs — High Precision Event Timer
//!
//! HPET implementation conforming to the ACPI HPET table.
//! The HPET is used as a reference time source pending
//! TSC calibration, or as a fallback if TSC is not invariant.
//!
//! ## HPET Registers (MMIO)
//! - 0x000: GCAP_ID — capabilities and frequency (10 ns = 100 MHz if periodic)
//! - 0x010: GEN_CFG — global configuration (bit 0 = ENABLE_CNF, bit 1 = LEG_RT_CNF)
//! - 0x020: GINTR_STA — interrupt status
//! - 0x0F0: MAIN_CTR — 64-bit main counter
//! - 0x100+N*0x20: Timer N comparators

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::arch::x86_64::memory_iface::KERNEL_FAULT_ALLOC;
use crate::memory::core::{fixmap_slot_addr, Frame, PageFlags, PhysAddr, FIXMAP_HPET};
use crate::memory::virt::address_space::kernel::KERNEL_AS;
use crate::memory::virt::address_space::tlb;

// ── HPET register offsets ─────────────────────────────────────────────────────

#[allow(dead_code)]
const HPET_GCAP_ID: u64 = 0x000; // capabilities (64-bit)
const HPET_GEN_CFG: u64 = 0x010; // global config (64-bit)
#[allow(dead_code)]
const HPET_GINTR_STA: u64 = 0x020; // interrupt status
const HPET_MAIN_CTR: u64 = 0x0F0; // main counter (64-bit)
#[allow(dead_code)]
const HPET_T0_CFG: u64 = 0x100; // Timer 0 config (64-bit)
#[allow(dead_code)]
const HPET_T0_CMP: u64 = 0x108; // Timer 0 comparator (64-bit)

// GEN_CFG bits
const HPET_ENABLE: u64 = 1 << 0;
#[allow(dead_code)]
const HPET_LEG_RT: u64 = 1 << 1; // Legacy Replacement Mapping

// GCAP_ID fields
const HPET_CLK_PERIOD_SHIFT: u32 = 32; // bits 63:32 = period in femtoseconds
#[allow(dead_code)]
const HPET_NUM_TIMERS_SHIFT: u32 = 8; // bits 12:8 = number of timers - 1

// ── HPET State ────────────────────────────────────────────────────────────────

/// HPET Information
#[derive(Debug, Clone, Copy)]
pub struct HpetInfo {
    pub mmio_base: u64,       // Physical MMIO address
    pub clock_period_fs: u32, // Clock period in femtoseconds
    pub freq_hz: u64,         // Frequency in Hz
    pub n_timers: u8,         // Number of timers (at least 3)
    pub is_64bit: bool,       // 64-bit main counter
}

static HPET_BASE: AtomicU64 = AtomicU64::new(0);
static HPET_PERIOD_FS: AtomicU32 = AtomicU32::new(0);
static HPET_FREQ_HZ: AtomicU64 = AtomicU64::new(0);

// ── ACPI HPET table structures ───────────────────────────────────────────────

/// ACPI HPET table (after SdtHeader)
#[repr(C, packed)]
#[allow(dead_code)]
struct AcpiHpetTable {
    event_timer_block_id: u32,
    base_address: [u8; 12], // Generic Address Structure (GAS)
    hpet_number: u8,
    minimum_clock_tick: u16,
    page_protection: u8,
}

// ── HPET MMIO Read / Write ───────────────────────────────────────────────────

/// Reads a 64-bit HPET register
#[inline]
fn hpet_read(offset: u64) -> u64 {
    let base = HPET_BASE.load(Ordering::Relaxed);
    // SAFETY: HPET base validated during init, register offset known
    unsafe { read_volatile((base + offset) as *const u64) }
}

/// Writes a 64-bit HPET register
#[inline]
#[allow(dead_code)]
fn hpet_write(offset: u64, val: u64) {
    let base = HPET_BASE.load(Ordering::Relaxed);
    // SAFETY: HPET base validated during init
    unsafe {
        write_volatile((base + offset) as *mut u64, val);
    }
}

// ── Initialization ───────────────────────────────────────────────────────────

/// Initializes the HPET from the ACPI table
///
/// Called by `init_acpi()` after locating the HPET table.
/// During early boot, we only register the MMIO address without
/// accessing registers (HPET MMIO at 0xFED00000 exceeds our
/// 1 GiB identity map — MMIO access will occur after memory init).
pub fn init_hpet(hpet_table_phys: u64) -> Option<HpetInfo> {
    use super::parser::{acpi_read_unaligned, SdtHeader};
    let header = acpi_read_unaligned::<SdtHeader>(hpet_table_phys)?;
    let sig = header.signature;
    if &sig != b"HPET" {
        return None;
    }

    // GAS base address: offset SdtHeader(36) + block_id(4) + GAS header(4) = 44
    // In GAS (Generic Address Structure) the 64-bit address is in the last 8 bytes
    let gas_addr_offset = core::mem::size_of::<SdtHeader>() + 4 + 4;
    // read_unaligned: HPET table may be at an address not 8-byte aligned
    let mmio_base = acpi_read_unaligned::<u64>(hpet_table_phys + gas_addr_offset as u64)?;

    // Register HPET MMIO base for future use (after memory init)
    // Do NOT access HPET registers here: 0xFED00000 > 1 GiB (outside boot identity map)
    HPET_BASE.store(mmio_base, Ordering::Release);

    // Return minimal info — frequency and n_timers will be read later
    Some(HpetInfo {
        mmio_base,
        clock_period_fs: 0, // populated during post-memory init
        freq_hz: 0,
        n_timers: 0,
        is_64bit: false,
    })
}

// ── Primitives ────────────────────────────────────────────────────────────────

/// Reads the main HPET counter (64-bit)
#[inline]
pub fn hpet_read_counter() -> u64 {
    hpet_read(HPET_MAIN_CTR)
}

/// Converts µs to HPET ticks
pub fn hpet_us_to_ticks(us: u64) -> u64 {
    let freq = HPET_FREQ_HZ.load(Ordering::Relaxed);
    if freq == 0 {
        return 0;
    }
    us.saturating_mul(freq) / 1_000_000
}

/// Busy-wait delay via HPET (in µs)
///
/// Usage: calibration in early boot phases before TSC.
pub fn hpet_delay_us(us: u64) {
    let start = hpet_read_counter();
    let ticks = hpet_us_to_ticks(us);
    while hpet_read_counter().wrapping_sub(start) < ticks {
        core::hint::spin_loop();
    }
}

/// HPET frequency in Hz
pub fn hpet_freq_hz() -> u64 {
    HPET_FREQ_HZ.load(Ordering::Relaxed)
}

/// `true` if HPET is available and initialized
pub fn hpet_available() -> bool {
    HPET_BASE.load(Ordering::Relaxed) != 0 && HPET_FREQ_HZ.load(Ordering::Relaxed) != 0
}

// ── Post-memory initialization ───────────────────────────────────────────────

/// Initializes the HPET after the memory subsystem is operational.
///
/// Actions:
/// 1. Maps HPET MMIO page with `PAGE_FLAGS_MMIO` (UC + NX) in fixmap
///    **if physmap allows**, otherwise uses physical address directly
///    (0–4 GiB identity map from trampoline — without UC, functional on QEMU).
/// 2. Reads GCAP_ID → extracts clock period in femtoseconds.
/// 3. Enables main counter (HPET_ENABLE in GEN_CFG).
/// 4. Verifies that the counter advances (with safety timeout).
/// 5. Stores frequency in `HPET_FREQ_HZ`.
///
/// Called from `kernel_init()` after `hybrid::init()`.
/// No-op if HPET was not detected during `init_hpet()`.
///
/// Returns `true` if HPET is now operational.
pub fn init_hpet_post_memory() -> bool {
    let hpet_phys = HPET_BASE.load(Ordering::Acquire);
    if hpet_phys == 0 {
        return false;
    }

    // ── 1. Fixmap MMIO mapping ────────────────────────────────────────────────
    let hpet_page_phys = PhysAddr::new(hpet_phys & !0xFFF);
    let hpet_page_off = hpet_phys & 0xFFF;
    let hpet_fix_virt = fixmap_slot_addr(FIXMAP_HPET);

    // MMIO: PRESENT|WRITABLE|NX|NO_CACHE|GLOBAL
    let flags = PageFlags::PRESENT
        | PageFlags::WRITABLE
        | PageFlags::NO_EXECUTE
        | PageFlags::NO_CACHE
        | PageFlags::GLOBAL;

    if KERNEL_AS.translate(hpet_fix_virt).is_none() {
        let frame = Frame::containing(hpet_page_phys);
        // SAFETY: kernel fixmap mapping in ring0 with valid kernel allocator.
        if unsafe { KERNEL_AS.map(hpet_fix_virt, frame, flags, &KERNEL_FAULT_ALLOC) }.is_err() {
            return false;
        }
        // SAFETY: local invalidation of newly mapped fixmap entry.
        unsafe {
            tlb::flush_single(hpet_fix_virt);
        }
    }

    let virt = hpet_fix_virt.as_u64().saturating_add(hpet_page_off);
    HPET_BASE.store(virt, Ordering::Release);

    // ── 2. Read GCAP_ID via fixmap MMIO ──────────────────────────────────────
    let gcap = unsafe { core::ptr::read_volatile(virt as *const u64) };
    let period_fs = (gcap >> HPET_CLK_PERIOD_SHIFT) as u32;

    // Validate period: QEMU returns 69841279 fs (~14.318 MHz PIT reference).
    // Acceptable range: 100 ps (10 GHz) to 100 ns (10 MHz).
    if !(100..=100_000_000).contains(&period_fs) {
        return false; // Invalid GCAP_ID — HPET not available or MMIO inaccessible
    }

    let freq = 1_000_000_000_000_000u64 / period_fs as u64;
    HPET_PERIOD_FS.store(period_fs, Ordering::Release);
    HPET_FREQ_HZ.store(freq, Ordering::Release);

    // ── 3. Enable HPET counter ────────────────────────────────────────────────
    // Disable → reset counter → enable.
    unsafe {
        core::ptr::write_volatile((virt + HPET_GEN_CFG) as *mut u64, 0);
        core::ptr::write_volatile((virt + HPET_MAIN_CTR) as *mut u64, 0);
        core::ptr::write_volatile((virt + HPET_GEN_CFG) as *mut u64, HPET_ENABLE);
    }

    // ── 4. Verify counter advances (iteration limit, not TSC) ──────────────────
    // On QEMU/TCG, each read_volatile MMIO may take ~50µs.
    // 100 iterations = 5ms max on QEMU, a few µs on bare-metal.
    let counter_start = unsafe { core::ptr::read_volatile((virt + HPET_MAIN_CTR) as *const u64) };
    let mut ok = false;
    for _ in 0u32..100 {
        let c = unsafe { core::ptr::read_volatile((virt + HPET_MAIN_CTR) as *const u64) };
        if c != counter_start {
            ok = true;
            break;
        }
        core::hint::spin_loop();
    }

    if !ok {
        // HPET not counting — disable and signal failure
        unsafe {
            core::ptr::write_volatile((virt + HPET_GEN_CFG) as *mut u64, 0);
        }
        HPET_FREQ_HZ.store(0, Ordering::Release);
        return false;
    }

    true
}

/// Returns current HPET MMIO virtual address (= physical address via identity map).
pub fn hpet_virt_base() -> u64 {
    HPET_BASE.load(Ordering::Relaxed)
}
