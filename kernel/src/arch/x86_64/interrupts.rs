// src/arch/x86_64/interrupts.rs
// Gestionnaires d'interruptions

use x86_64::instructions::interrupts::{enable, disable};
use crate::println;

/// Initialise les interruptions
pub fn init() {
    println!("Initialisation des interruptions...");
    
    // Activer les interruptions
    enable();
    
    println!("Interruptions activées.");
}

/// Désactive les interruptions
pub fn disable() {
    disable();
}

/// Active les interruptions
pub fn enable() {
    enable();
}

/// Exécute une fonction avec les interruptions désactivées
pub fn without_interrupts<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    // Désactiver les interruptions
    disable();
    
    // Exécuter la fonction
    let result = f();
    
    // Réactiver les interruptions
    enable();
    
    result
}