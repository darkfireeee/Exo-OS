// libs/exo_std/src/lib.rs
//! Bibliothèque standard pour les applications natives Exo-OS
//!
//! exo_std fournit une API robuste, optimisée et type-safe pour interagir
//! avec le kernel Exo-OS. Toutes les implémentations sont no_std et suivent
//! des principes de zero-cost abstraction.
//!
//! # Modules Principaux
//!
//! - **sync**: Primitives de synchronisation (Mutex, RwLock, Condvar, etc.)
//! - **collections**: Structures de données optimisées (BoundedVec, SmallVec, RingBuffer, etc.)
//! - **io**: Opérations d'entrée/sortie
//! - **process**: Gestion des processus
//! - **thread**: Gestion des threads
//! - **time**: Primitives temporelles
//! - **syscall**: Couche d'abstraction pour appels système

#![no_std]
#![feature(alloc_error_handler)]
#![feature(min_specialization)]
#![feature(const_trait_impl)]
#![feature(const_mut_refs)]

extern crate alloc;

// Modules publics
pub mod error;
pub mod syscall;
pub mod collections;
pub mod sync;
pub mod io;
pub mod ipc;
pub mod process;
pub mod security;
pub mod thread;
pub mod time;

// Réexportations pour l'API publique
pub use error::{Result, ExoStdError};

// Types de base depuis exo_types
pub use exo_types::{Capability, PhysAddr, VirtAddr, Rights};

// Cryptographie depuis exo_crypto
pub use exo_crypto::{dilithium_sign, kyber_keypair, ChaCha20};

// IPC depuis exo_ipc
pub use exo_ipc::{Channel, Receiver, Sender};

// Synchronisation
pub use sync::{
    Mutex, MutexGuard,
    RwLock, RwLockReadGuard, RwLockWriteGuard,
    Condvar, Barrier, Once, OnceLock,
    AtomicCell, Ordering,
};

// Collections
pub use collections::{
    BoundedVec, SmallVec, RingBuffer,
    IntrusiveList, IntrusiveNode,
    RadixTree, CapacityError,
};

/// Version de la bibliothèque standard
pub const EXO_STD_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Version majeure
pub const VERSION_MAJOR: u32 = 0;
/// Version mineure
pub const VERSION_MINOR: u32 = 2;
/// Version patch
pub const VERSION_PATCH: u32 = 0;

/// Information sur le système
#[derive(Debug, Clone, Copy)]
pub struct SystemInfo {
    /// Version d'exo_std
    pub version: &'static str,
    /// Architecture CPU
    pub arch: &'static str,
    /// Nombre de CPUs
    pub cpu_count: usize,
    /// Taille mémoire totale (bytes)
    pub memory_size: usize,
}

/// Retourne des informations sur le système
///
/// # Exemple
/// ```no_run
/// use exo_std::system_info;
///
/// let info = system_info();
/// println!("CPUs: {}", info.cpu_count);
/// println!("Memory: {} bytes", info.memory_size);
/// ```
#[inline]
pub fn system_info() -> SystemInfo {
    SystemInfo {
        version: EXO_STD_VERSION,
        arch: option_env!("CARGO_CFG_TARGET_ARCH").unwrap_or("unknown"),
        cpu_count: sys_cpu_count(),
        memory_size: sys_memory_size(),
    }
}

/// Obtient le nombre de CPUs
#[inline]
fn sys_cpu_count() -> usize {
    #[cfg(feature = "test_mode")]
    {
        4 // Valeur de test
    }

    #[cfg(not(feature = "test_mode"))]
    {
        // Appel système pour obtenir le nombre de CPU
        unsafe {
            // Utilise le syscall approprié
            use syscall::SyscallReturn;
            // TODO: implémenter le syscall spécifique
            4 // Temporaire
        }
    }
}

/// Obtient la taille de la mémoire système
#[inline]
fn sys_memory_size() -> usize {
    #[cfg(feature = "test_mode")]
    {
        8 * 1024 * 1024 * 1024 // 8GB pour les tests
    }

    #[cfg(not(feature = "test_mode"))]
    {
        // Appel système pour obtenir la taille de la mémoire
        unsafe {
            // TODO: implémenter le syscall spécifique
            8 * 1024 * 1024 * 1024 // Temporaire
        }
    }
}

/// Initialise la bibliothèque standard
///
/// Doit être appelé au démarrage de l'application.
/// Configure les handlers d'erreurs, TLS, etc.
#[inline]
pub fn init() {
    // Configuration initiale si nécessaire
}

/// Handler global pour les erreurs d'allocation
#[cfg(not(test))]
#[alloc_error_handler]
fn alloc_error_handler(layout: core::alloc::Layout) -> ! {
    panic!("Allocation error: {:?}", layout);
}

/// Macro print! pour écrire sur stdout
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let _ = write!($crate::io::stdout(), $($arg)*);
    }};
}

/// Macro println! pour écrire sur stdout avec newline
#[macro_export]
macro_rules! println {
    () => { $crate::print!("\n") };
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let _ = writeln!($crate::io::stdout(), $($arg)*);
    }};
}

/// Macro eprint! pour écrire sur stderr
#[macro_export]
macro_rules! eprint {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let _ = write!($crate::io::stderr(), $($arg)*);
    }};
}

/// Macro eprintln! pour écrire sur stderr avec newline
#[macro_export]
macro_rules! eprintln {
    () => { $crate::eprint!("\n") };
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let _ = writeln!($crate::io::stderr(), $($arg)*);
    }};
}

// Tests de la bibliothèque
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version() {
        assert_eq!(VERSION_MAJOR, 0);
        assert_eq!(VERSION_MINOR, 2);
    }

    #[test]
    fn test_system_info() {
        let info = system_info();
        assert!(info.cpu_count > 0);
        assert!(info.memory_size > 0);
    }
}

