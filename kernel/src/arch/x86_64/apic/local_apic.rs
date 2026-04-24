//! # arch/x86_64/apic/local_apic.rs — Local APIC (xAPIC MMIO)
//!
//! Implémente l'accès au Local APIC via MMIO (mode xAPIC, base 0xFEE00000)
//! et les primitives partagées avec x2APIC (via MSR).
//!
//! ## Registres LAPIC (offset depuis LAPIC_BASE)
//! Les registres sont tous 32 bits, alignés sur 16 octets.

use super::super::cpu::msr;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

// ── Adresse MMIO LAPIC ────────────────────────────────────────────────────────

/// Adresse MMIO par défaut du Local APIC (peut être relue depuis MSR_IA32_APICBASE)
pub const LAPIC_DEFAULT_BASE: u64 = 0xFEE0_0000;

/// Base LAPIC actuelle (peut différer après remapping ACPI)
static LAPIC_BASE: AtomicUsize = AtomicUsize::new(LAPIC_DEFAULT_BASE as usize);

// ── Offsets des registres LAPIC ───────────────────────────────────────────────

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
pub const LAPIC_ICR_LOW: u32 = 0x300; // Interrupt Command Register (faible)
pub const LAPIC_ICR_HIGH: u32 = 0x310; // Interrupt Command Register (haut)
pub const LAPIC_LVT_TIMER: u32 = 0x320; // LVT Timer
pub const LAPIC_LVT_THERMAL: u32 = 0x330; // LVT Thermal Sensor
pub const LAPIC_LVT_PERF: u32 = 0x340; // LVT Performance Counter
pub const LAPIC_LVT_LINT0: u32 = 0x350; // LVT Local Interrupt 0
pub const LAPIC_LVT_LINT1: u32 = 0x360; // LVT Local Interrupt 1
pub const LAPIC_LVT_ERROR: u32 = 0x370; // LVT Error
pub const LAPIC_TIMER_ICR: u32 = 0x380; // Timer Initial Count
pub const LAPIC_TIMER_CCR: u32 = 0x390; // Timer Current Count
pub const LAPIC_TIMER_DCR: u32 = 0x3E0; // Timer Divide Configuration

// ── Bits du registre SIVR ─────────────────────────────────────────────────────

pub const SIVR_APIC_ENABLE: u32 = 1 << 8;
pub const SIVR_FOCUS_DISABLE: u32 = 1 << 9;

// ── Timer modes (bits 17:18 du LVT_TIMER) ────────────────────────────────────

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

// ── Mode de fonctionnement ────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ApicMode {
    XApic,
    X2Apic,
}

// ── Lecture / écriture MMIO ───────────────────────────────────────────────────

/// Lit un registre LAPIC (xAPIC MMIO)
#[inline(always)]
pub fn lapic_read(reg: u32) -> u32 {
    let base = LAPIC_BASE.load(Ordering::Relaxed);
    // SAFETY: base LAPIC validée lors de l'init, registre est un offset connu
    unsafe { read_volatile((base + reg as usize) as *const u32) }
}

/// Écrit un registre LAPIC (xAPIC MMIO)
#[inline(always)]
pub fn lapic_write(reg: u32, val: u32) {
    let base = LAPIC_BASE.load(Ordering::Relaxed);
    // SAFETY: base LAPIC validée lors de l'init, écriture sur registre connu
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

// ── Initialisation ────────────────────────────────────────────────────────────

/// Active le LAPIC en mode xAPIC (MMIO)
pub fn enable_xapic() {
    // 1. Lire APICBASE pour confirmer l'adresse et activer
    // SAFETY: MSR IA32_APICBASE est lisible depuis Ring 0
    let apicbase = unsafe { msr::read_msr(MSR_IA32_APICBASE) };
    let lapic_addr = apicbase & APICBASE_ADDR_MASK;

    if lapic_addr != 0 {
        LAPIC_BASE.store(lapic_addr as usize, Ordering::Release);
    }

    // 2. Assurer que APIC est activé (bit 11) sans x2APIC (bit 10)
    let new_base = (apicbase & !APICBASE_EXTD) | APICBASE_ENABLE;
    // SAFETY: écriture APICBASE pour activer le LAPIC en mode xAPIC
    unsafe {
        msr::write_msr(MSR_IA32_APICBASE, new_base);
    }

    // 3. Soft-enable via SIVR (bit 8)
    // configure_spurious() sera appelé séparément
}

/// Configure la base LAPIC (remapping ACPI possible)
pub fn set_lapic_base(phys_addr: u64) {
    LAPIC_BASE.store(phys_addr as usize, Ordering::Release);
}

/// Retourne l'ID LAPIC du CPU courant
#[inline]
pub fn lapic_id() -> u32 {
    lapic_read(LAPIC_ID) >> 24
}

/// Configure le vecteur d'interruption spurious et active le LAPIC (soft-enable)
pub fn set_spurious_vector(vector: u8) {
    let svr = lapic_read(LAPIC_SIVR);
    lapic_write(
        LAPIC_SIVR,
        (svr & 0xFFFF_FF00) | SIVR_APIC_ENABLE | (vector as u32),
    );
}

// ── EOI ───────────────────────────────────────────────────────────────────────

/// Envoie l'End Of Interrupt (EOI) au LAPIC
///
/// DOIT être appelé depuis le handler IRQ avant toute opération longue.
#[inline(always)]
pub fn eoi() {
    lapic_write(LAPIC_EOI, 0);
}

// ── Timer LAPIC ───────────────────────────────────────────────────────────────

/// Initialise le timer LAPIC en mode TSC-Deadline
///
/// LAPIC lève l'interruption quand TSC >= IA32_TSC_DEADLINE.
/// Rechargement explicite à chaque handler.
pub fn timer_init_tsc_deadline(vector: u8) {
    // Mode TSC-Deadline (bits 17:18 = 0b10) + unmasked
    lapic_write(LAPIC_LVT_TIMER, TIMER_MODE_TSC_DEADLINE | (vector as u32));
    // Pas de diviseur ni de compteur initial en mode TSC-Deadline
}

/// Initialise le timer LAPIC en mode one-shot (fallback si TSC-Deadline indisponible)
pub fn timer_init_oneshot(vector: u8) {
    lapic_write(LAPIC_LVT_TIMER, TIMER_MODE_ONESHOT | (vector as u32));
    lapic_write(LAPIC_TIMER_DCR, 0x3); // Diviseur /16
    TIMER_CALIBRATED.store(false, Ordering::Release);
}

/// Programme la prochaine deadline TSC-Deadline
#[inline]
pub fn timer_set_deadline(deadline_tsc: u64) {
    // SAFETY: WRMSR IA32_TSC_DEADLINE est sûr depuis Ring 0 si CPUID a validé le support
    unsafe {
        msr::write_msr(msr::MSR_TSC_DEADLINE, deadline_tsc);
    }
}

/// Programme le timer one-shot pour N µs
pub fn timer_oneshot_us(us: u64) {
    let ticks_per_us = TIMER_TICKS_PER_US.load(Ordering::Relaxed);
    if ticks_per_us == 0 {
        return;
    }
    let count = us.saturating_mul(ticks_per_us as u64);
    let count = count.min(u32::MAX as u64) as u32;
    lapic_write(LAPIC_TIMER_ICR, count);
}

/// Lit le compteur courant du timer LAPIC (mode one-shot/periodic)
#[inline]
pub fn timer_current_count() -> u32 {
    lapic_read(LAPIC_TIMER_CCR)
}

/// Calibration du timer LAPIC via le TSC (appelé après init TSC)
///
/// Mesure le nombre de ticks LAPIC / µs en laissant tourner ~1ms.
pub fn calibrate_lapic_timer() {
    if TIMER_CALIBRATED.load(Ordering::Acquire) {
        return;
    }

    // 1. Configurer timer en one-shot avec comput max
    lapic_write(LAPIC_LVT_TIMER, TIMER_MODE_ONESHOT | 0xFF); // vecteur 0xFF (masqué)
    lapic_write(LAPIC_TIMER_DCR, 0x3); // /16
    lapic_write(LAPIC_TIMER_ICR, 0xFFFF_FFFF);

    // 2. Attendre 1ms via TSC
    super::super::cpu::tsc::tsc_delay_ms(1);

    // 3. Lire le décompte
    let remaining = lapic_read(LAPIC_TIMER_CCR);
    let elapsed_ticks = 0xFFFF_FFFFu32.saturating_sub(remaining);
    // ticks_per_ms = elapsed_ticks ; ticks_per_us = elapsed_ticks / 1000
    let ticks_per_us = elapsed_ticks / 1000;
    TIMER_TICKS_PER_US.store(ticks_per_us, Ordering::Release);
    TIMER_TICKSPER_MS.store(elapsed_ticks, Ordering::Release);
    TIMER_CALIBRATED.store(true, Ordering::Release);
}

// ── Envoi IPI via ICR (xAPIC) ─────────────────────────────────────────────────

/// Attend que le Previous IPI soit livré (Poll Delivery Status bit 12)
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

/// Envoie un IPI à destination d'un APIC ID spécifique
pub fn send_ipi(dest_apic_id: u8, vector: u8, delivery_mode: u32) {
    icr_wait_delivery();
    // Écrire la partie haute en premier (destination)
    lapic_write(LAPIC_ICR_HIGH, (dest_apic_id as u32) << 24);
    // Puis partie basse (déclenchement)
    lapic_write(
        LAPIC_ICR_LOW,
        ICR_LEVEL_ASSERT | delivery_mode | (vector as u32),
    );
}

/// Broadcast IPI vers tous les CPUs SAUF soi-même
pub fn broadcast_ipi_except_self(vector: u8) {
    icr_wait_delivery();
    lapic_write(LAPIC_ICR_HIGH, 0);
    lapic_write(
        LAPIC_ICR_LOW,
        ICR_DEST_OTHER | ICR_LEVEL_ASSERT | ICR_DM_FIXED | (vector as u32),
    );
}

/// Envoi INIT IPI (pour SMP AP startup)
pub fn send_init_ipi(dest_apic_id: u8) {
    icr_wait_delivery();
    lapic_write(LAPIC_ICR_HIGH, (dest_apic_id as u32) << 24);
    lapic_write(
        LAPIC_ICR_LOW,
        ICR_LEVEL_ASSERT | ICR_TRIGGER_LEVEL | ICR_DM_INIT,
    );
}

/// Envoi STARTUP IPI (pour SMP AP startup à l'adresse `page`)
pub fn send_startup_ipi(dest_apic_id: u8, page: u8) {
    icr_wait_delivery();
    lapic_write(LAPIC_ICR_HIGH, (dest_apic_id as u32) << 24);
    lapic_write(
        LAPIC_ICR_LOW,
        ICR_LEVEL_ASSERT | ICR_DM_STARTUP | (page as u32),
    );
}

/// Initialise le LAPIC complet pour le CPU courant
///
/// Appelé à la fois par le BSP et par chaque AP.
pub fn init_local_apic() {
    enable_xapic();
    // Masquer tous les LVT entries sauf ce qui sera configuré
    lapic_write(LAPIC_LVT_THERMAL, 0x0001_0000); // masqué
    lapic_write(LAPIC_LVT_PERF, 0x0001_0000); // masqué
    lapic_write(LAPIC_LVT_CMCI, 0x0001_0000); // masqué
                                              // LINT0 / LINT1 : dépendent de la topologie (ACPI MADT)
    lapic_write(LAPIC_LVT_LINT0, 0x0001_0000); // masqué par défaut
    lapic_write(LAPIC_LVT_LINT1, 0x0000_0400); // NMI non-masqué
    lapic_write(LAPIC_LVT_ERROR, 0x0000_00FE); // erreur vecteur 0xFE
                                               // Effacer ESR (écrire 0 d'abord pour certains chipsets)
    lapic_write(LAPIC_ESR, 0);
    lapic_write(LAPIC_ESR, 0);
    // Task Priority : accepter toutes les interruptions
    lapic_write(LAPIC_TPR, 0);
}

// ── Instrumentation ───────────────────────────────────────────────────────────

static TIMER_TICKS_PER_US: AtomicU32 = AtomicU32::new(0);
static TIMER_TICKSPER_MS: AtomicU32 = AtomicU32::new(0);
static TIMER_CALIBRATED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Fréquence du timer LAPIC en ticks/µs (après calibration)
pub fn lapic_timer_freq_us() -> u32 {
    TIMER_TICKS_PER_US.load(Ordering::Relaxed)
}
pub fn lapic_timer_freq_ms() -> u32 {
    TIMER_TICKSPER_MS.load(Ordering::Relaxed)
}
pub fn lapic_timer_calibrated() -> bool {
    TIMER_CALIBRATED.load(Ordering::Relaxed)
}
