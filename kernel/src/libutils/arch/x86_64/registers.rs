//! Accès aux registres CPU pour x86_64
//! 
//! Ce module fournit des fonctions pour lire et écrire dans les registres
//! du processeur x86_64.

use core::arch::asm;

/// Lit le registre CR3 (Page Directory Base)
pub fn read_cr3() -> usize {
    let value: usize;
    unsafe {
        asm!("mov {}, cr3", out(reg) value);
    }
    value
}

/// Écrit dans le registre CR3 (Page Directory Base)
pub fn write_cr3(value: usize) {
    unsafe {
        asm!("mov cr3, {}", in(reg) value);
    }
}

/// Lit le registre CR2 (Page Fault Linear Address)
pub fn read_cr2() -> usize {
    let value: usize;
    unsafe {
        asm!("mov {}, cr2", out(reg) value);
    }
    value
}

/// Lit le registre CR0 (Control Register 0)
pub fn read_cr0() -> usize {
    let value: usize;
    unsafe {
        asm!("mov {}, cr0", out(reg) value);
    }
    value
}

/// Écrit dans le registre CR0 (Control Register 0)
pub fn write_cr0(value: usize) {
    unsafe {
        asm!("mov cr0, {}", in(reg) value);
    }
}

/// Lit le registre CR4 (Control Register 4)
pub fn read_cr4() -> usize {
    let value: usize;
    unsafe {
        asm!("mov {}, cr4", out(reg) value);
    }
    value
}

/// Écrit dans le registre CR4 (Control Register 4)
pub fn write_cr4(value: usize) {
    unsafe {
        asm!("mov cr4, {}", in(reg) value);
    }
}

/// Invalide une page dans le TLB
pub fn invlpg(addr: usize) {
    unsafe {
        asm!("invlpg [{}]", in(reg) addr);
    }
}

/// Invalide toutes les pages dans le TLB
pub fn invltlb() {
    write_cr3(read_cr3());
}

/// Active les interruptions
pub fn enable_interrupts() {
    unsafe {
        asm!("sti");
    }
}

/// Désactive les interruptions
pub fn disable_interrupts() {
    unsafe {
        asm!("cli");
    }
}

/// Vérifie si les interruptions sont activées
pub fn interrupts_enabled() -> bool {
    let flags: usize;
    unsafe {
        asm!("pushf; pop {}", out(reg) flags);
    }
    flags & 0x200 != 0
}

/// Lit le registre RFLAGS
pub fn read_rflags() -> usize {
    let flags: usize;
    unsafe {
        asm!("pushf; pop {}", out(reg) flags);
    }
    flags
}

/// Arrête le processeur jusqu'à la prochaine interruption
pub fn hlt() {
    unsafe {
        asm!("hlt");
    }
}

/// Instruction NOP (No Operation)
pub fn nop() {
    unsafe {
        asm!("nop");
    }
}

/// Instruction de barrière de mémoire
pub fn mfence() {
    unsafe {
        asm!("mfence");
    }
}

/// Instruction de barrière de mémoire pour les écritures
pub fn sfence() {
    unsafe {
        asm!("sfence");
    }
}

/// Instruction de barrière de mémoire pour les lectures
pub fn lfence() {
    unsafe {
        asm!("lfence");
    }
}

/// Lit le registre TSC (Time Stamp Counter)
pub fn read_tsc() -> u64 {
    let low: u32;
    let high: u32;
    unsafe {
        asm!("rdtsc", out("eax") low, out("edx") high);
    }
    ((high as u64) << 32) | (low as u64)
}

/// Lit le registre MSR (Model Specific Register)
pub fn read_msr(msr: u32) -> u64 {
    let low: u32;
    let high: u32;
    unsafe {
        asm!("rdmsr", in("ecx") msr, out("eax") low, out("edx") high);
    }
    ((high as u64) << 32) | (low as u64)
}

/// Écrit dans le registre MSR (Model Specific Register)
pub fn write_msr(msr: u32, value: u64) {
    let low = value as u32;
    let high = (value >> 32) as u32;
    unsafe {
        asm!("wrmsr", in("ecx") msr, in("eax") low, in("edx") high);
    }
}

/// Lit le registre FS base
pub fn read_fs_base() -> usize {
    let value: usize;
    unsafe {
        asm!("rdfsbase {}", out(reg) value);
    }
    value
}

/// Écrit dans le registre FS base
pub fn write_fs_base(value: usize) {
    unsafe {
        asm!("wrfsbase {}", in(reg) value);
    }
}

/// Lit le registre GS base
pub fn read_gs_base() -> usize {
    let value: usize;
    unsafe {
        asm!("rdgsbase {}", out(reg) value);
    }
    value
}

/// Écrit dans le registre GS base
pub fn write_gs_base(value: usize) {
    unsafe {
        asm!("wrgsbase {}", in(reg) value);
    }
}

/// Lit le registre GS base du kernel
pub fn read_kernel_gs_base() -> u64 {
    read_msr(0xC0000102)
}

/// Écrit dans le registre GS base du kernel
pub fn write_kernel_gs_base(value: u64) {
    write_msr(0xC0000102, value);
}/// Lit un octet depuis un port E/S
#[inline]
pub fn read_port_u8(port: u16) -> u8 {
    let result: u8;
    unsafe {
        asm!(
            "in al, dx",
            out("al") result,
            in("dx") port,
            options(nomem, nostack, preserves_flags)
        );
    }
    result
}

/// Écrit un octet vers un port E/S
#[inline]
pub fn write_port_u8(port: u16, value: u8) {
    unsafe {
        asm!(
            "out dx, al",
            in("dx") port,
            in("al") value,
            options(nomem, nostack, preserves_flags)
        );
    }
}

/// Lit un mot (16 bits) depuis un port E/S
#[inline]
pub fn read_port_u16(port: u16) -> u16 {
    let result: u16;
    unsafe {
        asm!(
            "in ax, dx",
            out("ax") result,
            in("dx") port,
            options(nomem, nostack, preserves_flags)
        );
    }
    result
}

/// Écrit un mot (16 bits) vers un port E/S
#[inline]
pub fn write_port_u16(port: u16, value: u16) {
    unsafe {
        asm!(
            "out dx, ax",
            in("dx") port,
            in("ax") value,
            options(nomem, nostack, preserves_flags)
        );
    }
}

/// Lit un double mot (32 bits) depuis un port E/S
#[inline]
pub fn read_port_u32(port: u16) -> u32 {
    let result: u32;
    unsafe {
        asm!(
            "in eax, dx",
            out("eax") result,
            in("dx") port,
            options(nomem, nostack, preserves_flags)
        );
    }
    result
}

/// Écrit un double mot (32 bits) vers un port E/S
#[inline]
pub fn write_port_u32(port: u16, value: u32) {
    unsafe {
        asm!(
            "out dx, eax",
            in("dx") port,
            in("eax") value,
            options(nomem, nostack, preserves_flags)
        );
    }
}
