// src/arch/x86_64/interrupts.rs
// Gestionnaires d'interruptions

use x86_64::instructions::interrupts;

/// Initialise les interruptions
pub fn init() {
    crate::println!("Initialisation des interruptions...");

    // NE PAS activer les interruptions immédiatement !
    // Le PIC doit être initialisé en premier
    // Les interruptions seront activées manuellement plus tard

    crate::println!("Interruptions configurées (désactivées).");
}

/// Désactive les interruptions
pub fn disable_interrupts() {
    interrupts::disable();
}

/// Active les interruptions
pub fn enable_interrupts() {
    unsafe {
        interrupts::enable();
    }
}

/// Exécute une fonction avec les interruptions désactivées
pub fn without_interrupts<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    interrupts::without_interrupts(f)
}