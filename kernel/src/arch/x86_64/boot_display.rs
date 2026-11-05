//! # boot_display.rs — Visible Boot Facade (VGA + Framebuffer)
//!
//! Keeps the historical VGA fallback and switches to the framebuffer as soon as
//! `arch_boot_init()` provides reliable screen information.

use core::sync::atomic::{AtomicUsize, Ordering};

use super::boot::early_init::BootInfo;
use super::{framebuffer_early, terminal, vga_early};

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

fn debug_line(prefix: &[u8], text: &str) {
    terminal::debug_write(prefix);
    terminal::debug_write(text.as_bytes());
    terminal::debug_write(b"\n");
}

fn draw_vga_shell() {
    vga_early::clear(vga_early::BLACK);

    let title = vga_early::attr(vga_early::WHITE, vga_early::BLUE);
    for col in 0..80 {
        // SAFETY: standard 80x25 VGA access via the identity-mapped text buffer.
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

/// Displays the initial boot screen.
pub fn boot_screen() {
    terminal::debug_write(b"boot_display: boot screen\n");
    draw_vga_shell();
}

/// Attaches the framebuffer console if the bootloader provides one.
pub fn attach_framebuffer(boot_info: &BootInfo) -> bool {
    let attached = framebuffer_early::init_from_boot_info(boot_info);
    if attached {
        terminal::debug_write(b"boot_display: framebuffer attached\n");
    } else {
        terminal::debug_write(b"boot_display: framebuffer unavailable\n");
    }
    attached
}

/// Displays progress for a genuinely initialized module.
pub fn stage_ok(label: &str) {
    debug_line(b"boot_display: stage ok ", label);
    let row = VGA_STAGE_ROW.fetch_add(1, Ordering::AcqRel);
    vga_early::set_cursor(2, row.min(22));
    vga_early::write_str(label, text_attr());
    vga_early::write_str(" ... ", text_attr());
    vga_early::write_str("[ OK ]", stage_attr());
    vga_early::write_char(b'\n', text_attr());

    framebuffer_early::stage_ok(label);
}

/// Closes the boot display.
pub fn boot_complete() {
    terminal::debug_write(b"boot_display: boot complete\n");
    vga_early::set_cursor(0, 23);
    vga_early::write_hline(vga_early::attr(vga_early::DARK_GRAY, vga_early::BLACK));
    vga_early::write_centered("[ Exo-OS boot complete ]", stage_attr());
    framebuffer_early::boot_complete();
}

/// Displays an explicit status for the kernel -> userspace transition.
pub fn userspace_status(title: &str, detail: &str, hint: &str) {
    debug_line(b"boot_display: userspace ", title);
    debug_line(b"boot_display: detail ", detail);
    debug_line(b"boot_display: hint ", hint);
    let warn_attr = vga_early::attr(vga_early::YELLOW, vga_early::BLACK);
    let text = text_attr();
    let accent = accent_attr();

    vga_early::set_cursor(0, 20);
    vga_early::write_hline(vga_early::attr(vga_early::DARK_GRAY, vga_early::BLACK));
    vga_early::write_centered(title, warn_attr);
    vga_early::write_str("  ", text);
    vga_early::write_str(detail, text);
    vga_early::write_char(b'\n', text);
    vga_early::write_str("  ", text);
    vga_early::write_str(hint, accent);

    framebuffer_early::userspace_status(title, detail, hint);
}
