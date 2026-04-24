//! # boot_display.rs — Façade de boot visible (VGA + framebuffer)
//!
//! Conserve le fallback VGA historique et bascule sur le framebuffer dès que
//! `arch_boot_init()` a rendu des infos d'écran fiables.

use core::sync::atomic::{AtomicUsize, Ordering};

use super::boot::early_init::BootInfo;
use super::{framebuffer_early, vga_early};

static VGA_STAGE_ROW: AtomicUsize = AtomicUsize::new(9);

fn stage_attr() -> u8 {
    vga_early::attr(vga_early::LIGHT_GREEN, vga_early::BLACK)
}

fn text_attr() -> u8 {
    vga_early::attr(vga_early::LIGHT_GRAY, vga_early::BLACK)
}

fn accent_attr() -> u8 {
    vga_early::attr(vga_early::LIGHT_CYAN, vga_early::BLACK)
}

fn draw_vga_shell() {
    vga_early::clear(vga_early::BLACK);

    let title = vga_early::attr(vga_early::WHITE, vga_early::BLUE);
    for col in 0..80 {
        // SAFETY: accès VGA 80x25 classique via le buffer texte identity-mappé.
        unsafe {
            let offset = col * 2;
            let ptr = (0xB8000 + offset) as *mut u8;
            core::ptr::write_volatile(ptr, b' ');
            core::ptr::write_volatile(ptr.add(1), title);
        }
    }

    vga_early::set_cursor(0, 0);
    vga_early::write_centered("  Exo-OS  --  Boot verification path  ", title);
    vga_early::write_char(b'\n', text_attr());
    vga_early::write_str(
        "  VGA fallback active. Framebuffer will attach after arch_boot_init.\n",
        accent_attr(),
    );
    vga_early::write_str(
        "  Module progress below follows the real init sequence.\n\n",
        accent_attr(),
    );
    vga_early::write_hline(vga_early::attr(vga_early::DARK_GRAY, vga_early::BLACK));
    vga_early::write_str("  Waiting for architecture handoff...\n\n", text_attr());
    vga_early::write_str("  Modules:\n", accent_attr());
    VGA_STAGE_ROW.store(9, Ordering::Release);
}

/// Affiche l'écran de boot initial.
pub fn boot_screen() {
    draw_vga_shell();
}

/// Attache la console framebuffer si le bootloader en fournit une.
pub fn attach_framebuffer(boot_info: &BootInfo) -> bool {
    framebuffer_early::init_from_boot_info(boot_info)
}

/// Affiche l'avancement d'un module réellement initialisé.
pub fn stage_ok(label: &str) {
    let row = VGA_STAGE_ROW.fetch_add(1, Ordering::AcqRel);
    vga_early::set_cursor(2, row.min(22));
    vga_early::write_str(label, text_attr());
    vga_early::write_str(" ... ", text_attr());
    vga_early::write_str("[ OK ]", stage_attr());
    vga_early::write_char(b'\n', text_attr());

    framebuffer_early::stage_ok(label);
}

/// Clôt l'affichage de boot.
pub fn boot_complete() {
    vga_early::set_cursor(0, 23);
    vga_early::write_hline(vga_early::attr(vga_early::DARK_GRAY, vga_early::BLACK));
    vga_early::write_centered("[ Exo-OS boot complete ]", stage_attr());
    framebuffer_early::boot_complete();
}
