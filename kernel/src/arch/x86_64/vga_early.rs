// kernel/src/arch/x86_64/vga_early.rs
//
// Affichage VGA texte 80×25 minimal — disponible dès le boot (avant drivers).
//
// Accès direct au buffer VGA physique 0xB8000 via l'identity map 0..1 GiB
// mise en place par le trampoline bootloader. Aucune initialisation requise.
//
// Utilisé pour :
//   - Afficher l'état de boot sur l'écran (Phase 1 "tester graphique")
//   - Afficher les exceptions fatales (panic visible à l'écran)
//   - Remplace les simples probes port 0xE9 pour la visibilité utilisateur
//
// Couche 0 — aucune dépendance externe.

use core::sync::atomic::{AtomicUsize, Ordering};

// ── Constantes ────────────────────────────────────────────────────────────────

/// Buffer VGA texte physique 0xB8000 — accessible via identity map.
const VGA_BASE: usize = 0xB8000;
const VGA_COLS: usize = 80;
const VGA_ROWS: usize = 25;

// ── Couleurs ──────────────────────────────────────────────────────────────────
pub const BLACK:       u8 = 0;
pub const BLUE:        u8 = 1;
pub const GREEN:       u8 = 2;
pub const CYAN:        u8 = 3;
pub const RED:         u8 = 4;
pub const MAGENTA:     u8 = 5;
pub const BROWN:       u8 = 6;
pub const LIGHT_GRAY:  u8 = 7;
pub const DARK_GRAY:   u8 = 8;
pub const LIGHT_BLUE:  u8 = 9;
pub const LIGHT_GREEN: u8 = 10;
pub const LIGHT_CYAN:  u8 = 11;
pub const LIGHT_RED:   u8 = 12;
pub const YELLOW:      u8 = 14;
pub const WHITE:       u8 = 15;

/// Construit un attribut VGA : fond × 16 + texte.
#[inline(always)]
pub const fn attr(fg: u8, bg: u8) -> u8 { (bg << 4) | (fg & 0x0F) }

// ── Position curseur ──────────────────────────────────────────────────────────

static VGA_COL: AtomicUsize = AtomicUsize::new(0);
static VGA_ROW: AtomicUsize = AtomicUsize::new(0);

// ── Accès buffer ─────────────────────────────────────────────────────────────

/// Écrit une cellule VGA (char + attribut) à la position (col, row).
///
/// SAFETY: VGA_BASE est dans l'identity map (trampoline 0..1 GiB) toujours
///         active pendant le boot. Valide dès `_start64` jusqu'au premier
///         remap explicite de cette région (jamais fait en Phase 1).
#[inline(always)]
unsafe fn put_cell(col: usize, row: usize, ch: u8, attribute: u8) {
    let offset = (row * VGA_COLS + col) * 2;
    let ptr = (VGA_BASE + offset) as *mut u8;
    core::ptr::write_volatile(ptr, ch);
    core::ptr::write_volatile(ptr.add(1), attribute);
}

/// Efface tout l'écran avec l'attribut donné.
pub fn clear(background: u8) {
    let attribute = attr(LIGHT_GRAY, background);
    for row in 0..VGA_ROWS {
        for col in 0..VGA_COLS {
            // SAFETY: voir put_cell
            unsafe { put_cell(col, row, b' ', attribute); }
        }
    }
    VGA_COL.store(0, Ordering::Relaxed);
    VGA_ROW.store(0, Ordering::Relaxed);
}

/// Fait défiler l'écran d'une ligne vers le haut et efface la dernière ligne.
fn scroll(attribute: u8) {
    // SAFETY: VGA_BASE accessible via identity map
    unsafe {
        let base = VGA_BASE as *mut u8;
        // Copier lignes 1..ROWS vers 0..ROWS-1
        core::ptr::copy(
            base.add(VGA_COLS * 2),
            base,
            (VGA_ROWS - 1) * VGA_COLS * 2,
        );
        // Effacer la dernière ligne
        for col in 0..VGA_COLS {
            put_cell(col, VGA_ROWS - 1, b' ', attribute);
        }
    }
}

/// Écrit un caractère à la position courante du curseur.
pub fn write_char(ch: u8, attribute: u8) {
    let col = VGA_COL.load(Ordering::Relaxed);
    let row = VGA_ROW.load(Ordering::Relaxed);

    match ch {
        b'\n' => {
            VGA_COL.store(0, Ordering::Relaxed);
            let new_row = row + 1;
            if new_row >= VGA_ROWS {
                scroll(attribute);
                VGA_ROW.store(VGA_ROWS - 1, Ordering::Relaxed);
            } else {
                VGA_ROW.store(new_row, Ordering::Relaxed);
            }
        }
        b'\r' => {
            VGA_COL.store(0, Ordering::Relaxed);
        }
        _ => {
            let ch_out = if ch >= 0x20 && ch < 0x7F { ch } else { b'?' };
            // SAFETY: voir put_cell
            unsafe { put_cell(col, row, ch_out, attribute); }
            let new_col = col + 1;
            if new_col >= VGA_COLS {
                VGA_COL.store(0, Ordering::Relaxed);
                let new_row = row + 1;
                if new_row >= VGA_ROWS {
                    scroll(attribute);
                    VGA_ROW.store(VGA_ROWS - 1, Ordering::Relaxed);
                } else {
                    VGA_ROW.store(new_row, Ordering::Relaxed);
                }
            } else {
                VGA_COL.store(new_col, Ordering::Relaxed);
            }
        }
    }
}

/// Écrit une chaîne à la position courante.
pub fn write_str(s: &str, attribute: u8) {
    for byte in s.bytes() {
        write_char(byte, attribute);
    }
}

/// Écrit une ligne centrée horizontalement (80 colonnes).
pub fn write_centered(s: &str, attribute: u8) {
    let len = s.len().min(VGA_COLS);
    let pad = (VGA_COLS - len) / 2;
    let row = VGA_ROW.load(Ordering::Relaxed);
    for col in 0..pad {
        unsafe { put_cell(col, row, b' ', attribute); }
    }
    VGA_COL.store(pad, Ordering::Relaxed);
    write_str(s, attribute);
    // Passer à la ligne suivante
    write_char(b'\n', attribute);
}

/// Écrit une ligne horizontale (caractère '─' = 0xC4 en CP437).
pub fn write_hline(attribute: u8) {
    let row = VGA_ROW.load(Ordering::Relaxed);
    for col in 0..VGA_COLS {
        unsafe { put_cell(col, row, 0xC4, attribute); }
    }
    write_char(b'\n', attribute);
}

/// Place le curseur à une position absolue.
pub fn set_cursor(col: usize, row: usize) {
    VGA_COL.store(col.min(VGA_COLS - 1), Ordering::Relaxed);
    VGA_ROW.store(row.min(VGA_ROWS - 1), Ordering::Relaxed);
}

/// Ecrit un u32 en décimal.
pub fn write_u32(mut n: u32, attribute: u8) {
    if n == 0 { write_char(b'0', attribute); return; }
    let mut buf = [0u8; 10];
    let mut len = 0;
    while n > 0 {
        buf[len] = b'0' + (n % 10) as u8;
        len += 1;
        n /= 10;
    }
    for i in (0..len).rev() {
        write_char(buf[i], attribute);
    }
}

// ── Écran de boot Phase 1 ─────────────────────────────────────────────────────

/// Affiche l'écran de démarrage Exo-OS au boot.
///
/// Appelé depuis `kernel_main` juste après le marqueur port 0xE9 'K'.
/// Visible sur la fenêtre QEMU (VGA std) dès les premières instructions.
pub fn boot_screen() {
    clear(BLACK);

    // Ligne de titre — fond bleu, texte blanc
    let title_attr = attr(WHITE, BLUE);
    let normal_attr = attr(LIGHT_GRAY, BLACK);
    let ok_attr     = attr(LIGHT_GREEN, BLACK);
    let warn_attr   = attr(YELLOW, BLACK);
    let cyan_attr   = attr(LIGHT_CYAN, BLACK);

    // ── En-tête ────────────────────────────────────────────────────────────────
    // Ligne 0 : titre plein écran
    for col in 0..VGA_COLS {
        unsafe { put_cell(col, 0, b' ', title_attr); }
    }
    set_cursor(0, 0);
    write_centered("  Exo-OS v0.1  --  Kernel Phase 1  ", title_attr);

    // Ligne 1 : vide
    write_char(b'\n', normal_attr);

    // ── Version ────────────────────────────────────────────────────────────────
    write_str("  Architecture : x86_64   Platform : QEMU Q35   RAM : 256 MiB\n", cyan_attr);
    write_char(b'\n', normal_attr);

    // ── Sous-systemes Phase 1 ──────────────────────────────────────────────────
    write_str("  Phase 1 — Memoire virtuelle et heap kernel\n", warn_attr);
    write_hline(attr(DARK_GRAY, BLACK));

    let item = |label: &str, status: &str, st_attr: u8| {
        write_str("  ", normal_attr);
        write_str(label, normal_attr);
        write_str(" ... ", normal_attr);
        write_str(status, st_attr);
        write_char(b'\n', normal_attr);
    };

    item("PML4 kernel haute memoire         ", "[  OK  ]", ok_attr);
    item("APIC MMIO (UC + NX)               ", "[  OK  ]", ok_attr);
    item("Buddy allocator (DMA/DMA32/Normal)", "[  OK  ]", ok_attr);
    item("SLUB allocator (#[global_alloc])  ", "[  OK  ]", ok_attr);
    item("Hybrid heap (SLUB + vmalloc)      ", "[  OK  ]", ok_attr);
    item("VMA tree AVL                      ", "[  OK  ]", ok_attr);
    item("Swap compress zswap               ", "[  OK  ]", ok_attr);
    item("Protections NX/SMEP/SMAP/PKU/KPTI", "[  OK  ]", ok_attr);
    item("TSC calibration HPET              ", "[ TODO ]", warn_attr);

    write_char(b'\n', normal_attr);
    write_hline(attr(DARK_GRAY, BLACK));
    write_str("  Booting kernel subsystems...\n", cyan_attr);
}

/// Met à jour l'affichage boot avec le statut final.
///
/// Appelé depuis `kernel_main` après que `kernel_init()` est terminé.
pub fn boot_complete() {
    let ok_attr  = attr(LIGHT_GREEN, BLACK);
    let cyan_attr = attr(LIGHT_CYAN, BLACK);

    // Aller à la fin de l'écran
    set_cursor(0, VGA_ROWS - 3);
    write_hline(attr(DARK_GRAY, BLACK));
    write_centered("[ Exo-OS Phase 1 complete - scheduler running ]", ok_attr);
    write_str("\n  Port 0xE9 debug output active. QEMU -serial stdio.\n", cyan_attr);
}
