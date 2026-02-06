//! Signal handlers pour daemon
//!
//! Gère les signaux POSIX pour contrôle du daemon:
//! - SIGHUP: Reload configuration
//! - SIGTERM/SIGINT: Shutdown gracieux
//! - SIGUSR1: Dump statistics
//! - SIGUSR2: Toggle verbose mode

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Signaux supportés
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Signal {
    /// Hang up (reload config)
    SIGHUP = 1,
    /// Interrupt (shutdown)
    SIGINT = 2,
    /// Terminate (shutdown)
    SIGTERM = 15,
    /// User signal 1 (dump stats)
    SIGUSR1 = 10,
    /// User signal 2 (toggle verbose)
    SIGUSR2 = 12,
}

impl Signal {
    /// Convertit depuis u32
    pub fn from_u32(value: u32) -> Option<Self> {
        match value {
            1 => Some(Self::SIGHUP),
            2 => Some(Self::SIGINT),
            15 => Some(Self::SIGTERM),
            10 => Some(Self::SIGUSR1),
            12 => Some(Self::SIGUSR2),
            _ => None,
        }
    }
}

/// Flags globaux pour gestion des signaux
pub struct SignalFlags {
    /// Flag de shutdown
    pub shutdown: AtomicBool,

    /// Flag de reload config
    pub reload_config: AtomicBool,

    /// Flag de dump stats
    pub dump_stats: AtomicBool,

    /// Flag de toggle verbose
    pub toggle_verbose: AtomicBool,

    /// Dernier signal reçu
    pub last_signal: AtomicU32,
}

impl SignalFlags {
    /// Crée de nouveaux flags
    pub const fn new() -> Self {
        Self {
            shutdown: AtomicBool::new(false),
            reload_config: AtomicBool::new(false),
            dump_stats: AtomicBool::new(false),
            toggle_verbose: AtomicBool::new(false),
            last_signal: AtomicU32::new(0),
        }
    }

    /// Traite un signal reçu
    pub fn handle_signal(&self, signal: Signal) {
        self.last_signal.store(signal as u32, Ordering::Release);

        match signal {
            Signal::SIGHUP => {
                self.reload_config.store(true, Ordering::Release);
            }
            Signal::SIGINT | Signal::SIGTERM => {
                self.shutdown.store(true, Ordering::Release);
            }
            Signal::SIGUSR1 => {
                self.dump_stats.store(true, Ordering::Release);
            }
            Signal::SIGUSR2 => {
                self.toggle_verbose.store(true, Ordering::Release);
            }
        }
    }

    /// Vérifie et consomme le flag shutdown
    pub fn should_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::Acquire)
    }

    /// Vérifie et consomme le flag reload_config
    pub fn should_reload_config(&self) -> bool {
        self.reload_config.swap(false, Ordering::AcqRel)
    }

    /// Vérifie et consomme le flag dump_stats
    pub fn should_dump_stats(&self) -> bool {
        self.dump_stats.swap(false, Ordering::AcqRel)
    }

    /// Vérifie et consomme le flag toggle_verbose
    pub fn should_toggle_verbose(&self) -> bool {
        self.toggle_verbose.swap(false, Ordering::AcqRel)
    }

    /// Reset tous les flags
    pub fn reset(&self) {
        self.shutdown.store(false, Ordering::Release);
        self.reload_config.store(false, Ordering::Release);
        self.dump_stats.store(false, Ordering::Release);
        self.toggle_verbose.store(false, Ordering::Release);
        self.last_signal.store(0, Ordering::Release);
    }
}

/// Handler global de signaux (singleton)
static SIGNAL_FLAGS: SignalFlags = SignalFlags::new();

/// Retourne une référence aux flags de signaux
pub fn signal_flags() -> &'static SignalFlags {
    &SIGNAL_FLAGS
}

/// Enregistre les handlers de signaux
///
/// Dans Exo-OS, cela utiliserait syscall signal() ou sigaction()
/// Pour no_std, on simule avec un registry global
pub fn register_signal_handlers() -> Result<(), &'static str> {
    // Dans un vrai système:
    // syscall::signal(Signal::SIGHUP as i32, signal_handler_wrapper)?;
    // syscall::signal(Signal::SIGINT as i32, signal_handler_wrapper)?;
    // syscall::signal(Signal::SIGTERM as i32, signal_handler_wrapper)?;
    // syscall::signal(Signal::SIGUSR1 as i32, signal_handler_wrapper)?;
    // syscall::signal(Signal::SIGUSR2 as i32, signal_handler_wrapper)?;

    Ok(())
}

/// Wrapper pour handler de signal (appelé par l'OS)
///
/// # Safety
/// Cette fonction est appelée dans un contexte de signal handler,
/// donc uniquement des opérations async-signal-safe sont permises
#[no_mangle]
pub extern "C" fn signal_handler_wrapper(signum: i32) {
    if let Some(signal) = Signal::from_u32(signum as u32) {
        SIGNAL_FLAGS.handle_signal(signal);
    }
}

/// Simulation de réception de signal (pour tests)
pub fn simulate_signal(signal: Signal) {
    SIGNAL_FLAGS.handle_signal(signal);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signal_flags() {
        let flags = SignalFlags::new();

        assert!(!flags.should_shutdown());
        assert!(!flags.should_reload_config());

        flags.handle_signal(Signal::SIGHUP);
        assert!(flags.should_reload_config());
        assert!(!flags.should_reload_config()); // Consumed

        flags.handle_signal(Signal::SIGTERM);
        assert!(flags.should_shutdown());
    }

    #[test]
    fn test_signal_conversion() {
        assert_eq!(Signal::from_u32(1), Some(Signal::SIGHUP));
        assert_eq!(Signal::from_u32(2), Some(Signal::SIGINT));
        assert_eq!(Signal::from_u32(15), Some(Signal::SIGTERM));
        assert_eq!(Signal::from_u32(99), None);
    }

    #[test]
    fn test_global_signal_handler() {
        signal_flags().reset();

        simulate_signal(Signal::SIGUSR1);
        assert!(signal_flags().should_dump_stats());

        simulate_signal(Signal::SIGUSR2);
        assert!(signal_flags().should_toggle_verbose());
    }
}
