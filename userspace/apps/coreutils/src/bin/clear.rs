#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#[cfg(target_os = "none")]
exo_coreutils::exo_command!(exo_coreutils::bare::cmd_clear);
#[cfg(not(target_os = "none"))]
fn main() { print!("\x1b[2J\x1b[H"); }
