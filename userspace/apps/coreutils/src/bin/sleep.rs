#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#[cfg(any(target_os = "none", target_os = "exo"))]
exo_coreutils::exo_command!(exo_coreutils::bare::cmd_sleep);
#[cfg(not(any(target_os = "none", target_os = "exo")))]
fn main() { std::thread::sleep(std::time::Duration::from_millis(std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0))); }
