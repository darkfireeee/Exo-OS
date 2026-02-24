//! defaults.rs — Configuration par défaut du bootloader.
//!
//! `BootConfig` = structure de configuration parsée depuis `exo-boot.cfg`.
//! Si le fichier est absent, les valeurs par défaut sont sûres et fonctionnelles.
//!
//! RÈGLE : Aucune allocation — toutes les chaînes utilisent `arrayvec`.

use arrayvec::ArrayString;

/// Taille maximale d'un chemin de fichier.
const MAX_PATH_LEN: usize = 256;

/// Configuration du bootloader Exo-OS.
///
/// Chargée depuis `/EFI/exo-os/exo-boot.cfg` (UEFI)
/// ou depuis le secteur de configuration BIOS.
#[derive(Debug, Clone)]
pub struct BootConfig {
    /// Chemin vers le kernel ELF (relatif à la racine ESP ou au disque BIOS).
    pub kernel_path:           ArrayString<MAX_PATH_LEN>,
    /// KASLR activé (recommandé en production, désactivable pour debug).
    pub kaslr_enabled:         bool,
    /// Secure Boot obligatoire (si `true` et pas de Secure Boot → panic).
    pub secure_boot_required:  bool,
    /// Délai avant chargement (en secondes, 0 = immédiat).
    pub boot_delay_secs:       u32,
    /// Délai d'affichage du splash screen (en millisecondes).
    pub splash_delay_ms:       u32,
    /// Résolution préférée : largeur (0 = auto).
    pub preferred_width:       u32,
    /// Résolution préférée : hauteur (0 = auto).
    pub preferred_height:      u32,
    /// Mode verbeux (log détaillé sur ConOut + framebuffer).
    pub verbose:               bool,
    /// Serial debug output (COM1 @ 115200 baud).
    pub serial_debug:          bool,
}

impl BootConfig {
    /// Crée une configuration avec les valeurs par défaut sûres.
    pub fn default_config() -> Self {
        let mut kernel_path = ArrayString::new();
        // Chemin par défaut du kernel sur l'ESP UEFI
        let _ = kernel_path.try_push_str("/EFI/exo-os/kernel.elf");

        Self {
            kernel_path,
            kaslr_enabled:         true,   // Activé par défaut (sécurité)
            secure_boot_required:  false,  // Désactivé — pour compatibilité dev
            boot_delay_secs:       0,      // Démarrage immédiat
            splash_delay_ms:       1500,   // 1.5s de splash screen
            preferred_width:       0,      // Auto (GOP choisit le mieux)
            preferred_height:      0,
            verbose:               false,
            serial_debug:          false,
        }
    }

    /// Crée une configuration pour le chemin BIOS.
    pub fn default_config_bios() -> Self {
        let mut cfg = Self::default_config();
        // En mode BIOS, le kernel est à un offset fixe sur le disque
        cfg.kernel_path.clear();
        let _ = cfg.kernel_path.try_push_str("kernel.elf");
        cfg
    }

    /// Retourne le chemin kernel comme `&str`.
    pub fn kernel_path_str(&self) -> &str {
        self.kernel_path.as_str()
    }

    /// Valide la cohérence de la configuration.
    /// Retourne `Err` si des valeurs incohérentes sont détectées.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.kernel_path.is_empty() {
            return Err(ConfigError::MissingKernelPath);
        }
        if self.boot_delay_secs > 60 {
            return Err(ConfigError::InvalidValue {
                key: "boot_delay",
                reason: "Valeur > 60 secondes",
            });
        }
        Ok(())
    }
}

// ─── Erreurs ──────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum ConfigError {
    MissingKernelPath,
    InvalidValue { key: &'static str, reason: &'static str },
    ParseError   { line: u32, reason: &'static str },
    PathTooLong  { len: usize },
}

impl core::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::MissingKernelPath =>
                write!(f, "kernel_path manquant dans la configuration"),
            Self::InvalidValue { key, reason } =>
                write!(f, "Valeur invalide pour '{}' : {}", key, reason),
            Self::ParseError { line, reason } =>
                write!(f, "Erreur parse ligne {} : {}", line, reason),
            Self::PathTooLong { len } =>
                write!(f, "Chemin trop long : {} > {}", len, MAX_PATH_LEN),
        }
    }
}
