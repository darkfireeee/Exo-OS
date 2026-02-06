//! Registry Daemon Binary - Production deployment
//!
//! Daemon système pour le service registry centralisant la découverte de services.
//!
//! ## Usage
//!
//! ```bash
//! # Démarrage standard
//! exo_registry_daemon
//!
//! # Avec configuration custom
//! exo_registry_daemon --config /etc/exo/registry.toml
//!
//! # Mode verbose
//! exo_registry_daemon --verbose
//! ```
//!
//! ## Signals
//!
//! - SIGHUP: Reload configuration
//! - SIGTERM/SIGINT: Shutdown gracieux
//! - SIGUSR1: Dump statistics

#![no_std]
#![no_main]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use exo_service_registry::{
    Registry, RegistryConfig,
    daemon::{RegistryDaemon, DaemonConfig},
    ServiceName, ServiceInfo,
};

#[cfg(feature = "ipc")]
use exo_service_registry::ipc::IpcServer;

/// Flag global pour shutdown gracieux
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// Flag pour reload configuration
static RELOAD_CONFIG: AtomicBool = AtomicBool::new(false);

/// Configuration du daemon
#[derive(Debug, Clone)]
pub struct DaemonSettings {
    pub socket_path: String,
    pub cache_size: usize,
    pub bloom_size: usize,
    pub stale_threshold: u64,
    pub max_connections: usize,
    pub request_queue_size: usize,
    pub verbose: bool,
}

impl Default for DaemonSettings {
    fn default() -> Self {
        Self {
            socket_path: "/var/run/exo/registry.sock".into(),
            cache_size: 500,
            bloom_size: 100_000,
            stale_threshold: 300,
            max_connections: 100,
            request_queue_size: 256,
            verbose: false,
        }
    }
}

impl DaemonSettings {
    /// Charge depuis les arguments de ligne de commande
    pub fn from_args() -> Self {
        // Dans un vrai système, on parserait argv
        // Pour l'instant on utilise les valeurs par défaut
        Self::default()
    }

    /// Charge depuis un fichier TOML
    #[cfg(feature = "persistent")]
    pub fn from_file(_path: &str) -> Result<Self, &'static str> {
        // TODO: Parser le fichier TOML
        Ok(Self::default())
    }
}

/// Point d'entrée du daemon
#[no_mangle]
pub extern "C" fn main(_argc: isize, _argv: *const *const u8) -> isize {
    // 1. Charge la configuration
    let settings = DaemonSettings::from_args();

    if settings.verbose {
        log_info("Starting Exo Registry Daemon...");
    }

    // 2. Configure le registry
    let registry_config = RegistryConfig::new()
        .with_cache_size(settings.cache_size)
        .with_bloom_size(settings.bloom_size)
        .with_stale_threshold(settings.stale_threshold);

    let registry = Box::new(Registry::with_config(registry_config));

    if settings.verbose {
        log_info("Registry configured");
    }

    // 3. Crée le daemon
    let daemon_config = DaemonConfig::new()
        .with_max_connections(settings.max_connections)
        .with_queue_size(settings.request_queue_size)
        .with_verbose(settings.verbose);

    let daemon = RegistryDaemon::with_config(registry, daemon_config);

    if settings.verbose {
        log_info("Daemon created");
    }

    // 4. Configure les signal handlers
    setup_signal_handlers();

    // 5. Lance le serveur IPC (si feature activée)
    #[cfg(feature = "ipc")]
    {
        let mut server = match IpcServer::new(daemon, 64) {
            Ok(s) => s,
            Err(_) => {
                log_error("Failed to create IPC server");
                return 1;
            }
        };

        if settings.verbose {
            log_info("IPC Server initialized");
            log_info("Listening for requests...");
        }

        // Boucle principale
        while !SHUTDOWN.load(Ordering::Acquire) {
            // Check reload config
            if RELOAD_CONFIG.load(Ordering::Acquire) {
                if settings.verbose {
                    log_info("Reloading configuration...");
                }
                RELOAD_CONFIG.store(false, Ordering::Release);
                // TODO: Reload config from file
            }

            // Process one request (with timeout)
            // Dans un vrai système, on utiliserait un event loop
            // Pour l'instant on simule juste la disponibilité
        }

        if settings.verbose {
            log_info("Shutting down gracefully...");
        }

        server.shutdown();

        // Flush le registry
        if let Err(_) = server.daemon_mut().flush() {
            log_error("Failed to flush registry");
        }

        if settings.verbose {
            log_info("Registry daemon stopped");
        }
    }

    #[cfg(not(feature = "ipc"))]
    {
        log_error("IPC feature not enabled - daemon cannot run");
        return 1;
    }

    0
}

/// Configure les handlers de signaux
fn setup_signal_handlers() {
    // Dans un vrai système Exo-OS, on enregistrerait des handlers via syscall
    // Pour l'instant c'est un stub

    // SIGHUP -> reload config
    // SIGTERM/SIGINT -> shutdown
    // SIGUSR1 -> dump stats
}

/// Log une info
fn log_info(msg: &str) {
    #[cfg(feature = "std")]
    println!("[INFO] {}", msg);

    #[cfg(not(feature = "std"))]
    {
        // Dans Exo-OS, on utiliserait exo_logger
        let _ = msg;
    }
}

/// Log une erreur
fn log_error(msg: &str) {
    #[cfg(feature = "std")]
    eprintln!("[ERROR] {}", msg);

    #[cfg(not(feature = "std"))]
    {
        let _ = msg;
    }
}

/// Panic handler pour no_std
#[cfg(not(feature = "std"))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

/// Entry point stub pour no_std
#[cfg(not(feature = "std"))]
#[lang = "eh_personality"]
extern "C" fn eh_personality() {}
