//! Boot Module for x86_64
//! 
//! Contains boot.asm (assembly), boot.c (C bridge), trampoline.asm (SMP)
//! These files are compiled separately by build.rs

// No Rust code here - all in ASM/C
// Files:
// - boot.asm: Multiboot2 header, 32→64 bit transition
// - boot.c: C bridge to Rust, serial init
// - trampoline.asm: SMP boot trampoline
