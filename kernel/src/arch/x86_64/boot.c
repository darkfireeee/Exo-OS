// src/arch/x86_64/boot.c
// Logique de boot en C (pont vers Rust)

#include <stddef.h>

// Déclaration de la fonction Rust principale
extern void rust_main(void);

// Fonction principale du noyau, appelée depuis boot.asm
void kmain(void) {
    // Initialisations bas niveau qui peuvent être plus simples en C
    // Par exemple, une configuration très précoce du matériel.
    
    // Appeler le point d'entrée principal du noyau écrit en Rust
    rust_main();
    
    // Ne devrait jamais arriver
    while (1) {
        asm volatile ("hlt");
    }
}