//! Petit utilitaire d'affichage VGA placé dans `libutils`.
//!
//! Fournit un fallback simple qui écrit directement dans le buffer texte VGA
//! (0xb8000). Conçu pour être utilisé très tôt dans le boot sans allocation.

use core::sync::atomic::{AtomicU8, Ordering};

/// Dimensions du mode texte VGA
const WIDTH: usize = 80;
const HEIGHT: usize = 25;
const BUFFER_ADDR: usize = 0xb8000;

/// Couleurs d'avant-plan
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Color {
    Black = 0x0,
    Blue = 0x1,
    Green = 0x2,
    Cyan = 0x3,
    Red = 0x4,
    Magenta = 0x5,
    Brown = 0x6,
    LightGray = 0x7,
    DarkGray = 0x8,
    LightBlue = 0x9,
    LightGreen = 0xa,
    LightCyan = 0xb,
    LightRed = 0xc,
    LightMagenta = 0xd,
    Yellow = 0xe,
    White = 0xf,
}

static FG_COLOR: AtomicU8 = AtomicU8::new(Color::LightGray as u8);

#[inline(always)]
fn attr_byte(fg: u8) -> u8 {
    // background = 0 (black), foreground = fg
    fg & 0x0f
}

/// Efface l'écran en utilisant la couleur d'avant-plan courante.
pub fn clear_screen() {
    let start = crate::perf_counters::rdtsc();
    
    let fg = FG_COLOR.load(Ordering::SeqCst);
    let attr = attr_byte(fg);
    let buf = BUFFER_ADDR as *mut u8;

    for row in 0..HEIGHT {
        for col in 0..WIDTH {
            let idx = (row * WIDTH + col) * 2;
            unsafe {
                core::ptr::write_volatile(buf.add(idx), b' ');
                core::ptr::write_volatile(buf.add(idx + 1), attr);
            }
        }
    }
    
    let end = crate::perf_counters::rdtsc();
    crate::perf_counters::PERF_MANAGER.record(crate::perf_counters::Component::Vga, end - start);
}

/// Définit la couleur d'avant-plan pour les écritures suivantes
pub fn set_color(c: Color) {
    FG_COLOR.store(c as u8, Ordering::SeqCst);
}

/// Écrit une chaîne ASCII à la position fournie (ligne/colonne)
pub fn write_str_at(row: usize, col: usize, s: &str) {
    let start = crate::perf_counters::rdtsc();
    
    if row >= HEIGHT || col >= WIDTH {
        let end = crate::perf_counters::rdtsc();
        crate::perf_counters::PERF_MANAGER.record(crate::perf_counters::Component::Vga, end - start);
        return;
    }
    let fg = FG_COLOR.load(Ordering::SeqCst);
    let attr = attr_byte(fg);
    let buf = BUFFER_ADDR as *mut u8;

    let mut c = col;
    for &b in s.as_bytes() {
        if c >= WIDTH {
            break;
        }
        let ch = if b.is_ascii() { b } else { b'?' };
        let idx = (row * WIDTH + c) * 2;
        unsafe {
            core::ptr::write_volatile(buf.add(idx), ch);
            core::ptr::write_volatile(buf.add(idx + 1), attr);
        }
        c += 1;
    }
    
    let end = crate::perf_counters::rdtsc();
    crate::perf_counters::PERF_MANAGER.record(crate::perf_counters::Component::Vga, end - start);
}

/// Écrit la chaîne centrée horizontalement sur la ligne donnée
pub fn write_centered(row: usize, s: &str) {
    let start = crate::perf_counters::rdtsc();
    
    let len = s.len();
    let start_col = if len >= WIDTH { 0 } else { (WIDTH - len) / 2 };
    write_str_at(row, start_col, s);
    
    let end = crate::perf_counters::rdtsc();
    crate::perf_counters::PERF_MANAGER.record(crate::perf_counters::Component::Vga, end - start);
}

/// Écrit un petit banner au centre de l'écran (utilisé comme fallback)
pub fn write_banner() {
    let start = crate::perf_counters::rdtsc();

    // Nettoyer l'écran et afficher un bandeau d'accueil lisible
    clear_screen();

    // Titre principal
    set_color(Color::White);
    write_centered(7, "EXO-OS 64");

    // Sous-titre / version
    set_color(Color::LightGreen);
    write_centered(9, "Kernel v0.2.0-PHASE8-BOOT");

    // Informations essentielles
    set_color(Color::LightGray);
    write_centered(11, "Arch: x86_64");
    write_centered(12, "Boot: Multiboot2 + GRUB");

    // Laisser la couleur par défaut en blanc
    set_color(Color::White);

    let end = crate::perf_counters::rdtsc();
    crate::perf_counters::PERF_MANAGER.record(crate::perf_counters::Component::Vga, end - start);
}

/// Écrit un entier décimal à la position donnée sans allocation.
/// Retourne le nombre de chiffres écrits.
pub fn write_decimal_at(row: usize, mut col: usize, mut num: u64) -> usize {
    if row >= HEIGHT || col >= WIDTH {
        return 0;
    }
    // Buffer local pour stocker les chiffres en ordre inverse (u64 max = 20 chiffres)
    let mut tmp = [0u8; 20];
    let mut i = 0usize;
    if num == 0 {
        tmp[0] = b'0';
        i = 1;
    } else {
        while num > 0 && i < tmp.len() {
            let d = (num % 10) as u8;
            tmp[i] = b'0' + d;
            num /= 10;
            i += 1;
        }
    }

    // Écrire les chiffres dans l'ordre correct
    let fg = FG_COLOR.load(Ordering::SeqCst);
    let attr = attr_byte(fg);
    let buf = BUFFER_ADDR as *mut u8;

    let mut written = 0usize;
    let mut j = i;
    while j > 0 && col < WIDTH {
        j -= 1;
        let ch = tmp[j];
        let idx = (row * WIDTH + col) * 2;
        unsafe {
            core::ptr::write_volatile(buf.add(idx), ch);
            core::ptr::write_volatile(buf.add(idx + 1), attr);
        }
        col += 1;
        written += 1;
    }
    written
}

/// Écrit une ligne de statut de la forme "<label>: OK/FAIL". Couleur du label claire, et OK en vert.
pub fn write_status_line(row: usize, label: &str, ok: bool) {
    // Label
    set_color(Color::LightGray);
    write_str_at(row, 2, label);
    let mut col = 2 + label.len();
    if col + 2 < WIDTH {
        write_str_at(row, col, ": ");
        col += 2;
    }

    // Valeur
    if ok {
        set_color(Color::LightGreen);
        write_str_at(row, col, "OK");
    } else {
        set_color(Color::LightRed);
        write_str_at(row, col, "FAIL");
    }

    // Restaurer
    set_color(Color::White);
}

/// Affiche sous la bannière les informations utiles de boot et statuts clés
pub fn write_boot_status(mem_mb: u64, heap_ok: bool, scheduler_ok: bool, ipc_ok: bool) {
    // Mémoire utilisable
    set_color(Color::LightCyan);
    let label = "Mémoire utilisable: ";
    write_str_at(14, 2, label);
    set_color(Color::LightCyan);
    let digits = write_decimal_at(14, 2 + label.len(), mem_mb);
    write_str_at(14, 2 + label.len() + digits, " MB");

    // Statuts
    write_status_line(15, "Heap", heap_ok);
    write_status_line(16, "Scheduler", scheduler_ok);
    write_status_line(17, "IPC", ipc_ok);

    // Restaurer la couleur
    set_color(Color::White);
}
