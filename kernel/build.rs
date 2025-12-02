// Build script for Exo-OS Kernel
// NOTE: Boot ASM and C files are compiled by external link_boot.ps1 script
// This build.rs only handles other C/ASM files

use std::process::Command;

fn main() {
    // ═══════════════════════════════════════════════════════════════
    // 1. Compile and link boot objects
    // ═══════════════════════════════════════════════════════════════
    // Boot files (boot.asm, boot.c) must be compiled first
    
    let out_dir = std::env::var("OUT_DIR").unwrap();
    
    println!("cargo:rerun-if-changed=src/arch/x86_64/boot/boot.asm");
    println!("cargo:rerun-if-changed=src/arch/x86_64/boot/boot.c");
    
    // Compile boot objects using link_boot.ps1 script
    let link_boot_status = Command::new("pwsh")
        .args(&[
            "-ExecutionPolicy", "Bypass",
            "-File", "../../link_boot.ps1",
            "-OutDir", &out_dir
        ])
        .status();
    
    match link_boot_status {
        Ok(s) if s.success() => {
            println!("cargo:rustc-link-search=native={}", out_dir);
            println!("cargo:rustc-link-arg=--whole-archive");
            println!("cargo:rustc-link-lib=static=boot_combined");
            println!("cargo:rustc-link-arg=--no-whole-archive");
        }
        _ => {
            println!("cargo:warning=Failed to compile boot objects with link_boot.ps1");
            println!("cargo:warning=Run './link_boot.ps1' manually before building");
        }
    }
    
    
    // ═══════════════════════════════════════════════════════════════
    // 2. Compile other C/ASM sources (if needed)
    // ═══════════════════════════════════════════════════════════════
    
    // Serial driver (C compat) - DISABLED: Causes linker issues with MSVC
    // Compiled separately in build.sh via GCC instead
    /*
    if std::path::Path::new("src/c_compat/serial.c").exists() {
        println!("cargo:rerun-if-changed=src/c_compat/serial.c");
        cc::Build::new()
            .file("src/c_compat/serial.c")
            .flag("-ffreestanding")
            .flag("-nostdlib")
            .flag("-fno-builtin")
            .flag("-fno-stack-protector")
            .compile("serial");
    }
    */
    
    // Context switch assembly - REMOVED: Now using global_asm! in windowed.rs
    // No external .S file needed - ASM is compiled directly into the kernel
    // See: kernel/src/scheduler/switch/windowed.rs
    
    // ═══════════════════════════════════════════════════════════════
    // 3. Compile IDT handlers (ASM) to avoid LLVM naked_asm! issues
    // ═══════════════════════════════════════════════════════════════
    if std::path::Path::new("src/arch/x86_64/idt_handlers.asm").exists() {
        println!("cargo:rerun-if-changed=src/arch/x86_64/idt_handlers.asm");
        
        // Try to assemble with NASM
        let obj_file = format!("{}/idt_handlers.o", out_dir);
        let status = Command::new("nasm")
            .args(&[
                "-f", "win64",  // Win64 COFF format for MSVC
                "-o", &obj_file,
                "src/arch/x86_64/idt_handlers.asm"
            ])
            .status();
        
        match status {
            Ok(s) if s.success() => {
                println!("cargo:rustc-link-search=native={}", out_dir);
                println!("cargo:rustc-link-arg={}", obj_file);
            }
            _ => {
                // If NASM fails, just warn but don't fail build
                // (naked_asm! stubs will be used instead)
                println!("cargo:warning=Could not assemble idt_handlers.asm with NASM");
                println!("cargo:warning=Using Rust stubs instead (may have performance impact)");
            }
        }
    }
}
