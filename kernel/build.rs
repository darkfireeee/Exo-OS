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

    println!("cargo:rerun-if-changed=src/c_compat/serial.c");
    println!("cargo:rerun-if-changed=src/c_compat/pci.c");
    println!("cargo:rerun-if-changed=src/arch/x86_64/boot.c");
    println!("cargo:rerun-if-changed=src/arch/x86_64/boot.asm");
    println!("cargo:rerun-if-changed=src/scheduler/context_switch.S");

    // Compiler les fichiers C avec cc
    let mut build = cc::Build::new();
    
    build
        .file("src/c_compat/serial.c")
        .file("src/c_compat/pci.c")
        // Options de compilation pour bare-metal
        .flag("-ffreestanding")
        .flag("-fno-stack-protector")
        .flag("-fno-pic")
        .flag("-mno-red-zone")
        .opt_level(2)
        .warnings(true);
    
    // Ajouter boot.c s'il existe
    if std::path::Path::new("src/arch/x86_64/boot.c").exists() {
        build.file("src/arch/x86_64/boot.c");
    }
    
    // Compiler la bibliothèque statique
    build.compile("c_compat");

    // Compiler le fichier assembleur context_switch.S s'il existe
    if std::path::Path::new("src/scheduler/context_switch.S").exists() {
        cc::Build::new()
            .file("src/scheduler/context_switch.S")
            .flag("-ffreestanding")
            .flag("-fno-pic")
            .compile("context_switch");
    }

    // Lier les bibliothèques
    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=c_compat");
    
    if std::path::Path::new("src/scheduler/context_switch.S").exists() {
        println!("cargo:rustc-link-lib=static=context_switch");
    }
}
