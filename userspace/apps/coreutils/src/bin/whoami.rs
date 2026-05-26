#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#[cfg(target_os = "none")]
exo_coreutils::exo_command!(exo_coreutils::bare::cmd_whoami);
#[cfg(not(target_os = "none"))]
fn main() { println!("{}", std::env::var("USER").unwrap_or_else(|_| "user".into())); }
