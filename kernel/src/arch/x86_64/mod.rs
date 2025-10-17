// src/arch/x86_64/mod.rs
// Module principal pour l'architecture x86_64

// On déclare les sous-modules présents dans ce répertoire.
// Sans ces lignes, le code dans gdt.rs, idt.rs, etc. ne serait pas compilé.
pub mod gdt;
pub mod idt;
pub mod interrupts;

// Réexportation de registers depuis libutils
pub use crate::libutils::arch::x86_64::registers;

// Note : Les fichiers `boot.asm` et `boot.c` sont des cas spéciaux.
// - `boot.asm` est un fichier d'assemblage global, lié via le script de l'éditeur de liens.
//   Il n'a pas besoin d'être déclaré ici.
// - `boot.c` est compilé en une bibliothèque statique par `build.rs`.
//   Sa fonction `kmain` est déclarée comme `extern "C"` dans le code Rust qui l'appelle.

/// Initialise tous les composants de l'architecture x86_64.
///
/// Cette fonction est appelée depuis le point d'entrée principal du noyau (`rust_main`)
/// pour mettre en place l'environnement d'exécution de base sur un processeur x86_64.
pub fn init(cores: usize) {
    crate::println!("Initialisation de l'architecture x86_64...");

    // L'ordre d'initialisation est important.
    // 1. La GDT (Global Descriptor Table) doit être chargée en premier.
    gdt::init();
    
    // 2. L'IDT (Interrupt Descriptor Table) est configurée ensuite.
    idt::init();
    
    // 3. Enfin, on active les interruptions matérielles.
    interrupts::init();

    crate::println!("Architecture x86_64 initialisée avec succès.");
}