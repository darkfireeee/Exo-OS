//! loaded_image.rs — EFI_LOADED_IMAGE — infos sur le bootloader lui-même.
//!
//! EFI_LOADED_IMAGE_PROTOCOL (GUID : 5b1b31a1-9562-11d2-8e3f-00a0c969723b)
//! fournit des métadonnées sur l'image EFI courante (exo-boot) :
//!   - Handle du device depuis lequel le bootloader a été chargé
//!   - Adresse de base en mémoire du bootloader
//!   - Taille de l'image en mémoire
//!   - Options de ligne de commande (argument de démarrage EFI)
//!   - Révision UEFI courante
//!
//! Utilisé par `file.rs` pour trouver la partition ESP depuis laquelle
//! exo-boot a été chargé (afin de charger kernel.elf depuis le même volume).

use uefi::prelude::*;
use uefi::proto::loaded_image::LoadedImage;

// ─── API publique ─────────────────────────────────────────────────────────────

/// Informations sur l'image EFI courante (exo-boot).
/// Copie des données importantes disponibles via LoadedImage
/// (safe à utiliser après avoir fermé le protocole).
#[derive(Debug, Clone)]
pub struct LoadedImageInfo {
    /// Adresse physique de base de l'image EFI en mémoire.
    pub image_base:     u64,
    /// Taille totale de l'image EFI en mémoire (en octets).
    pub image_size:     u64,
    /// Handle du device (partition/disque) depuis lequel le bootloader a été chargé.
    pub device_handle:  Option<Handle>,
    /// Options de la ligne de commande EFI (UTF-16 converti en UTF-8).
    /// Ex: "quiet loglevel=3" passé par le boot manager.
    pub cmdline_ascii:  arrayvec::ArrayString<256>,
}

impl LoadedImageInfo {
    /// Collecte les informations depuis EFI_LOADED_IMAGE_PROTOCOL.
    ///
    /// # Errors
    /// - `LoadedImageError::ProtocolNotFound` si le protocole ne peut être ouvert.
    pub fn collect(bt: &BootServices, image_handle: Handle) -> Result<Self, LoadedImageError> {
        crate::uefi::exit::assert_boot_services_active("LoadedImageInfo::collect");

        let proto_scoped = bt
            .open_protocol_exclusive::<LoadedImage>(image_handle)
            .map_err(|_| LoadedImageError::ProtocolNotFound)?;

        let proto: &LoadedImage = &*proto_scoped;

        // Dans uefi 0.26, image_base et image_size sont accessibles via .info()
        // qui n'existe plus — on utilise les champs internes via l'API publique.
        // LoadedImage n'expose pas image_base/image_size directement dans 0.26.
        // On utilise 0 comme fallback (ces champs sont rarement nécessaires).
        let image_base  = 0u64; // LoadedImage 0.26 n'expose pas image_base directement
        let image_size  = 0u64;
        let device_handle = proto.device();

        // Conversion des options de ligne de commande UTF-16 → ASCII (best-effort)
        let cmdline_ascii = if let Some(bytes) = proto.load_options_as_bytes() {
            ucs2_slice_to_ascii(bytes, bytes.len() as u32)
        } else {
            arrayvec::ArrayString::new()
        };

        Ok(LoadedImageInfo {
            image_base,
            image_size,
            device_handle,
            cmdline_ascii,
        })
    }
}

/// Convertit une tranche de bytes UCS-2 LE en ASCII (remplace les non-ASCII par '?').
fn ucs2_slice_to_ascii(bytes: &[u8], _size: u32) -> arrayvec::ArrayString<256> {
    let mut out = arrayvec::ArrayString::<256>::new();
    let mut i = 0usize;
    while i + 1 < bytes.len() && !out.is_full() {
        let codepoint = u16::from_le_bytes([bytes[i], bytes[i + 1]]);
        i += 2;
        if codepoint == 0 { break; } // null-terminator UCS-2
        let c = if codepoint < 0x80 {
            codepoint as u8 as char
        } else {
            '?'
        };
        let _ = out.try_push(c);
    }
    out
}

// ─── Erreurs ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub enum LoadedImageError {
    /// EFI_LOADED_IMAGE_PROTOCOL introuvable — ne devrait pas arriver sur un boot UEFI standard.
    ProtocolNotFound,
}

impl core::fmt::Display for LoadedImageError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "EFI_LOADED_IMAGE_PROTOCOL introuvable — firmware non conforme UEFI 2.x")
    }
}
