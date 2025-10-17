// src/main.rs
// Binaire pour les tests unitaires du noyau

#![no_std] // Pas de bibliothèque standard
#![no_main] // Pas de point d'entrée standard

use core::panic::PanicInfo;
use exo_kernel::println;

/// Cette fonction est appelée en cas de panic.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("{}", info);
    loop {}
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Point d'entrée pour les tests unitaires
    println!("Démarrage des tests unitaires du noyau...");
    
    // Exécuter les tests ici
    
    println!("Tests terminés.");
    loop {}
}