//! vga.rs — Sortie texte VGA 80×25 (debug boot BIOS).
//!
//! Accès direct au buffer vidéo VGA en mode texte 80×25.
//! Le buffer est mappé en mémoire à l'adresse physique 0xB8000.
//!
//! Structure d'une cellule VGA (2 octets) :
//!   Octet bas  : caractère ASCII
//!   Octet haut : attribut (couleur fond × 16 + couleur texte)
//!
//! LIMITATION : Ce module utilise uniquement le jeu de caractères ASCII.
//! Les accents et caractères non-ASCII sont remplacés par '?'.
//!
//! Utilisé pour :
//!   - Afficher les messages d'état pendant le démarrage BIOS
//!   - Afficher les erreurs fatales (panique bootloader)
//!   - Indiquer la progression du chargement

use core::fmt;

// ─── Constantes ───────────────────────────────────────────────────────────────

/// Adresse physique du framebuffer VGA texte (0xB8000 — plan vidéo MMIO).
const VGA_BUFFER_BASE: usize = 0xB800_0000;

/// Dimensions de l'écran VGA texte standard.
const VGA_COLS: usize = 80;
const VGA_ROWS: usize = 25;

/// Nombre total de cellules dans le buffer VGA.
const VGA_CELLS: usize = VGA_COLS * VGA_ROWS;

/// Port I/O du contrôleur CRT VGA (pour positionner le curseur hardware).
const VGA_CRT_INDEX_PORT: u16 = 0x3D4;
const VGA_CRT_DATA_PORT:  u16 = 0x3D5;

// ─── Couleurs VGA ─────────────────────────────────────────────────────────────

/// Couleurs disponibles en mode texte VGA (4 bits).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Color {
    Black         = 0,
    Blue          = 1,
    Green         = 2,
    Cyan          = 3,
    Red           = 4,
    Magenta       = 5,
    Brown         = 6,
    LightGray     = 7,
    DarkGray      = 8,
    LightBlue     = 9,
    LightGreen    = 10,
    LightCyan     = 11,
    LightRed      = 12,
    LightMagenta  = 13,
    Yellow        = 14,
    White         = 15,
}

impl Color {
    /// Construit un attribut VGA (fond × 16 + texte).
    #[inline]
    pub fn attr(foreground: Color, background: Color) -> u8 {
        (background as u8) << 4 | (foreground as u8)
    }
}

// ─── VgaWriter ────────────────────────────────────────────────────────────────

/// Écrivain VGA texte 80×25.
///
/// Gère automatiquement :
/// - Le retour à la ligne (`\n`)
/// - Le retour chariot (`\r`)
/// - Le défilement quand le curseur dépasse la ligne 24
/// - La colorisation via attribut VGA
/// - Le positionnement Hardware du curseur
pub struct VgaWriter {
    /// Attribut courant (fond + texte).
    attr:    u8,
    /// Colonne courante (0–79).
    col:     usize,
    /// Ligne courante (0–24).
    row:     usize,
    /// Pointeur vers le buffer VGA 80×25 (MMIO physique).
    buffer:  *mut u16,
}

// SAFETY : VgaWriter accède à du MMIO physique. En bootloader BIOS mono-cœur
// pré-kernel, il n'y a pas de concurrence. Le pointeur est toujours valide.
unsafe impl Send for VgaWriter {}

impl VgaWriter {
    /// Crée un VgaWriter en début d'écran (ligne 0, colonne 0).
    pub fn new(fg: Color, bg: Color) -> Self {
        let mut w = Self {
            attr:   Color::attr(fg, bg),
            col:    0,
            row:    0,
            // SAFETY : 0xB8000 est l'adresse standard du framebuffer VGA texte.
            buffer: VGA_BUFFER_BASE as *mut u16,
        };
        w.clear();
        w
    }

    /// Crée un VgaWriter positionné à une ligne spécifique (pour le panic handler).
    pub fn new_at_row(row: usize, fg: Color, bg: Color) -> Self {
        Self {
            attr:   Color::attr(fg, bg),
            col:    0,
            row:    row.min(VGA_ROWS - 1),
            buffer: VGA_BUFFER_BASE as *mut u16,
        }
    }

    /// Efface l'écran avec l'attribut courant (caractère espace).
    pub fn clear(&mut self) {
        let blank = self.make_cell(b' ');
        for i in 0..VGA_CELLS {
            // SAFETY : i < VGA_CELLS = 2000, buffer est le framebuffer VGA valide.
            unsafe { self.buffer.add(i).write_volatile(blank); }
        }
        self.col = 0;
        self.row = 0;
        self.move_cursor();
    }

    /// Définit la couleur de texte.
    #[inline]
    pub fn set_color(&mut self, fg: Color, bg: Color) {
        self.attr = Color::attr(fg, bg);
    }

    /// Affiche une ligne horizontale (caractère '─' passé en ASCII '-').
    pub fn draw_separator(&mut self) {
        for _ in 0..VGA_COLS {
            self.write_byte(b'-');
        }
    }

    // ── Écriture d'un byte ──────────────────────────────────────────────────

    fn write_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => {
                self.col = 0;
                self.next_row();
            }
            b'\r' => {
                self.col = 0;
            }
            b'\t' => {
                // Tabulation : aligne sur la prochaine colonne multiple de 8
                let next_tab = (self.col + 8) & !7;
                while self.col < next_tab && self.col < VGA_COLS {
                    self.write_printable(b' ');
                }
            }
            byte => {
                // Remplace les non-ASCII (accents, etc.) par '?'
                let safe_byte = if byte.is_ascii_graphic() || byte == b' ' { byte } else { b'?' };
                self.write_printable(safe_byte);
            }
        }
    }

    fn write_printable(&mut self, byte: u8) {
        if self.col >= VGA_COLS {
            self.col = 0;
            self.next_row();
        }
        let idx = self.row * VGA_COLS + self.col;
        // SAFETY : idx < VGA_CELLS car row < VGA_ROWS et col < VGA_COLS.
        unsafe {
            self.buffer.add(idx).write_volatile(self.make_cell(byte));
        }
        self.col += 1;
    }

    fn next_row(&mut self) {
        if self.row + 1 < VGA_ROWS {
            self.row += 1;
        } else {
            // Scroll : déplace les lignes 1…24 vers 0…23 et efface la ligne 24
            self.scroll_up();
        }
        self.move_cursor();
    }

    fn scroll_up(&mut self) {
        // Copie les lignes 1..24 vers 0..23
        for r in 0..(VGA_ROWS - 1) {
            for c in 0..VGA_COLS {
                let src = (r + 1) * VGA_COLS + c;
                let dst = r * VGA_COLS + c;
                // SAFETY : src et dst < VGA_CELLS.
                unsafe {
                    let val = self.buffer.add(src).read_volatile();
                    self.buffer.add(dst).write_volatile(val);
                }
            }
        }
        // Efface la dernière ligne
        let blank = self.make_cell(b' ');
        for c in 0..VGA_COLS {
            let idx = (VGA_ROWS - 1) * VGA_COLS + c;
            // SAFETY : idx < VGA_CELLS.
            unsafe { self.buffer.add(idx).write_volatile(blank); }
        }
        // Le row reste à VGA_ROWS - 1 après un scroll
        self.row = VGA_ROWS - 1;
    }

    /// Crée une cellule VGA encodée : attribut (octet haut) + caractère (octet bas).
    #[inline]
    fn make_cell(&self, ch: u8) -> u16 {
        (self.attr as u16) << 8 | ch as u16
    }

    /// Déplace le curseur hardware VGA à la position courante.
    ///
    /// Utilise les ports I/O du contrôleur CRT VGA (index 0x0E = cursor high, 0x0F = cursor low).
    fn move_cursor(&self) {
        let pos = (self.row * VGA_COLS + self.col) as u16;
        // SAFETY : Ports I/O VGA standard, accès direct en Ring 0 bootloader.
        unsafe {
            outb(VGA_CRT_INDEX_PORT, 0x0F);         // Cursor location low
            outb(VGA_CRT_DATA_PORT, (pos & 0xFF) as u8);
            outb(VGA_CRT_INDEX_PORT, 0x0E);         // Cursor location high
            outb(VGA_CRT_DATA_PORT, ((pos >> 8) & 0xFF) as u8);
        }
    }
}

// ─── Implémentation fmt::Write ────────────────────────────────────────────────

impl fmt::Write for VgaWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            self.write_byte(byte);
        }
        self.move_cursor();
        Ok(())
    }
}

// ─── I/O Port helpers ─────────────────────────────────────────────────────────

/// Écrit un byte sur un port I/O x86.
///
/// SAFETY : Uniquement en Ring 0 (bootloader, avant passage au mode utilisateur).
#[inline]
unsafe fn outb(port: u16, val: u8) {
    // SAFETY : la fonction est déjà unsafe, mais unsafe_op_in_unsafe_fn requiert un bloc explicite.
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") port,
            in("al") val,
            options(nomem, nostack)
        );
    }
}
