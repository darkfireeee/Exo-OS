//! mod.rs — Module display : affichage framebuffer GOP + BIOS VGA.
//!
//! Ce module unifie l'accès graphique pendant le boot :
//!   - Chemin UEFI : `init_uefi_framebuffer()` → GOP → `Framebuffer`
//!   - Chemin BIOS : Mode texte VGA 80×25 via `crate::bios::vga`
//!
//! Après `init_uefi_framebuffer()`, les messages sont envoyés vers :
//!   1. Le framebuffer GOP (via `BootWriter`)
//!   2. Le ConOut UEFI si encore actif (avant ExitBootServices)
//!
//! RÈGLE : Ce module n'alloue pas de heap.

pub mod framebuffer;
pub mod font;

pub use framebuffer::{
    Framebuffer, FramebufferFormat, BootWriter, PanicWriter,
    init_global_framebuffer, try_get_framebuffer,
    draw_progress_bar, draw_boot_logo,
};

use crate::kernel_loader::handoff::FramebufferInfo;

// ─── Init UEFI ────────────────────────────────────────────────────────────────

/// Retourne le `FramebufferInfo` pour intégration dans BootInfo.
/// Doit être appelé après `init_uefi_framebuffer()`.
pub fn get_boot_info_framebuffer() -> FramebufferInfo {
    match try_get_framebuffer() {
        Some(fb) => fb.to_info(),
        None     => FramebufferInfo::absent(),
    }
}

/// Construit un `Framebuffer` depuis les données GOP préalablement collectées
/// par `crate::uefi::protocols::graphics` et l'enregistre globalement.
///
/// À appeler depuis `efi_main()` après `init_gop()`.
pub fn init_display_from_gop(
    phys_addr: u64,
    width:     u32,
    height:    u32,
    stride:    u32,
    format:    FramebufferFormat,
    size:      u64,
) {
    let fb = Framebuffer { phys_addr, width, height, stride, format, size_bytes: size };
    fb.clear();
    draw_boot_logo(&fb);
    init_global_framebuffer(&fb);
}

// ─── Macros d'affichage ────────────────────────────────────────────────────────

/// Affiche un message de boot sur le framebuffer + ConOut UEFI si actif.
///
/// Usage : `boot_print!("Chargement kernel : {}", path);`
#[macro_export]
macro_rules! boot_print {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        // Framebuffer GOP (toujours, si disponible)
        let _ = write!($crate::display::BootWriter, $($arg)*);
        // Logger uefi-services (ConOut) — actif avant ExitBootServices
        #[cfg(feature = "uefi-boot")]
        {
            // uefi_services redirige déjà via logger — noop ici
            let _ = format_args!($($arg)*);
        }
    }};
}

/// Affiche un message de boot avec saut de ligne.
#[macro_export]
macro_rules! boot_println {
    () => { $crate::boot_print!("\n") };
    ($($arg:tt)*) => { $crate::boot_print!("{}\n", format_args!($($arg)*)) };
}

/// Affiche un message de progression avec barre.
pub fn update_progress(step: u8, total: u8, msg: &str) {
    use core::fmt::Write;
    let pct = if total > 0 { (step as u32 * 100 / total as u32) as u8 } else { 0 };

    // Message
    let _ = write!(BootWriter, "[{:3}%] {}\n", pct, msg);

    // Barre de progression
    if let Some(fb) = try_get_framebuffer() {
        draw_progress_bar(&fb, pct);
    }
}
