//! graphics.rs — GOP (Graphics Output Protocol) — framebuffer UEFI.
//!
//! Le GOP (GUID : 9042a9de-23dc-4a38-96fb-7aded080516a) fournit un accès
//! direct au framebuffer linéaire de l'écran, indépendamment des drivers
//! vidéo BIOS/VGA.
//!
//! Exo-boot utilise GOP pour :
//!   - Afficher le logo et la barre de progression au démarrage
//!   - Passer FramebufferInfo au kernel via BootInfo (kernel/tty l'utilise)
//!
//! RÈGLE BOOT-06 : L'adresse du framebuffer physique est dans BootInfo.
//! Le kernel mappe ce framebuffer dans son espace virtuel au démarrage.
//! Contrairement aux Boot Services, le framebuffer physique RESTE VALIDE
//! après ExitBootServices.

use uefi::proto::console::gop::{GraphicsOutput, Mode, PixelFormat};
use uefi::prelude::*;
use crate::display::framebuffer::{Framebuffer, FramebufferFormat};

// ─── Point d'entrée principal ─────────────────────────────────────────────────

/// Initialise le GOP et retourne une structure `Framebuffer` utilisable.
///
/// Sélectionne automatiquement la résolution optimale parmi celles disponibles :
///   - Préférence 1920×1080 (1080p) si disponible
///   - Sinon : résolution maximale ≤ 4K (évite les FB > 64 MB)
///   - Sinon : mode courant sans modification
///
/// # Errors
/// - `GopError::ProtocolNotFound` : Aucun GOP disponible sur ce système.
/// - `GopError::NoSuitableMode`   : GOP présent mais aucun mode utilisable.
pub fn init_gop(bt: &BootServices) -> Result<Framebuffer, GopError> {
    crate::uefi::exit::assert_boot_services_active("GOP init");

    // Trouve le handle du GOP, puis ouvre le protocole en mode exclusif.
    let gop_handle = bt
        .get_handle_for_protocol::<GraphicsOutput>()
        .map_err(|_| GopError::ProtocolNotFound)?;

    let mut gop_scoped = bt
        .open_protocol_exclusive::<GraphicsOutput>(gop_handle)
        .map_err(|_| GopError::ProtocolNotFound)?;

    let gop: &mut GraphicsOutput = &mut *gop_scoped;

    // ── Sélection du mode optimal ─────────────────────────────────────────────
    let selected_mode = select_optimal_mode(gop, bt)?;

    // ── Application du mode ───────────────────────────────────────────────────
    gop.set_mode(&selected_mode)
        .map_err(|e| GopError::SetModeFailed { status: e.status() })?;

    // ── Récupération des informations framebuffer ─────────────────────────────
    let current_mode  = gop.current_mode_info();
    let (width, height) = current_mode.resolution();
    let stride_pixels   = current_mode.stride();
    let format          = pixel_format_to_exo(current_mode.pixel_format())?;

    let mut fb     = gop.frame_buffer();
    let fb_ptr     = fb.as_mut_ptr() as u64;
    let fb_size    = fb.size() as u64;

    let framebuffer = Framebuffer {
        phys_addr:  fb_ptr,
        width:      width as u32,
        height:     height as u32,
        stride:     stride_pixels as u32,
        format,
        size_bytes: fb_size,
    };

    Ok(framebuffer)
}

// ─── Sélection du mode optimal ────────────────────────────────────────────────

/// Sélectionne le mode GOP optimal selon les critères de préférence.
fn select_optimal_mode(gop: &mut GraphicsOutput, bt: &BootServices) -> Result<Mode, GopError> {
    let mut best_mode:     Option<Mode>  = None;
    let mut best_score:    i64           = -1;
    let mut mode_count                   = 0u32;

    for mode in gop.modes(bt) {
        mode_count += 1;
        let info = mode.info();
        let (w, h) = info.resolution();

        // Exclure les modes avec PixelFormat::BltOnly (pas de framebuffer linéaire)
        if info.pixel_format() == PixelFormat::BltOnly {
            continue;
        }

        // Exclure les résolutions supérieures à 4K (framebuffer > 64MB non souhaitable)
        if w > 3840 || h > 2160 {
            continue;
        }

        let score = score_mode(w as u32, h as u32);
        if score > best_score {
            best_score = score;
            best_mode  = Some(mode);
        }
    }

    if mode_count == 0 {
        return Err(GopError::NoSuitableMode { reason: "aucun mode GOP disponible" });
    }

    best_mode.ok_or(GopError::NoSuitableMode {
        reason: "tous les modes exclus (BltOnly ou >4K)",
    })
}

/// Calcule un score pour un mode GOP.
/// Objectif : préférer 1920×1080, puis maximiser la résolution dans la limite 4K.
fn score_mode(width: u32, height: u32) -> i64 {
    // Bonus maximal pour exactement 1080p
    if width == 1920 && height == 1080 {
        return i64::MAX / 2;
    }
    // Bonus pour 720p si 1080p absent
    if width == 1280 && height == 720 {
        return i64::MAX / 4;
    }
    // Score général : pixels totaux (favorise les hautes résolutions)
    (width as i64).saturating_mul(height as i64)
}

// ─── Conversion PixelFormat ────────────────────────────────────────────────────

/// Convertit un `PixelFormat` UEFI GOP en `FramebufferFormat` interne.
fn pixel_format_to_exo(pf: PixelFormat) -> Result<FramebufferFormat, GopError> {
    match pf {
        PixelFormat::Rgb  => Ok(FramebufferFormat::Rgbx),
        PixelFormat::Bgr  => Ok(FramebufferFormat::Bgrx),
        PixelFormat::Bitmask => {
            // Pixel Bitmask — exo-boot ne gère pas les formats custom
            // (très rare sur matériel réel, courant sur certaines VMs)
            Err(GopError::UnsupportedPixelFormat { format: pf })
        }
        PixelFormat::BltOnly => {
            // BltOnly signifie pas de framebuffer linéaire — ne devrait pas arriver ici
            Err(GopError::UnsupportedPixelFormat { format: pf })
        }
    }
}

// ─── Types ────────────────────────────────────────────────────────────────────

/// Erreurs GOP.
#[derive(Debug)]
pub enum GopError {
    /// GOP absent sur ce système (headless, pas d'affichage).
    ProtocolNotFound,
    /// Aucun mode vidéo utilisable.
    NoSuitableMode { reason: &'static str },
    /// Échec de l'application d'un mode vidéo.
    SetModeFailed { status: uefi::Status },
    /// Format de pixels non supporté.
    UnsupportedPixelFormat { format: PixelFormat },
}

impl core::fmt::Display for GopError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ProtocolNotFound =>
                write!(f, "GOP (Graphics Output Protocol) introuvable sur ce système"),
            Self::NoSuitableMode { reason } =>
                write!(f, "Aucun mode GOP utilisable : {}", reason),
            Self::SetModeFailed { status } =>
                write!(f, "GOP SetMode échoué : {:?}", status),
            Self::UnsupportedPixelFormat { format } =>
                write!(f, "Format de pixels GOP non supporté : {:?}", format),
        }
    }
}
