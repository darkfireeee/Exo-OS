// libs/exo_std/src/lib.rs
#![no_std]
#![feature(panic_info_message)]
#![feature(alloc_error_handler)]

extern crate alloc;

pub mod io;
pub mod ipc;
pub mod process;
pub mod security;
pub mod sync;
pub mod thread;
pub mod time;

// Réexportations des types de base
pub use exo_crypto::{dilithium_sign, kyber_keypair, ChaCha20};
pub use exo_ipc::{Channel, Receiver, Sender};
pub use exo_types::{Capability, ExoError, PhysAddr, Result, Rights, VirtAddr};

/// Initialise la bibliothèque standard
pub fn init() {
    exo_crypto::init();
    log::info!("exo_std initialized");
}

/// Version de la bibliothèque standard
pub const EXO_STD_VERSION: &str = "0.1.0-alpha";

/// Information sur le système
pub struct SystemInfo {
    pub version: &'static str,
    pub arch: &'static str,
    pub cpu_count: usize,
    pub memory_size: usize,
}

/// Retourne des informations sur le système
pub fn system_info() -> SystemInfo {
    SystemInfo {
        version: EXO_STD_VERSION,
        arch: option_env!("CARGO_CFG_TARGET_ARCH").unwrap_or("unknown"),
        cpu_count: sys_cpu_count(),
        memory_size: sys_memory_size(),
    }
}

/// Obtient le nombre de CPU
fn sys_cpu_count() -> usize {
    #[cfg(feature = "test_mode")]
    {
        4 // Valeur de test
    }

    #[cfg(not(feature = "test_mode"))]
    {
        // Appel système pour obtenir le nombre de CPU
        unsafe {
            extern "C" {
                fn sys_get_cpu_count() -> usize;
            }
            sys_get_cpu_count()
        }
    }
}

/// Obtient la taille de la mémoire
fn sys_memory_size() -> usize {
    #[cfg(feature = "test_mode")]
    {
        8 * 1024 * 1024 * 1024 // 8GB pour les tests
    }

    #[cfg(not(feature = "test_mode"))]
    {
        // Appel système pour obtenir la taille de la mémoire
        unsafe {
            extern "C" {
                fn sys_get_memory_size() -> usize;
            }
            sys_get_memory_size()
        }
    }
}

