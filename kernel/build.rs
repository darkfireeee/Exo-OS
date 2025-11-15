// build.rs - Script de build pour compiler le code C et assembleur
// Ce script est exécuté avant la compilation du code Rust

use std::env;
use std::path::PathBuf;

fn main() {
    // Récupérer le répertoire de sortie
    let _out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    
    // Ne compiler que pour la cible bare-metal
    let target = env::var("TARGET").unwrap();
    if !target.contains("unknown-none") && !target.contains("elf") {
        println!("cargo:warning=Skipping assembly compilation for non-kernel target: {}", target);
        return;
    }

    // Compiler boot.asm (entry point pour QEMU/Multiboot2) avec NASM
    println!("cargo:rerun-if-changed=src/arch/x86_64/boot.asm");
    println!("cargo:rerun-if-changed=src/c_compat/serial.c");
    
    // Compiler boot.asm avec NASM
    let boot_obj = std::path::PathBuf::from(env::var("OUT_DIR").unwrap()).join("boot.o");
    let status = std::process::Command::new("nasm")
        .args(&[
            "-f", "elf64",
            "-o", boot_obj.to_str().unwrap(),
            "src/arch/x86_64/boot.asm"
        ])
        .status()
        .expect("Failed to run nasm");
    
    if !status.success() {
        panic!("NASM compilation failed");
    }
    
    // Lier boot.o
    println!("cargo:rustc-link-arg={}", boot_obj.to_str().unwrap());
    println!("cargo:warning=Compiled boot.asm with NASM");
    
    // Compiler serial.c (port série COM1)
    cc::Build::new()
        .file("src/c_compat/serial.c")
        .flag("-ffreestanding")
        .flag("-fno-stack-protector")
        .flag("-mno-red-zone")
        .flag("-m64")
        .flag("-nostdlib")
        .flag("-O2")
        .include("src/c_compat")
        .compile("c_serial");
        
    println!("cargo:warning=Compiled serial.c");

    // Compiler windowed_context_switch.S (toujours, pour fournir 'context_switch')
    // Cela évite les problèmes d'assemblage spécifiques à Windows/MSVC avec le fichier .S complet
    println!("cargo:rerun-if-changed=src/scheduler/windowed_context_switch.S");
    cc::Build::new()
        .file("src/scheduler/windowed_context_switch.S")
        .flag("-ffreestanding")
        .flag("-fno-stack-protector")
        .flag("-mno-red-zone")
        .compile("windowed_context_switch");
    println!("cargo:warning=Compiled windowed_context_switch.S (alias context_switch)");

    // Note: context_switch.S classique reste désactivé pour compatibilité environnement
}
