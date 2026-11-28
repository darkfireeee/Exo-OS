//! # Paramètres — Centre de réglages modulaire conforme à la spécification ExoOS v0 (§1).

mod appearance;
mod input_locale;
mod shell;
mod system_sec;

pub use shell::{SettingsApp, SettingsCategory, SystemConfig};
