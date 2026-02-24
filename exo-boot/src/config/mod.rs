//! mod.rs — Module config : chargement de la configuration bootloader.
//!
//! Fournit `load_config_uefi()` et `load_config_bios()`.
//! Dans les deux cas, retourne un `BootConfig` complet (defaults si pas de fichier).
//!
//! Chemin UEFI : `/EFI/exo-os/exo-boot.cfg` sur l'ESP actif.
//! Chemin BIOS : Config intégrée en dur (pas de lecture fichier en mode réel).

pub mod defaults;
pub mod parser;

pub use defaults::{BootConfig, ConfigError};

// ─── Chargement UEFI ─────────────────────────────────────────────────────────

/// Chemin du fichier de configuration sur l'ESP UEFI.
const UEFI_CONFIG_PATH: &str = "\\EFI\\exo-os\\exo-boot.cfg";

/// Charge la configuration depuis l'ESP UEFI.
/// Si le fichier est absent ou illisible, retourne les defaults.
///
/// # Safety requirements
/// Appelé avant ExitBootServices — les BootServices sont actifs.
#[cfg(feature = "uefi-boot")]
pub fn load_config_uefi(
    bt:           &uefi::table::boot::BootServices,
    image_handle: uefi::Handle,
) -> BootConfig {
    let mut config = BootConfig::default_config();

    // Tente de charger le fichier de configuration via EFI File Protocol
    match crate::uefi::protocols::file::load_file(bt, image_handle, UEFI_CONFIG_PATH) {
        Ok(file_buf) => {
            match parser::parse_config(file_buf.as_bytes(), &mut config) {
                Ok(())  => {}
                Err(_e) => {
                    // Config invalide — utilise les defaults
                    config = BootConfig::default_config();
                }
            }
        }
        Err(_) => {
            // Pas de fichier de config — defaults silencieux
        }
    }

    // Valide la configuration finale
    if config.validate().is_err() {
        config = BootConfig::default_config();
    }

    config
}

// ─── Chargement BIOS ─────────────────────────────────────────────────────────

/// Retourne la configuration par défaut pour le chemin BIOS.
/// En mode BIOS, la configuration est compilée en dur
/// (pas d'accès fichier depuis long mode sans pilote FAT).
#[cfg(feature = "bios-boot")]
pub fn load_config_bios() -> BootConfig {
    BootConfig::default_config_bios()
}

// ─── API commune ──────────────────────────────────────────────────────────────

/// Charge la configuration de façon transparente selon le chemin de boot.
///
/// Cette macro permet d'unifier le code de `main.rs`.
#[macro_export]
macro_rules! load_boot_config {
    ($handle:expr) => {{
        #[cfg(feature = "uefi-boot")]
        { $crate::config::load_config_uefi($handle) }
        #[cfg(feature = "bios-boot")]
        { $crate::config::load_config_bios() }
    }};
}
