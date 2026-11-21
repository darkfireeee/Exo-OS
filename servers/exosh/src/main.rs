#![allow(dead_code, unused_imports)]

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

#[allow(unused_unsafe)]
fn seed_default_wayland_env() {
    unsafe {
        if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
            std::env::set_var("XDG_RUNTIME_DIR", "/run");
        }
        if std::env::var_os("WAYLAND_DISPLAY").is_none() {
            std::env::set_var("WAYLAND_DISPLAY", "wayland-0");
        }
    }
}

fn main() {
    attach_stdio_to_tty();
    register_readiness_endpoint();
    ensure_input_ready();
    ignore_sigint();
    seed_default_wayland_env();
    crate::terminal::register_exosh_wayland_surface();

    let mut state = ShellState::new();
    write_all(b"\x0cExo-OS userspace console ready\n");
    write_all(b"Target: x86_64-unknown-exo (Rust std + wayland-rs 0.31.11)\n");
    write_all(b"Services launched by init_server. Type 'help' or 'exo wayland'.\n\n");

    loop {
        let mut line = [0u8; LINE_MAX];
        let len = read_line_interactive(&mut line, &mut state);
        let command = trim(&line[..len]);
        if command.is_empty() {
            continue;
        }
        state.remember(command);
        run_commands(command, &mut state);
    }
}
