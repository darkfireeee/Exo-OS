// src/arch/x86_64/timer.rs
// Programmation du PIT (Programmable Interval Timer) canal 0

use x86_64::instructions::port::Port;

const PIT_CH0: u16 = 0x40;
const PIT_CMD: u16 = 0x43;

/// Source de fréquence du PIT
const PIT_BASE_FREQUENCY: u32 = 1_193_182; // ~1.193182 MHz

/// Initialise le PIT en mode périodique (mode 3) à la fréquence donnée en Hz
pub fn init_pit(hz: u32) {
    let hz = hz.max(19).min(10_000); // bornes raisonnables
    let divisor: u16 = (PIT_BASE_FREQUENCY / hz) as u16;

    unsafe {
        // Mode 3 (square wave), accès lobyte/hibyte, canal 0
        Port::<u8>::new(PIT_CMD).write(0x36);
        // Charger le diviseur (low byte puis high byte)
        Port::<u8>::new(PIT_CH0).write((divisor & 0xFF) as u8);
        Port::<u8>::new(PIT_CH0).write((divisor >> 8) as u8);
    }
}
