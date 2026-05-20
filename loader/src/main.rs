#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use core::panic::PanicInfo;

#[cfg(all(target_os = "none", feature = "dynamic_linking"))]
use exo_loader::dynamic_linker::{runtime_entry, DynamicLoaderHandoff, LoaderError, UserJump};

#[cfg(target_os = "none")]
const SYS_EXIT: u64 = 60;

#[cfg(all(target_os = "none", feature = "dynamic_linking"))]
#[no_mangle]
pub extern "C" fn _start(handoff: *const DynamicLoaderHandoff) -> ! {
    debug_write(b"LD:entry\n");
    let result = unsafe { runtime_entry(handoff) };
    match result {
        Ok(jump) => {
            debug_write(b"LD:jump\n");
            unsafe { jump_to_user(jump) }
        }
        Err(err) => {
            debug_write(b"LD:error ");
            debug_error(err);
            debug_write(b"\n");
            exit(38);
        }
    }
}

#[cfg(all(target_os = "none", not(feature = "dynamic_linking")))]
#[no_mangle]
pub extern "C" fn _start() -> ! {
    debug_write(b"LD:disabled\n");
    exit(38);
}

#[cfg(not(target_os = "none"))]
fn main() {}

#[cfg(target_os = "none")]
fn exit(code: u64) -> ! {
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") SYS_EXIT,
            in("rdi") code,
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

#[cfg(all(target_os = "none", feature = "dynamic_linking"))]
unsafe fn jump_to_user(jump: UserJump) -> ! {
    unsafe {
        core::arch::asm!(
            "xor rbp, rbp",
            "jmp rax",
            in("rax") jump.entry,
            in("rdi") jump.arg0,
            options(noreturn),
        );
    }
}

#[cfg(target_os = "none")]
#[inline(always)]
fn debug_byte(byte: u8) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!(
            "out 0xE9, al",
            in("al") byte,
            options(nomem, nostack, preserves_flags)
        );
    }
    let _ = byte;
}

#[cfg(target_os = "none")]
fn debug_write(bytes: &[u8]) {
    for &byte in bytes {
        debug_byte(byte);
    }
}

#[cfg(all(target_os = "none", feature = "dynamic_linking"))]
fn debug_error(err: LoaderError) {
    let code = match err {
        LoaderError::NullHandoff => b'N',
        LoaderError::BadMagic => b'M',
        LoaderError::UnsupportedVersion => b'V',
        LoaderError::EmptyEntry => b'E',
        LoaderError::Dynamic(_) => b'D',
        LoaderError::Relocation(_) => b'R',
        LoaderError::UnsupportedNeededLibrary => b'L',
        LoaderError::UnsupportedPltRelocation => b'P',
        LoaderError::BadRelocationEntrySize => b'S',
    };
    debug_byte(code);
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    debug_write(b"LD:panic\n");
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") SYS_EXIT,
            in("rdi") 22u64,
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
