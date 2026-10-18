#![no_std]
#![no_main]
#![allow(dead_code, unused_imports)]

use core::panic::PanicInfo;

pub mod types;
pub mod util;
pub mod terminal;
pub mod builtins;
pub mod exec;

use types::*;
use util::*;
use terminal::*;
use builtins::*;
use exec::*;

#[no_mangle]
pub extern "C" fn _start(_boot_info_virt: usize) -> ! {
    attach_stdio_to_tty();
    register_readiness_endpoint();

    let mut state = ShellState::new();
    write_all(b"\x0cExo-OS userspace console ready\n");
    write_all(b"Services launched by init_server. Type 'help' for commands.\n\n");

    loop {
        let mut line = [0u8; LINE_MAX];
        let len = read_line(&mut line, &mut state);
        let command = trim(&line[..len]);
        if command.is_empty() {
            continue;
        }
        state.remember(command);
        run_commands(command, &mut state);
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nostack, nomem));
        }
    }
}

