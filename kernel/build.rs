// build.rs - Script de build pour compiler le code C et assembleur
// Ce script est exécuté avant la compilation du code Rust

use std::env;
use std::path::PathBuf;

fn main() {
    // Récupérer le répertoire de sortie
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    
    // Ne compiler que pour la cible bare-metal
    let target = env::var("TARGET").unwrap();
    if !target.contains("unknown-none") && !target.contains("elf") {
        println!("cargo:warning=Skipping C compilation for non-kernel target: {}", target);
        return;
    }

    // Compiler le fichier d'assembly context_switch.S
    println!("cargo:rerun-if-changed=src/scheduler/context_switch.S");
    
    // Compiler avec cc (qui gère aussi l'assembly)
    let mut build = cc::Build::new();
    build
        .file("src/scheduler/context_switch.S")
        .target(&target)
        .compiler("gcc")
        .flag("-ffreestanding")
        .flag("-fno-stack-protector")
        .flag("-fno-pic")
        .flag("-mno-red-zone")
        .flag("-mno-sse")
        .flag("-mno-sse2")
        .flag("-m64")
        .opt_level(2)
        .warnings(false); // Désactiver les warnings pour l'assembly
    
    build.compile("context_switch");

    // Lier les bibliothèques
    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=context_switch");

    // TODO: Réactiver quand clang sera disponible ou réécrire en Rust
    // Pour l'instant on utilise uart_16550 pour serial
    /*
    println!("cargo:rerun-if-changed=src/c_compat/serial.c");
    println!("cargo:rerun-if-changed=src/c_compat/pci.c");

    // Compiler les fichiers C avec cc - configuré pour bare-metal x86_64
    let mut build = cc::Build::new();
    
    build
        .file("src/c_compat/serial.c")
        .file("src/c_compat/pci.c")
        // Options de compilation pour bare-metal x86_64
        .target(&target)  // Utiliser la cible spécifiée
        .compiler("clang")  // Utiliser clang au lieu de GCC pour compatibilité avec rust-lld
        .flag("-ffreestanding")
        .flag("-fno-stack-protector")
        .flag("-fno-pic")
        .flag("-mno-red-zone")
        .flag("-mno-sse")
        .flag("-mno-sse2")
        .flag("--target=x86_64-unknown-none")  // Cible explicite pour clang
        .opt_level(2)
        .warnings(true);
    
    // Compiler la bibliothèque statique
    build.compile("c_compat");

    // Lier les bibliothèques
    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=c_compat");
    */
}
