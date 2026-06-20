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

    // ── Affichage VGA 80×25 (BIOS legacy) ──────────────────────────────────────
    // SAFETY : bootloader BIOS mono-cœur pré-kernel — aucun lock tenu, MMIO valide.
    unsafe {
        try_display_panic_bios(info);
    }

    halt_forever()
}

// ─── Affichage BIOS ───────────────────────────────────────────────────────────

/// Affiche la panique via VGA 80×25 en mode texte (BIOS legacy).
///
/// `PanicInfo` implémente `Display` (message + localisation), donc un seul
/// `write!` suffit — pas besoin d'extraire `message()`/`location()` séparément
/// (l'API `message()` ne renvoie plus `core::fmt::Arguments`).
///
/// SAFETY : Accès direct au framebuffer VGA texte (0xB8000) — Ring 0, mono-cœur.
#[cfg(not(feature = "uefi-boot"))]
unsafe fn try_display_panic_bios(info: &PanicInfo<'_>) {
    use crate::bios::vga::{Color, VgaWriter};
    use core::fmt::Write as _;
    let mut vga = VgaWriter::new_at_row(0, Color::LightRed, Color::Black);
    panic_format_header(&mut vga);
    let _ = write!(vga, "[EXOBOOT PANIC] {}", info);
    panic_format_footer(&mut vga);
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
