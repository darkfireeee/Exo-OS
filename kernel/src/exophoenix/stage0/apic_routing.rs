//! # exophoenix/stage0/apic_routing.rs — Routage APIC, calibration timer et watchdog
//!
//! Gère la correspondance APIC ID -> slots de cœurs logiques, la calibration
//! du timer APIC via le canal 2 du PIT, l'armement du watchdog matériel, et l'envoi SIPI.

use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use crate::arch::x86_64;
use crate::arch::x86_64::acpi::madt;
use crate::arch::x86_64::apic::local_apic;
use crate::arch::x86_64::apic::{ipi, x2apic};
use crate::arch::x86_64::cpu::{msr, tsc};
use crate::arch::x86_64::smp::init::TRAMPOLINE_PAGE;
use crate::arch::x86_64::time::sources::pit;
use super::features::{BootApicMode, B_FEATURES};

pub const APIC_X2APIC_MSR_BASE: u32 = 0x800;
pub const WATCHDOG_DEFAULT_MS: u64 = 5_000;
pub const CORE_A_SLOT: u8 = 0;
pub const A_ENTRY_VECTOR: u8 = TRAMPOLINE_PAGE;
pub const SIPI_SENT_BIT: u64 = 1;

/// Table APIC ID (0..255) -> slot SSR (0..255), 0xFF = non assigné.
pub static APIC_TO_SLOT: [AtomicU8; 256] = [const { AtomicU8::new(0xFF) }; 256];

/// Étape 7 : ticks APIC par microseconde (calibré via PIT ch2).
pub static TICKS_PER_US: AtomicU64 = AtomicU64::new(0);
pub static WATCHDOG_ARMED_MS: AtomicU64 = AtomicU64::new(0);
pub static WATCHDOG_ARMED_TICKS: AtomicU64 = AtomicU64::new(0);

/// Étape 13 : garde-fou anti double SIPI (G8).
pub static SIPI_SENT: AtomicU64 = AtomicU64::new(0);

/// Étape 6 : construction apic_to_slot[256] depuis la MADT réelle.
pub fn build_apic_to_slot_from_real_madt(madt_phys: u64) -> usize {
    for slot in &APIC_TO_SLOT {
        slot.store(0xFF, Ordering::Relaxed);
    }

    if madt_phys == 0 {
        return 0;
    }

    let info = madt::parse_madt(madt_phys);
    let mut mapped = 0usize;
    for idx in 0..(info.cpu_count as usize).min(256) {
        let apic_id = info.apic_ids[idx];
        if apic_id < 256 {
            APIC_TO_SLOT[apic_id as usize].store(idx as u8, Ordering::Release);
            mapped += 1;
        }
    }

    if mapped == 0 {
        // Fallback BSP minimal si MADT inutilisable.
        let bsp_apic = match B_FEATURES.apic_mode() {
            BootApicMode::X2Apic => crate::arch::x86_64::apic::x2apic::x2apic_id(),
            BootApicMode::XApic => local_apic::lapic_id(),
        } & 0xFF;
        APIC_TO_SLOT[bsp_apic as usize].store(0, Ordering::Release);
        mapped = 1;
    }

    mapped
}

#[inline(always)]
pub fn apic_reg_to_x2apic_msr(reg: u32) -> u32 {
    APIC_X2APIC_MSR_BASE + (reg >> 4)
}

#[inline(always)]
pub fn apic_timer_write(reg: u32, val: u32) {
    match B_FEATURES.apic_mode() {
        BootApicMode::XApic => local_apic::lapic_write(reg, val),
        BootApicMode::X2Apic => {
            let msr_reg = apic_reg_to_x2apic_msr(reg);
            // SAFETY: registre x2APIC standard, ring0 bootstrap.
            unsafe {
                msr::write_msr(msr_reg, val as u64);
            }
        }
    }
}

#[inline(always)]
pub fn apic_timer_read(reg: u32) -> u32 {
    match B_FEATURES.apic_mode() {
        BootApicMode::XApic => local_apic::lapic_read(reg),
        BootApicMode::X2Apic => {
            let msr_reg = apic_reg_to_x2apic_msr(reg);
            // SAFETY: registre x2APIC standard, ring0 bootstrap.
            unsafe { msr::read_msr(msr_reg) as u32 }
        }
    }
}

/// Étape 7 : calibration APIC timer via PIT channel 2.
pub fn calibrate_apic_timer_via_pit_ch2() -> Option<u64> {
    let irq_flags = x86_64::irq_save();
    let pit_count: u16 = 11_931; // ~10ms @ 1.193182MHz

    pit::setup_ch2_oneshot(pit_count);
    // Timer APIC one-shot masqué, compteur au maximum.
    apic_timer_write(
        local_apic::LAPIC_LVT_TIMER,
        local_apic::TIMER_MODE_ONESHOT | 0x0001_0000 | 0xFF,
    );
    apic_timer_write(local_apic::LAPIC_TIMER_DCR, 0x3); // diviseur /16
    apic_timer_write(local_apic::LAPIC_TIMER_ICR, u32::MAX);

    let done = pit::wait_ch2_done();
    pit::disable_ch2();
    let remaining = apic_timer_read(local_apic::LAPIC_TIMER_CCR);

    x86_64::irq_restore(irq_flags);

    if !done {
        return None;
    }

    let elapsed = u32::MAX.saturating_sub(remaining) as u64;
    if elapsed == 0 {
        return None;
    }

    let measure_us = (pit_count as u64).saturating_mul(1_000_000) / pit::PIT_FREQ_HZ;
    if measure_us == 0 {
        return None;
    }

    let ticks_per_us = elapsed / measure_us;
    if ticks_per_us == 0 {
        None
    } else {
        Some(ticks_per_us)
    }
}

pub fn calibrate_and_store_ticks_per_us() -> u64 {
    let ticks = calibrate_apic_timer_via_pit_ch2().unwrap_or(0);
    TICKS_PER_US.store(ticks, Ordering::Release);
    ticks
}

/// Étape 8 : init APIC local avec dispatch explicite xAPIC/x2APIC via B_FEATURES.
pub fn init_local_apic_dispatch() {
    match B_FEATURES.apic_mode() {
        BootApicMode::X2Apic => {
            x2apic::mask_all_lvt_x2apic();
            local_apic::set_spurious_vector(crate::arch::x86_64::idt::VEC_SPURIOUS);
            if crate::arch::x86_64::cpu::features::cpu_features_or_none()
                .map_or(false, |features| features.has_tsc_deadline())
            {
                local_apic::timer_init_tsc_deadline(crate::arch::x86_64::idt::VEC_IRQ_TIMER);
            } else {
                local_apic::timer_init_oneshot(crate::arch::x86_64::idt::VEC_IRQ_TIMER);
            }
        }
        BootApicMode::XApic => {
            local_apic::init_local_apic();
            local_apic::set_spurious_vector(crate::arch::x86_64::idt::VEC_SPURIOUS);
            if crate::arch::x86_64::cpu::features::cpu_features_or_none()
                .map_or(false, |features| features.has_tsc_deadline())
            {
                local_apic::timer_init_tsc_deadline(crate::arch::x86_64::idt::VEC_IRQ_TIMER);
            } else {
                local_apic::timer_init_oneshot(crate::arch::x86_64::idt::VEC_IRQ_TIMER);
            }
        }
    }
}

/// Étape 12 : arme watchdog APIC (par défaut 5000ms), basé sur TICKS_PER_US.
pub fn arm_apic_watchdog(ms: u64) -> u64 {
    let ms = if ms == 0 { WATCHDOG_DEFAULT_MS } else { ms };
    let ticks_per_us = TICKS_PER_US.load(Ordering::Acquire);
    if ticks_per_us == 0 {
        WATCHDOG_ARMED_MS.store(0, Ordering::Release);
        WATCHDOG_ARMED_TICKS.store(0, Ordering::Release);
        return 0;
    }

    let watchdog_us = ms.saturating_mul(1000);
    let ticks = watchdog_us.saturating_mul(ticks_per_us);
    if !crate::arch::x86_64::apic::local_apic::claim_lapic_timer(
        crate::arch::x86_64::apic::local_apic::LapicTimerOwner::BootWatchdog,
    ) {
        WATCHDOG_ARMED_MS.store(ms, Ordering::Release);
        WATCHDOG_ARMED_TICKS.store(ticks, Ordering::Release);
        return ticks;
    }
    let initial_count = ticks.min(u32::MAX as u64) as u32;

    apic_timer_write(
        local_apic::LAPIC_LVT_TIMER,
        local_apic::TIMER_MODE_ONESHOT | crate::arch::x86_64::idt::VEC_IRQ_TIMER as u32,
    );
    apic_timer_write(local_apic::LAPIC_TIMER_DCR, 0x3);
    apic_timer_write(local_apic::LAPIC_TIMER_ICR, initial_count);

    WATCHDOG_ARMED_MS.store(ms, Ordering::Release);
    WATCHDOG_ARMED_TICKS.store(ticks, Ordering::Release);
    ticks
}

pub fn ticks_per_us() -> u64 {
    TICKS_PER_US.load(Ordering::Acquire)
}

#[inline(always)]
pub fn set_apic_slot(apic_id: u8, slot_index: u8) {
    APIC_TO_SLOT[apic_id as usize].store(slot_index, Ordering::Release);
}

#[inline(always)]
pub fn apic_slot(apic_id: u32) -> Option<usize> {
    let slot = APIC_TO_SLOT[(apic_id & 0xFF) as usize].load(Ordering::Acquire);
    if slot == 0xFF {
        None
    } else {
        Some(slot as usize)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendSipiError {
    AlreadySent,
    TargetNotFound,
}

#[inline(always)]
pub fn current_apic_id_low8() -> u8 {
    match B_FEATURES.apic_mode() {
        BootApicMode::X2Apic => (x2apic::x2apic_id() & 0xFF) as u8,
        BootApicMode::XApic => (local_apic::lapic_id() & 0xFF) as u8,
    }
}

pub fn resolve_apic_id_for_slot(slot_index: u8) -> Option<u8> {
    let current = current_apic_id_low8();
    let mut fallback = None;
    for apic_id in 0u16..=255u16 {
        let mapped = APIC_TO_SLOT[apic_id as usize].load(Ordering::Acquire);
        if mapped != slot_index {
            continue;
        }
        let found = apic_id as u8;
        if found != current {
            return Some(found);
        }
        fallback = Some(found);
    }
    fallback
}

/// Étape 13 : envoi SIPI one-shot (G8) vers le slot cible.
pub fn send_sipi_once(core_slot: u8, entry_vector: u8) -> Result<(), SendSipiError> {
    let prior = SIPI_SENT.fetch_or(SIPI_SENT_BIT, Ordering::AcqRel);
    if (prior & SIPI_SENT_BIT) != 0 {
        return Err(SendSipiError::AlreadySent);
    }

    let apic_id = resolve_apic_id_for_slot(core_slot).ok_or(SendSipiError::TargetNotFound)?;
    ipi::send_init_ipi(apic_id);
    tsc::tsc_delay_ms(10);
    ipi::send_startup_ipi(apic_id, entry_vector);
    tsc::tsc_delay_ms(1);
    ipi::send_startup_ipi(apic_id, entry_vector);
    Ok(())
}

pub fn watchdog_armed_ms() -> u64 {
    WATCHDOG_ARMED_MS.load(Ordering::Acquire)
}

pub fn watchdog_armed_ticks() -> u64 {
    WATCHDOG_ARMED_TICKS.load(Ordering::Acquire)
}
