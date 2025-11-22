// Build script for Exo-OS Kernel
// Compiles C and ASM files

fn main() {
    // Compile boot.asm with NASM
    println!("cargo:rerun-if-changed=src/arch/x86_64/boot/boot.asm");
    
    // Compile C sources
    cc::Build::new()
        .file("src/c_compat/serial.c")
        .flag("-ffreestanding")
        .flag("-nostdlib")
        .flag("-fno-builtin")
        .flag("-fno-stack-protector")
        .compile("serial");
    
    println!("cargo:rerun-if-changed=src/c_compat/serial.c");
    
    // Compile context switch assembly
    cc::Build::new()
        .file("src/scheduler/switch/windowed.S")
        .flag("-nostdlib")
        .compile("windowed");
    
    println!("cargo:rerun-if-changed=src/scheduler/switch/windowed.S");
}
