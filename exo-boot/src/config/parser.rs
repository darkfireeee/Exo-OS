//! parser.rs — Parseur de fichier de configuration `exo-boot.cfg`.
//!
//! Format : clé=valeur, une paire par ligne.
//! Commentaires : lignes commençant par '#' ou ';'.
//! Paires vides et whitespace ignorés.
//!
//! Exemple de fichier :
//! ```
//! # Configuration Exo-OS bootloader
//! kernel_path=/EFI/exo-os/kernel.elf
//! kaslr=true
//! secure_boot_required=false
//! boot_delay=0
//! verbose=false
//! ```
//!
//! RÈGLE : Aucune allocation heap — `no_std` pur.
//! Le buffer de configuration est passé comme `&[u8]`.

use super::defaults::{BootConfig, ConfigError};

/// Nombre maximal de lignes dans le fichier de config.
const MAX_LINES: usize = 64;

/// Parse un fichier de configuration `exo-boot.cfg` depuis un buffer `&[u8]`.
///
/// Les clés inconnues sont silencieusement ignorées (forward compatibility).
/// En cas d'erreur de parse, la valeur par défaut est conservée.
pub fn parse_config(data: &[u8], base: &mut BootConfig) -> Result<(), ConfigError> {
    let text = match core::str::from_utf8(data) {
        Ok(s)  => s,
        Err(_) => return Err(ConfigError::ParseError {
            line:   0,
            reason: "Fichier de config n'est pas UTF-8 valide",
        }),
    };

    let mut line_num: u32 = 0;

    for line in text.split('\n') {
        line_num += 1;
        if line_num > MAX_LINES as u32 {
            break; // Sécurité — ignore les lignes en excès
        }

        let line = line.trim();

        // Ignore les lignes vides et les commentaires
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }

        // Parse clé=valeur
        let (key, value) = match line.split_once('=') {
            Some(pair) => (pair.0.trim(), pair.1.trim()),
            None => {
                // Ligne mal formée — ignore avec avertissement
                continue;
            }
        };

        // Applique la valeur
        parse_key_value(key, value, base, line_num)?;
    }

    Ok(())
}

/// Applique une paire clé=valeur à la configuration.
fn parse_key_value(
    key:      &str,
    value:    &str,
    cfg:      &mut BootConfig,
    line:     u32,
) -> Result<(), ConfigError> {
    match key {
        "kernel_path" | "kernel" => {
            if value.len() > cfg.kernel_path.capacity() {
                return Err(ConfigError::PathTooLong { len: value.len() });
            }
            cfg.kernel_path.clear();
            cfg.kernel_path.try_push_str(value)
                .map_err(|_| ConfigError::PathTooLong { len: value.len() })?;
        }
        "kaslr" | "kaslr_enabled" => {
            cfg.kaslr_enabled = parse_bool(value).ok_or(ConfigError::InvalidValue {
                key: "kaslr",
                reason: "Valeur attendue : true/false/1/0",
            })?;
        }
        "secure_boot_required" | "secure_boot" => {
            cfg.secure_boot_required = parse_bool(value).ok_or(ConfigError::InvalidValue {
                key: "secure_boot_required",
                reason: "Valeur attendue : true/false/1/0",
            })?;
        }
        "boot_delay" | "boot_delay_secs" => {
            cfg.boot_delay_secs = parse_u32(value).ok_or(ConfigError::InvalidValue {
                key: "boot_delay",
                reason: "Valeur attendue : entier positif",
            })?;
        }
        "splash_delay" | "splash_delay_ms" => {
            cfg.splash_delay_ms = parse_u32(value).ok_or(ConfigError::InvalidValue {
                key: "splash_delay_ms",
                reason: "Valeur attendue : entier positif (millisecondes)",
            })?;
        }
        "width" | "preferred_width" => {
            cfg.preferred_width = parse_u32(value).ok_or(ConfigError::InvalidValue {
                key: "preferred_width",
                reason: "Valeur attendue : entier positif en pixels",
            })?;
        }
        "height" | "preferred_height" => {
            cfg.preferred_height = parse_u32(value).ok_or(ConfigError::InvalidValue {
                key: "preferred_height",
                reason: "Valeur attendue : entier positif en pixels",
            })?;
        }
        "verbose" => {
            cfg.verbose = parse_bool(value).ok_or(ConfigError::InvalidValue {
                key: "verbose",
                reason: "Valeur attendue : true/false/1/0",
            })?;
        }
        "serial_debug" | "serial" => {
            cfg.serial_debug = parse_bool(value).ok_or(ConfigError::InvalidValue {
                key: "serial_debug",
                reason: "Valeur attendue : true/false/1/0",
            })?;
        }
        // Clé inconnue — ignore silencieusement
        _ => {}
    }

    let _ = line; // Utilisé pour les messages d'erreur futurs
    Ok(())
}

// ─── Fonctions de parsing scalaires ──────────────────────────────────────────

/// Parse un booléen : "true", "1", "yes", "on" → true ; "false", "0", "no", "off" → false.
fn parse_bool(s: &str) -> Option<bool> {
    match s {
        "true" | "1" | "yes" | "on"  | "TRUE" | "YES" | "ON"  => Some(true),
        "false"| "0" | "no"  | "off" | "FALSE"| "NO"  | "OFF" => Some(false),
        _ => None,
    }
}

/// Parse un entier u32 décimal.
fn parse_u32(s: &str) -> Option<u32> {
    if s.is_empty() { return None; }

    // Support hexadécimal (0x...)
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        return u32::from_str_radix(hex, 16).ok();
    }

    // Décimal
    let mut result: u32 = 0;
    for ch in s.bytes() {
        if ch < b'0' || ch > b'9' { return None; }
        result = result.checked_mul(10)?.checked_add((ch - b'0') as u32)?;
    }
    Some(result)
}
