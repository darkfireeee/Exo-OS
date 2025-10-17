//! Test de boot basique du kernel
//! 
//! Ce test vérifie que le kernel peut démarrer et exécuter du code de base

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]

use core::panic::PanicInfo;

/// Point d'entrée pour ce test
#[no_mangle]
pub extern "C" fn _start() -> ! {
    test_basic_boot();
    
    // Exit QEMU avec code de succès
    exit_qemu(QemuExitCode::Success);
    
    loop {}
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    serial_println!("[FAILED]");
    serial_println!("Panic: {}", info);
    exit_qemu(QemuExitCode::Failed);
    loop {}
}

/// Teste que le kernel peut booter et exécuter du code
fn test_basic_boot() {
    serial_println!("[TEST] Basic Boot Test...");
    
    // Test 1: Le code s'exécute
    serial_println!("  ✓ Code execution works");
    
    // Test 2: Les opérations arithmétiques fonctionnent
    let result = 2 + 2;
    assert_eq!(result, 4);
    serial_println!("  ✓ Arithmetic works");
    
    // Test 3: Les boucles fonctionnent
    let mut counter = 0;
    for i in 0..10 {
        counter += i;
    }
    assert_eq!(counter, 45);
    serial_println!("  ✓ Loops work");
    
    serial_println!("[PASSED] All basic boot tests passed!");
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum QemuExitCode {
    Success = 0x10,
    Failed = 0x11,
}

pub fn exit_qemu(exit_code: QemuExitCode) {
    use x86_64::instructions::port::Port;
    
    unsafe {
        let mut port = Port::new(0xf4);
        port.write(exit_code as u32);
    }
}

/// Macro simple pour écrire sur le port série
macro_rules! serial_println {
    () => ($crate::serial_print!("\n"));
    ($($arg:tt)*) => ($crate::serial_print!("{}\n", format_args!($($arg)*)));
}

macro_rules! serial_print {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        unsafe {
            SERIAL_WRITER.write_fmt(format_args!($($arg)*)).ok();
        }
    }};
}

/// Writer simple pour le port série
struct SerialWriter;

static mut SERIAL_WRITER: SerialWriter = SerialWriter;

impl core::fmt::Write for SerialWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for byte in s.bytes() {
            unsafe {
                // Port série COM1
                let mut port = x86_64::instructions::port::Port::<u8>::new(0x3f8);
                port.write(byte);
            }
        }
        Ok(())
    }
}

/// Pour assert_eq!
#[track_caller]
#[inline(never)]
fn assert_eq<T: PartialEq + core::fmt::Debug>(left: T, right: T) {
    if left != right {
        panic!("assertion failed: {:?} == {:?}", left, right);
    }
}
