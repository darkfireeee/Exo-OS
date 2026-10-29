#![cfg_attr(any(target_os = "none", target_os = "exo"), no_std)]
#![cfg_attr(any(target_os = "none", target_os = "exo"), no_main)]
#[cfg(any(target_os = "none", target_os = "exo"))]
exo_coreutils::exo_command!(exo_coreutils::bare::cmd_whoami);
#[cfg(not(any(target_os = "none", target_os = "exo")))]
fn main() { println!("{}", std::env::var("USER").unwrap_or_else(|_| "user".into())); }
