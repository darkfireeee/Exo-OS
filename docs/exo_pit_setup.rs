// kernel/src/arch/x86_64/time/pit.rs
//
// PROGRAMMABLE INTERVAL TIMER (8253/8254)
// Timer système pour Exo-OS

use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};

/// Ports I/O du PIT
const PIT_CHANNEL_0: u16 = 0x40;  // Channel 0 (IRQ 0)
const PIT_CHANNEL_1: u16 = 0x41;  // Channel 1 (RAM refresh, obsolète)
const PIT_CHANNEL_2: u16 = 0x42;  // Channel 2 (PC Speaker)
const PIT_COMMAND: u16 = 0x43;    // Command register

/// Fréquence de base du PIT (1.193182 MHz)
const PIT_BASE_FREQ: u32 = 1193182;

/// Mode du PIT
const PIT_MODE_RATE_GENERATOR: u8 = 0x34;  // Channel 0, lobyte/hibyte, mode 2

/// Compteur de ticks global (atomique pour SMP futur)
static TICKS: AtomicU64 = AtomicU64::new(0);

/// Structure de gestion du PIT
pub struct Pit {
    frequency: u32,     // Fréquence cible en Hz (100 = 10ms, 1000 = 1ms)
    tick_duration_ns: u64, // Durée d'un tick en nanosecondes
}

impl Pit {
    /// Crée une nouvelle instance du PIT
    pub const fn new(frequency: u32) -> Self {
        let tick_duration_ns = 1_000_000_000 / frequency as u64;
        Pit {
            frequency,
            tick_duration_ns,
        }
    }

    /// Initialise le PIT avec la fréquence désirée
    pub unsafe fn init(&self) {
        // Calculer le diviseur
        let divisor = (PIT_BASE_FREQ / self.frequency) as u16;

        // Envoyer la commande (Channel 0, lobyte/hibyte, mode 2, binaire)
        outb(PIT_COMMAND, PIT_MODE_RATE_GENERATOR);

        // Envoyer le diviseur (low byte puis high byte)
        outb(PIT_CHANNEL_0, (divisor & 0xFF) as u8);
        io_wait();
        outb(PIT_CHANNEL_0, ((divisor >> 8) & 0xFF) as u8);
        io_wait();
    }

    /// Retourne la fréquence configurée
    pub fn frequency(&self) -> u32 {
        self.frequency
    }

    /// Retourne la durée d'un tick en nanosecondes
    pub fn tick_duration_ns(&self) -> u64 {
        self.tick_duration_ns
    }
}

// ============================================================================
// GESTION DES TICKS
// ============================================================================

/// Incrémente le compteur de ticks (appelé par l'IRQ 0 handler)
pub fn tick() {
    TICKS.fetch_add(1, Ordering::Relaxed);
}

/// Retourne le nombre de ticks depuis le boot
pub fn get_ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Retourne le temps écoulé en millisecondes depuis le boot
pub fn get_uptime_ms() -> u64 {
    let ticks = get_ticks();
    let pit = &PIT;
    (ticks * pit.tick_duration_ns) / 1_000_000
}

/// Retourne le temps écoulé en secondes depuis le boot
pub fn get_uptime_seconds() -> u64 {
    get_uptime_ms() / 1000
}

/// Sleep pour un nombre de ticks (busy-wait, à améliorer avec scheduler)
pub fn sleep_ticks(ticks: u64) {
    let start = get_ticks();
    while get_ticks() - start < ticks {
        unsafe {
            asm!("pause", options(nomem, nostack, preserves_flags));
        }
    }
}

/// Sleep pour un nombre de millisecondes (busy-wait)
pub fn sleep_ms(ms: u64) {
    let pit = &PIT;
    let ticks_needed = (ms * 1_000_000) / pit.tick_duration_ns;
    sleep_ticks(ticks_needed);
}

// ============================================================================
// FONCTIONS I/O BAS NIVEAU
// ============================================================================

#[inline(always)]
unsafe fn outb(port: u16, value: u8) {
    asm!(
        "out dx, al",
        in("dx") port,
        in("al") value,
        options(nomem, nostack, preserves_flags)
    );
}

#[inline(always)]
unsafe fn io_wait() {
    outb(0x80, 0);
}

// ============================================================================
// INSTANCE GLOBALE
// ============================================================================

static PIT: Pit = Pit::new(1000);  // 1000 Hz = 1 tick par milliseconde

/// Initialise le PIT (à appeler depuis kernel_main)
pub fn init_pit() {
    unsafe {
        PIT.init();
    }
    
    serial_println!("[PIT] Initialized at {} Hz", PIT.frequency());
    serial_println!("[PIT] Tick duration: {} ns", PIT.tick_duration_ns());
}

// ============================================================================
// HELPER POUR SERIAL OUTPUT
// ============================================================================

macro_rules! serial_println {
    ($($arg:tt)*) => {
        // TODO: Implémenter selon votre serial driver
    };
}
