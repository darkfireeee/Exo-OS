#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use core::panic::PanicInfo;

#[cfg(all(target_os = "none", feature = "dynamic_linking"))]
#[no_mangle]
pub extern "C" fn _start() -> ! {
    const SYS_EXIT: u64 = 60;
    const ENOSYS: u64 = 38;
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") SYS_EXIT,
            in("rdi") ENOSYS,
            lateout("rax") _,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(all(target_os = "none", not(feature = "dynamic_linking")))]
#[no_mangle]
pub extern "C" fn _start() -> ! {
    const SYS_EXIT: u64 = 60;
    const ENOSYS: u64 = 38;
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") SYS_EXIT,
            in("rdi") ENOSYS,
            lateout("rax") _,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(not(target_os = "none"))]
fn main() {}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
