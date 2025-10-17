// src/arch/x86_64/mod.rs
// Module principal pour x86_64

pub mod gdt;
pub mod idt;
pub mod interrupts;

/// Initialise l'architecture x86_64
pub fn init() {
    println!("Initialisation de l'architecture x86_64...");
    
    gdt::init();
    idt::init();
    interrupts::init();
    
    println!("Architecture x86_64 initialisée.");
}