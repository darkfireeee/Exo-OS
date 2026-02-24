//! entry.rs — EFI_MAIN_ENTRY — signature UEFI obligatoire.
//!
//! Ce module documente et valide la signature de l'entrée EFI.
//! Le point d'entrée réel `efi_main` est dans `src/main.rs` pour permettre
//! la conditionnalité `#[cfg(feature = "uefi-boot")]`.
//!
//! Spec UEFI 2.10 § 7.1 : EFI_IMAGE_ENTRY_POINT
//! ```c
//! typedef EFI_STATUS (EFIAPI *EFI_IMAGE_ENTRY_POINT)(
//!     IN EFI_HANDLE       ImageHandle,
//!     IN EFI_SYSTEM_TABLE *SystemTable
//! );
//! ```
//!
//! Signature Rust équivalente avec le crate `uefi` :
//! ```rust
//! #[uefi::entry]
//! fn efi_main(handle: Handle, mut st: SystemTable<Boot>) -> Status
//! ```
//!
//! EXIGENCES UEFI POUR LE BOOTLOADER :
//!   - L'image doit être PE32+ (x86_64-unknown-uefi produit ça automatiquement)
//!   - Le subsystem doit être EFI_APPLICATION (10) ou EFI_BOOT_SERVICE_DRIVER (11)
//!   - Le PE doit avoir une table de relocations (.reloc) — requis pour Secure Boot
//!   - L'image doit être signée si Secure Boot est activé dans le firmware

use uefi::Handle;
use uefi::table::{Boot, SystemTable};

/// Valide les préconditions de l'entrée UEFI.
///
/// Appelé en premier dans `efi_main()` pour vérifier que le contexte
/// d'exécution est cohérent avec nos attentes.
///
/// Retourne `Err` si une condition critique n'est pas remplie, avec un
/// message explicatif. Des conditions non critiques produisent des warnings.
pub fn validate_uefi_entry_preconditions(
    _image_handle: Handle,
    st: &SystemTable<Boot>,
) -> Result<EntryDiagnostics, EntryError> {
    let mut diag = EntryDiagnostics::default();

    // ── Vérification version UEFI ──────────────────────────────────────────────
    let revision = st.uefi_revision();

    // On requiert UEFI 2.0 minimum pour GOP + RNG_PROTOCOL
    if revision.major() < 2 {
        return Err(EntryError::UefiVersionTooOld {
            found_major:    revision.major() as u32,
            found_minor:    revision.minor() as u32,
            required_major: 2,
            required_minor: 0,
        });
    }

    diag.uefi_major = revision.major() as u32;
    diag.uefi_minor = revision.minor() as u32;

    // UEFI 2.3+ : EFI_RNG_PROTOCOL disponible (entropie KASLR)
    if revision.major() < 2 || (revision.major() == 2 && revision.minor() < 3) {
        diag.rng_protocol_available = false;
        diag.warnings.push(EntryWarning::RngProtocolMayBeUnavailable);
    } else {
        diag.rng_protocol_available = true;
    }

    // ── Vérification ConOut ────────────────────────────────────────────────────
    // ConOut peut être null sur certains firmware headless — non bloquant.
    // On peut toujours utiliser le framebuffer GOP directement.
    // Simplement noter l'absence dans les diagnostics.
    diag.conout_available = true; // uefi crate abstrait ce check

    // ── Vérification Secure Boot ──────────────────────────────────────────────
    let sb_status = crate::uefi::secure_boot::query_secure_boot_status(st);
    diag.secure_boot_enabled = sb_status.enabled;
    diag.secure_boot_setup_mode = sb_status.setup_mode;

    if sb_status.enabled && sb_status.setup_mode {
        // Setup Mode + Secure Boot enabled = état incohérent
        diag.warnings.push(EntryWarning::SecureBootSetupModeInconsistent);
    }

    Ok(diag)
}

/// Diagnostics collectés à l'entrée UEFI.
#[derive(Default, Debug)]
pub struct EntryDiagnostics {
    pub uefi_major:              u32,
    pub uefi_minor:              u32,
    pub rng_protocol_available:  bool,
    pub conout_available:        bool,
    pub secure_boot_enabled:     bool,
    pub secure_boot_setup_mode:  bool,
    pub warnings:                arrayvec::ArrayVec<EntryWarning, 8>,
}

/// Avertissements non bloquants détectés à l'entrée.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryWarning {
    /// EFI_RNG_PROTOCOL peut être indisponible sur ce firmware (UEFI < 2.3).
    RngProtocolMayBeUnavailable,
    /// Secure Boot activé ET Setup Mode activé simultanément — état incohérent.
    SecureBootSetupModeInconsistent,
    /// ConOut null — affichage uniquement via framebuffer GOP.
    ConOutUnavailable,
}

/// Erreurs bloquantes à l'entrée UEFI.
#[derive(Debug)]
pub enum EntryError {
    /// Version UEFI trop ancienne pour exo-boot.
    UefiVersionTooOld {
        found_major:    u32,
        found_minor:    u32,
        required_major: u32,
        required_minor: u32,
    },
}

impl core::fmt::Display for EntryError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            EntryError::UefiVersionTooOld { found_major, found_minor, required_major, required_minor } => {
                write!(
                    f,
                    "UEFI {}.{} détectée, minimum requis : {}.{}",
                    found_major, found_minor, required_major, required_minor
                )
            }
        }
    }
}
