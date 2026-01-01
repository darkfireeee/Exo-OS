//! PIT (Programmable Interval Timer) 8253/8254
//! 
//! Génère des interruptions périodiques pour le scheduler

use crate::arch::x86_64::outb;

use core::sync::atomic::{AtomicU64, Ordering};

/// Port du canal 0 du PIT
const PIT_CHANNEL0: u16 = 0x40;
/// Port de commande du PIT
const PIT_COMMAND: u16 = 0x43;

/// Fréquence de base du PIT (1.193182 MHz)
const PIT_BASE_FREQUENCY: u32 = 1193182;

/// Compteur de ticks global (atomic pour éviter problèmes d'optimisation)
static TICKS: AtomicU64 = AtomicU64::new(0);

/// Configure le PIT pour générer des interruptions à une fréquence donnée
/// 
/// # Arguments
/// * `frequency` - Fréquence désirée en Hz (ex: 100 pour 100 interruptions/sec)
pub fn init(frequency: u32) {
    unsafe {
        // Calculer le diviseur pour la fréquence désirée
        let divisor = (PIT_BASE_FREQUENCY / frequency) as u16;

        // Configurer le PIT:
        // - Canal 0 (bits 7-6 = 00)
        // - Access mode: lobyte/hibyte (bits 5-4 = 11)
        // - Mode 3: square wave generator (bits 3-1 = 011)
        // - Binary mode (bit 0 = 0)
        let command: u8 = 0b00110110;
        outb(PIT_COMMAND, command);

        // Envoyer le diviseur (low byte puis high byte)
        outb(PIT_CHANNEL0, (divisor & 0xFF) as u8);
        outb(PIT_CHANNEL0, ((divisor >> 8) & 0xFF) as u8);
    }
}

/// Incrémente le compteur de ticks (appelé par le handler d'interruption)
pub fn tick() {
    TICKS.fetch_add(1, Ordering::SeqCst);
}

/// Retourne le nombre de ticks depuis le démarrage
pub fn get_ticks() -> u64 {
    TICKS.load(Ordering::SeqCst)
}

/// Retourne le temps écoulé en millisecondes (approximatif)
/// Suppose une fréquence de 100 Hz (10ms par tick)
pub fn get_uptime_ms() -> u64 {
    TICKS.load(Ordering::SeqCst) * 10
}

/// Attend un certain nombre de millisecondes (busy wait)
pub fn sleep_ms(ms: u64) {
    log::info!("[SLEEP] sleep_ms({}) called", ms);
    
    // En mode SMP, le PIT n'est pas initialisé, on utilise RDTSC
    // Approximation: 2.4 GHz CPU → 2_400_000 cycles/ms
    const CYCLES_PER_MS: u64 = 2_400_000;
    
    let start = unsafe { core::arch::x86_64::_rdtsc() };
    let target = start + (ms * CYCLES_PER_MS);
    
    log::info!("[SLEEP] RDTSC start={}, target={}", start, target);
    
    let mut iterations = 0u64;
    while unsafe { core::arch::x86_64::_rdtsc() } < target {
        unsafe {
            core::arch::asm!("pause", options(nomem, nostack, preserves_flags));
        }
        iterations += 1;
        if iterations % 10_000_000 == 0 {
            let now = unsafe { core::arch::x86_64::_rdtsc() };
            log::info!("[SLEEP] Still waiting... now={}, target={}", now, target);
        }
    }
    
    log::info!("[SLEEP] sleep_ms({}) completed after {} iterations", ms, iterations);
}

/// Attend un certain nombre de microsecondes (busy wait)
pub fn sleep_us(us: u64) {
    // Simple busy loop with pause
    for _ in 0..(us * 100) {
        unsafe {
            core::arch::asm!("pause", options(nomem, nostack, preserves_flags));
        }
    }
}
