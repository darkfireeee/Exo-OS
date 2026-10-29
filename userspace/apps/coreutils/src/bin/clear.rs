#![cfg_attr(any(target_os = "none", target_os = "exo"), no_std)]
#![cfg_attr(any(target_os = "none", target_os = "exo"), no_main)]
#[cfg(any(target_os = "none", target_os = "exo"))]
exo_coreutils::exo_command!(exo_coreutils::bare::cmd_clear);
#[cfg(not(any(target_os = "none", target_os = "exo")))]
fn main() { print!("\x1b[2J\x1b[H"); }
