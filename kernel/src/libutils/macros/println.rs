//! Macro `kprintln!` pour le debug
//! 
//! Ce module fournit une macro pour afficher des messages de debug
//! dans le noyau, similaire à println! mais adaptée pour no_std.

/// Macro pour afficher des messages de debug dans le noyau
#[macro_export]
macro_rules! kprintln {
    () => {
        $crate::kprint!("\n")
    };
    ($($arg:tt)*) => {
        $crate::kprint!("{}\n", format_args!($($arg)*))
    };
}

/// Macro pour afficher des messages dans le noyau sans retour à la ligne
#[macro_export]
macro_rules! kprint {
    ($($arg:tt)*) => {
        #[allow(unused_imports)]
        use core::fmt::Write;
        
        // Utiliser un writer vers le port série pour le debug
        let mut writer = $crate::macros::serial::SerialWriter::new();
        let _ = write!(writer, $($arg)*);
    };
}

/// Module interne pour l'écriture vers le port série
#[doc(hidden)]
pub mod serial {
    use core::fmt;
    use crate::libutils::arch::x86_64::registers;

    /// Writer vers le port série COM1
    pub struct SerialWriter;

    impl SerialWriter {
        /// Crée un nouveau writer vers le port série
        pub const fn new() -> Self {
            Self
        }
    }

    impl fmt::Write for SerialWriter {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            for byte in s.bytes() {
                // Écrire vers le port série COM1 (adresse 0x3F8)
                unsafe {
                    // Attendre que le port soit prêt
                    while (registers::read_port_u8(0x3F8 + 5) & 0x20) == 0 {
                        registers::nop();
                    }
                    // Écrire le byte
                    registers::write_port_u8(0x3F8, byte);
                }
            }
            Ok(())
        }
    }
}
