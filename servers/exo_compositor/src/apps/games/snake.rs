//! Lightweight 2D Snake game (tests compositor timers, keyboard latency & ExoFS score save).

use crate::assets::FontSet;
use crate::i18n::Lang;
use exo_ui::{Canvas, Color, Palette, Rect};

const GRID_W: i32 = 20;
const GRID_H: i32 = 15;

pub struct SnakeGame {
    body: Vec<(i32, i32)>,
    dir: (i32, i32),
    food: (i32, i32),
    pub score: u32,
    pub best_score: u32,
    pub paused: bool,
    pub game_over: bool,
    rng: u64,
}

impl SnakeGame {
    pub fn new() -> Self {
        let best = load_best_snake();
        let mut g = Self {
            body: Vec::new(),
            dir: (1, 0),
            food: (14, 7),
            score: 0,
            best_score: best,
            paused: false,
            game_over: false,
            rng: 0x51A4_E123,
        };
        g.reset();
        g
    }

    pub fn reset(&mut self) {
        self.body = vec![(8, 7), (7, 7), (6, 7)];
        self.dir = (1, 0);
        self.score = 0;
        self.paused = false;
        self.game_over = false;
        self.spawn_food();
    }

    fn spawn_food(&mut self) {
        for _ in 0..40 {
            self.rng = self.rng.wrapping_mul(6364136223846793005).wrapping_add(1);
            let x = ((self.rng >> 16) as i32).abs() % GRID_W;
            let y = ((self.rng >> 32) as i32).abs() % GRID_H;
            if !self.body.contains(&(x, y)) {
                self.food = (x, y);
                return;
            }
        }
    }

    pub fn on_key(&mut self, code: u16) -> bool {
        if code == 0x002c {
            if self.game_over { self.reset(); } else { self.paused = !self.paused; }
            return true;
        }
        let next = match code {
            0x0052 => (0, -1), // Up
            0x0051 => (0, 1),  // Down
            0x0050 => (-1, 0), // Left
            0x004f => (1, 0),  // Right
            _ => return false,
        };
        if next.0 != -self.dir.0 || next.1 != -self.dir.1 {
            self.dir = next;
        }
        true
    }

    pub fn tick(&mut self) -> bool {
        if self.paused || self.game_over { return false; }
        let head = self.body[0];
        let nh = (head.0 + self.dir.0, head.1 + self.dir.1);
        if nh.0 < 0 || nh.1 < 0 || nh.0 >= GRID_W || nh.1 >= GRID_H || self.body.contains(&nh) {
            self.game_over = true;
            if self.score > self.best_score {
                self.best_score = self.score;
                save_best_snake(self.best_score);
            }
            return true;
        }
        self.body.insert(0, nh);
        if nh == self.food {
            self.score += 10;
            if self.score > self.best_score { self.best_score = self.score; }
            self.spawn_food();
        } else {
            self.body.pop();
        }
        true
    }

    pub fn draw(&self, cv: &mut Canvas<'_>, area: Rect, pal: &Palette, lang: Lang, fonts: &FontSet) {
        let info = format!(
            "Score : {}   •   {} : {}   •   {}",
            self.score,
            lang.pick("Record", "Best"),
            self.best_score,
            lang.pick("Flèches pour diriger • Espace : Pause", "Arrow keys • Space: Pause")
        );
        cv.draw_text(&fonts.sm, area.x + 16, area.y + 10, &info, pal.text_secondary);

        let cell = ((area.w - 32) / GRID_W).min((area.h - 46) / GRID_H).max(12);
        let bw = cell * GRID_W;
        let bh = cell * GRID_H;
        let ox = area.x + (area.w - bw) / 2;
        let oy = area.y + 34 + (area.h - 44 - bh) / 2;
        let board = Rect::new(ox, oy, bw, bh);

        cv.fill_rounded_rect(board, 10, pal.card_bg);
        cv.stroke_rounded_rect(board, 10, pal.border_strong);

        // Food (coral circle)
        let fx = ox + self.food.0 * cell + cell / 2;
        let fy = oy + self.food.1 * cell + cell / 2;
        cv.fill_circle_aa(fx as f32, fy as f32, (cell as f32) * 0.36, Color::rgb(0xF4, 0x3F, 0x5E));

        // Snake segments (smooth emerald -> teal gradient)
        let len = self.body.len().max(1);
        for (i, &(gx, gy)) in self.body.iter().enumerate() {
            let t = ((i * 255) / len) as u8;
            let col = Color::rgb(0x10, 0xB9, 0x81).lerp(Color::rgb(0x0E, 0xA5, 0xE9), t);
            let sr = Rect::new(ox + gx * cell + 2, oy + gy * cell + 2, cell - 4, cell - 4);
            cv.fill_rounded_rect(sr, 5, col);
        }

        if self.game_over || self.paused {
            let msg = if self.game_over { lang.pick("Partie terminée — Espace pour rejouer", "Game Over — Press Space") } else { lang.pick("Pause", "Paused") };
            let mw = fonts.md.measure_width(msg) + 28;
            let ov = Rect::new(ox + (bw - mw) / 2, oy + bh / 2 - 18, mw, 36);
            cv.fill_rounded_rect(ov, 10, pal.menu_bg);
            cv.stroke_rounded_rect(ov, 10, pal.accent);
            cv.draw_text(&fonts.md, ov.x + 14, ov.y + 9, msg, pal.text_primary);
        }
    }
}

fn load_best_snake() -> u32 {
    std::fs::read_to_string("/tmp/exo_snake_best.txt").ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}

fn save_best_snake(score: u32) {
    let _ = std::fs::write("/tmp/exo_snake_best.txt", format!("{}", score).as_bytes());
}
