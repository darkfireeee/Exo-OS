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
    println!("cargo:rerun-if-changed=src/arch/x86_64/boot/trampoline.asm");
    
    // Assemble trampoline.asm with nasm
    let trampoline_asm = "src/arch/x86_64/boot/trampoline.asm";
    let trampoline_obj = format!("{}/trampoline.o", out_dir);
    
    let nasm_status = Command::new("nasm")
        .args(&[
            "-f", "elf64",
            "-o", &trampoline_obj,
            trampoline_asm
        ])
        .status();
    
    match nasm_status {
        Ok(s) if s.success() => {
            println!("cargo:rustc-link-search=native={}", out_dir);
            println!("cargo:rustc-link-lib=static:+whole-archive=trampoline");
            
            // Create static library from trampoline.o
            let ar_status = Command::new("ar")
                .args(&["crus", &format!("{}/libtrampoline.a", out_dir), &trampoline_obj])
                .status();
            
            if !ar_status.map(|s| s.success()).unwrap_or(false) {
                println!("cargo:warning=Failed to create libtrampoline.a");
            }
        }
        _ => {
            println!("cargo:warning=Failed to assemble trampoline.asm with nasm");
            println!("cargo:warning=Install nasm: sudo apk add nasm");
        }
    }
    
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
    compile_asm_file(&out_dir, "src/arch/x86_64/idt_handlers.asm", "idt_handlers");
    
    // ═══════════════════════════════════════════════════════════════
    // 4. Compile Syscall entry point (ASM)
    // ═══════════════════════════════════════════════════════════════
    compile_asm_file(&out_dir, "src/arch/x86_64/syscall_entry.asm", "syscall_entry");
    
    // ═══════════════════════════════════════════════════════════════
    // 5. Compile AP (Application Processor) Trampoline for SMP
    // ═══════════════════════════════════════════════════════════════
    compile_asm_file(&out_dir, "src/arch/x86_64/smp/ap_trampoline.asm", "ap_trampoline");
}

/// Compile an assembly file with NASM
fn compile_asm_file(out_dir: &str, src_path: &str, name: &str) {
    if !std::path::Path::new(src_path).exists() {
        println!("cargo:warning=ASM file {} not found", src_path);
        return;
    }
    
    println!("cargo:rerun-if-changed={}", src_path);
    
    // Try ELF64 format first (for Linux/Codespace), then win64 for Windows
    let obj_file = format!("{}/{}.o", out_dir, name);
    
    // Detect platform and use appropriate format
    let format = if cfg!(target_os = "windows") { "win64" } else { "elf64" };
    
    let status = Command::new("nasm")
        .args(&[
            "-f", format,
            "-o", &obj_file,
            src_path
        ])
        .status();
    
    match status {
        Ok(s) if s.success() => {
            println!("cargo:rustc-link-search=native={}", out_dir);
            println!("cargo:rustc-link-arg={}", obj_file);
            println!("cargo:warning=Compiled {} with NASM ({})", name, format);
        }
        _ => {
            println!("cargo:warning=Could not assemble {} with NASM", src_path);
            println!("cargo:warning=Using Rust stubs instead (may have performance impact)");
        }
    }
}
