//! Lightweight 2D Minesweeper (tests left-click reveal, right-click flags, flood-fill).

use crate::assets::FontSet;
use crate::i18n::Lang;
use exo_ui::{Canvas, Color, Palette, Rect};

const COLS: usize = 10;
const ROWS: usize = 10;
const MINES: usize = 14;

#[derive(Clone, Copy, Default)]
struct Cell {
    mine: bool,
    revealed: bool,
    flagged: bool,
    adj: u8,
}

pub struct MinesweeperGame {
    cells: [Cell; COLS * ROWS],
    pub won: bool,
    pub lost: bool,
}

impl MinesweeperGame {
    pub fn new(seed: u64) -> Self {
        let mut cells = [Cell::default(); COLS * ROWS];
        let mut rng = seed ^ 0x9E37_79B9_7F4A_7C15;
        let mut placed = 0usize;
        while placed < MINES {
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let idx = ((rng >> 20) as usize) % (COLS * ROWS);
            if !cells[idx].mine {
                cells[idx].mine = true;
                placed += 1;
            }
        }
        for r in 0..ROWS {
            for c in 0..COLS {
                let mut cnt = 0u8;
                for dr in -1i32..=1 {
                    for dc in -1i32..=1 {
                        if dr == 0 && dc == 0 { continue; }
                        let nr = r as i32 + dr;
                        let nc = c as i32 + dc;
                        if nr >= 0 && nc >= 0 && (nr as usize) < ROWS && (nc as usize) < COLS {
                            if cells[(nr as usize) * COLS + (nc as usize)].mine { cnt += 1; }
                        }
                    }
                }
                cells[r * COLS + c].adj = cnt;
            }
        }
        Self { cells, won: false, lost: false }
    }

    fn board_geom(area: Rect) -> (i32, i32, i32) {
        let cell = ((area.w - 32) / COLS as i32).min((area.h - 52) / ROWS as i32).clamp(22, 38);
        let ox = area.x + (area.w - cell * COLS as i32) / 2;
        let oy = area.y + 36 + (area.h - 46 - cell * ROWS as i32) / 2;
        (ox, oy, cell)
    }

    pub fn click(&mut self, area: Rect, mx: i32, my: i32, right: bool) -> bool {
        if self.won || self.lost { return false; }
        let (ox, oy, cell) = Self::board_geom(area);
        let c = (mx - ox) / cell;
        let r = (my - oy) / cell;
        if mx < ox || my < oy || c < 0 || r < 0 || c as usize >= COLS || r as usize >= ROWS {
            return false;
        }
        let idx = (r as usize) * COLS + (c as usize);
        if right {
            if !self.cells[idx].revealed { self.cells[idx].flagged = !self.cells[idx].flagged; }
            return true;
        }
        if self.cells[idx].flagged || self.cells[idx].revealed { return false; }
        if self.cells[idx].mine {
            self.lost = true;
            for cell in &mut self.cells { if cell.mine { cell.revealed = true; } }
            return true;
        }
        self.flood_reveal(r as usize, c as usize);
        let safe_left = self.cells.iter().filter(|c| !c.mine && !c.revealed).count();
        if safe_left == 0 { self.won = true; }
        true
    }

    fn flood_reveal(&mut self, sr: usize, sc: usize) {
        let mut stack = vec![(sr, sc)];
        while let Some((r, c)) = stack.pop() {
            let idx = r * COLS + c;
            if self.cells[idx].revealed || self.cells[idx].flagged { continue; }
            self.cells[idx].revealed = true;
            if self.cells[idx].adj == 0 {
                for dr in -1i32..=1 {
                    for dc in -1i32..=1 {
                        let nr = r as i32 + dr;
                        let nc = c as i32 + dc;
                        if nr >= 0 && nc >= 0 && (nr as usize) < ROWS && (nc as usize) < COLS {
                            stack.push((nr as usize, nc as usize));
                        }
                    }
                }
            }
        }
    }

    pub fn draw(&self, cv: &mut Canvas<'_>, area: Rect, mx: i32, my: i32, pal: &Palette, lang: Lang, fonts: &FontSet) {
        let flags = self.cells.iter().filter(|c| c.flagged).count();
        let status = if self.won {
            lang.pick("Victoire ! Toutes les mines évitées.", "Victory! All mines cleared.")
        } else if self.lost {
            lang.pick("Mine touchée ! Cliquez sur Rejouer.", "Boom! Click Restart.")
        } else {
            lang.pick("Clic gauche : révéler  •  Clic droit : drapeau", "Left-click: reveal • Right-click: flag")
        };
        cv.draw_text(&fonts.sm, area.x + 16, area.y + 10, &format!("Mines : {}/{}   •   {}", flags, MINES, status), pal.text_secondary);

        let (ox, oy, cell) = Self::board_geom(area);
        for r in 0..ROWS {
            for c in 0..COLS {
                let it = self.cells[r * COLS + c];
                let rect = Rect::new(ox + (c as i32) * cell + 2, oy + (r as i32) * cell + 2, cell - 4, cell - 4);
                let hov = rect.contains(mx, my);
                if it.revealed {
                    cv.fill_rounded_rect(rect, 6, if it.mine { pal.danger.with_alpha(0x45) } else { pal.input_bg });
                    cv.stroke_rounded_rect(rect, 6, pal.border_subtle);
                    if it.mine {
                        cv.fill_circle_aa((rect.x + rect.w / 2) as f32, (rect.y + rect.h / 2) as f32, 6.0, pal.danger);
                    } else if it.adj > 0 {
                        let col = match it.adj {
                            1 => Color::rgb(0x38, 0xBD, 0xF8),
                            2 => Color::rgb(0x10, 0xB9, 0x81),
                            3 => Color::rgb(0xF4, 0x3F, 0x5E),
                            _ => Color::rgb(0xA8, 0x55, 0xF7),
                        };
                        let s = format!("{}", it.adj);
                        let tw = fonts.md.measure_width(&s);
                        cv.draw_text(&fonts.md, rect.x + (rect.w - tw) / 2, rect.y + (rect.h - fonts.md.line_height) / 2, &s, col);
                    }
                } else {
                    cv.fill_rounded_rect(rect, 6, if hov { pal.hover_bg } else { pal.elevated_bg });
                    cv.stroke_rounded_rect(rect, 6, pal.border_subtle);
                    if it.flagged {
                        let tw = fonts.sm.measure_width("F");
                        cv.draw_text(&fonts.sm, rect.x + (rect.w - tw) / 2, rect.y + (rect.h - fonts.sm.line_height) / 2, "F", pal.warning);
                    }
                }
            }
        }
    }
}
