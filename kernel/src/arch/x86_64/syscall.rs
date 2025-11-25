//! Syscall Entry Point for x86_64
//! 
//! SYSCALL/SYSRET fast path implementation.

use core::arch::asm;

/// MSR addresses
const IA32_STAR: u32 = 0xC0000081;
const IA32_LSTAR: u32 = 0xC0000082;
const IA32_FMASK: u32 = 0xC0000084;
const IA32_EFER: u32 = 0xC0000080;

/// Initialize SYSCALL/SYSRET
pub unsafe fn init() {
    // Set STAR: segment selectors for syscall/sysret
    // Bits 32-47: Kernel CS/SS base
    // Bits 48-63: User CS/SS base  
    let star: u64 = (0x08u64 << 32) | (0x18u64 << 48);
    wrmsr(IA32_STAR, star);

    // Set LSTAR: syscall entry point
    let lstar = syscall_entry as u64;
    wrmsr(IA32_LSTAR, lstar);

    // Set FMASK: flags to clear on syscall
    let fmask: u64 = 0x200; // Clear IF (interrupts)
    wrmsr(IA32_FMASK, fmask);

    // Enable SYSCALL in EFER
    let efer = rdmsr(IA32_EFER);
    wrmsr(IA32_EFER, efer | 1);

    log::info!("SYSCALL/SYSRET initialized");
}

/// Read MSR
#[inline]
unsafe fn rdmsr(msr: u32) -> u64 {
    let (high, low): (u32, u32);
    asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") low,
        out("edx") high,
        options(nomem, nostack)
    );
    ((high as u64) << 32) | (low as u64)
}

/// Write MSR
#[inline]
unsafe fn wrmsr(msr: u32, value: u64) {
    let low = value as u32;
    let high = (value >> 32) as u32;
    asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") low,
        in("edx") high,
        options(nomem, nostack)
    );
}

/// Syscall entry point (assembly stub)
/// TODO: Fix naked_asm! compilation issue with LLVM/MSVC
/// For now, this is a placeholder that allows compilation
#[no_mangle]
pub extern "C" fn syscall_entry() {
    // FIXME: Implement proper assembly entry point
    // The naked_asm! version causes "Undefined temporary symbol .Ltmp0"
    // with current LLVM/MSVC toolchain
    // Will need either:
    // 1. External .asm file
    // 2. Different assembly syntax
    // 3. Updated toolchain version
    syscall_handler_rust();
}

// Commented out until assembly issue is resolved:
/*
#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn syscall_entry() -> ! {
    core::arch::naked_asm!(
        "swapgs",
        "mov qword ptr gs:[0], rsp",
        "mov rsp, qword ptr gs:[8]",
        "push rbp",
        "push rbx",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "push rcx",
        "push r11",
        "sub rsp, 8",
        "mov rbp, rsp",
        "call syscall_handler_rust",
        "add rsp, 8",
        "pop r11",
        "pop rcx",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbx",
        "pop rbp",
        "mov rsp, qword ptr gs:[0]",
        "swapgs",
        "sysretq",
    )
}
*/

/// Rust syscall handler
extern "C" fn syscall_handler_rust() {
    // TODO: Dispatch to syscall table
}
