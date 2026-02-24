//! panic.rs — Handler de panique du bootloader Exo-OS.
//!
//! Contexte : Exo-boot est un binaire `no_std`. Un panic en bootloader est
//! toujours fatal : il n'y a pas de système d'exploitation pour récupérer.
//!
//! Comportement :
//!   UEFI  → Affiche le message via ConOut (EFI Simple Text Output Protocol),
//!             puis appelle EFI RuntimeServices::ResetSystem(EfiResetShutdown).
//!   BIOS  → Écrit en VGA 80×25, puis HLT en boucle.
//!
//! RÈGLE ABSOLUTE (DOC10/BOOT-06) :
//!   Après ExitBootServices, les boot services EFI ne sont plus disponibles.
//!   Le handler utilise alors directement le framebuffer GOP pour afficher
//!   l'erreur, sans passer par les boot services.

#[cfg(not(feature = "uefi-boot"))]
use core::panic::PanicInfo;
#[cfg(not(feature = "uefi-boot"))]
use core::sync::atomic::{AtomicBool, Ordering};

/// Verrou pour éviter la récursion de panique (double-panic → halt immédiat).
#[cfg(not(feature = "uefi-boot"))]
static PANICKING: AtomicBool = AtomicBool::new(false);

/// Métrique : nombre de panics depuis le démarrage (pour les logs de diagnostic).
#[cfg(not(feature = "uefi-boot"))]
static PANIC_COUNT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

// ─── Point d'entrée du panic handler ──────────────────────────────────────────

// En mode UEFI, uefi_services fournit son propre #[panic_handler].
// On ne définit le nôtre que pour le chemin BIOS (no_std sans uefi_services).
#[cfg(not(feature = "uefi-boot"))]
#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    // ── Protection récursion ───────────────────────────────────────────────────
    if PANICKING.swap(true, Ordering::SeqCst) {
        // Double-panic : arrêt immédiat sans tenter d'afficher quoi que ce soit.
        halt_forever();
    }
    PANIC_COUNT.fetch_add(1, Ordering::Relaxed);

    // ── Collecte des informations de panique ───────────────────────────────────
    let location = info.location();
    let message  = info.message();

    // ── Tentative d'affichage ──────────────────────────────────────────────────
    // On tente d'abord UEFI ConOut (disponible avant ExitBootServices),
    // puis le framebuffer GOP (toujours disponible),
    // enfin VGA 80×25 en dernier recours.
    #[cfg(feature = "uefi-boot")]
    unsafe {
        try_display_panic_uefi(message, location);
    }

    #[cfg(feature = "bios-boot")]
    unsafe {
        try_display_panic_bios(message, location);
    }

    halt_forever()
}

// ─── Affichage UEFI (utilisé uniquement dans le chemin BIOS via cfg) ─────────

/// Tente d'afficher la panique via les mécanismes UEFI disponibles.
///
/// SAFETY : Appelé depuis le panic handler — aucun lock ne peut être tenu.
/// On accède aux globaux UEFI en mode "best effort" sans verrouillage.
#[cfg(not(feature = "uefi-boot"))]
unsafe fn try_display_panic_uefi(
    message: core::fmt::Arguments<'_>,
    location: Option<&core::panic::Location<'_>>,
) {
    // Écriture via le framebuffer GOP (toujours disponible même après ExitBootServices)
    if let Some(_fb) = crate::display::framebuffer::try_get_framebuffer() {
        let mut writer = crate::display::framebuffer::PanicWriter;
        panic_format_header(&mut writer);
        let _ = core::fmt::write(&mut writer, message);
        if let Some(loc) = location {
            panic_format_location(&mut writer, loc);
        }
        panic_format_footer(&mut writer);
    }
}

// ─── Affichage BIOS ───────────────────────────────────────────────────────────

/// Affichage via VGA 80×25 en mode texte (BIOS legacy).
///
/// SAFETY : Accès direct à l'adresse MMIO VGA 0xB8000.
#[cfg(feature = "bios-boot")]
unsafe fn try_display_panic_bios(
    message: core::fmt::Arguments<'_>,
    location: Option<&core::panic::Location<'_>>,
) {
    use crate::bios::vga::{VgaWriter, Color};
    let mut vga = VgaWriter::new_at_row(0, Color::LightRed, Color::Black);
    let _ = vga.write_str("[EXOBOOT PANIC] ");
    let _ = core::fmt::write(&mut vga, message);
    if let Some(loc) = location {
        let _ = write!(vga, " @ {}:{}", loc.file(), loc.line());
    }
}

// ─── Helpers formatage ────────────────────────────────────────────────────────

#[cfg(not(feature = "uefi-boot"))]
fn panic_format_header(w: &mut impl core::fmt::Write) {
    let _ = w.write_str(
        "══════════════════════════════════════════════════\n\
         ██  EXO-BOOT PANIC — SYSTÈME HALTÉ               ██\n\
         ══════════════════════════════════════════════════\n",
    );
}

#[cfg(not(feature = "uefi-boot"))]
fn panic_format_location(w: &mut impl core::fmt::Write, loc: &core::panic::Location<'_>) {
    let _ = write!(
        w,
        "\nLocalisation : {}:{}:{}\n",
        loc.file(), loc.line(), loc.column()
    );
}

#[cfg(not(feature = "uefi-boot"))]
fn panic_format_footer(w: &mut impl core::fmt::Write) {
    let _ = w.write_str(
        "\nVeuillez redémarrer le système.\n\
         ══════════════════════════════════════════════════\n",
    );
}

// ─── Arrêt définitif ──────────────────────────────────────────────────────────

/// Arrêt permanent du CPU. Ne retourne jamais.
///
/// Sur x86_64 : désactive les interruptions (`cli`) puis boucle sur `hlt`.
/// La boucle est nécessaire car des NMI (Non-Maskable Interrupts) peuvent
/// sortir d'un `hlt` même avec les interruptions masquées.
#[cfg(not(feature = "uefi-boot"))]
#[inline(never)]
#[cold]
fn halt_forever() -> ! {
    loop {
        unsafe {
            core::arch::asm!("cli; hlt", options(nomem, nostack));
        }
    }
}
