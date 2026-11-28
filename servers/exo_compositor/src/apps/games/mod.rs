//! # Jeux 2D légers — Snake (timers + clavier) & Démineur (souris + clic droit) (Spec §4.3).

mod minesweeper;
mod snake;

use crate::apps::AppAction;
use crate::assets::FontSet;
use crate::i18n::Lang;
use exo_ui::widgets::draw_button;
use exo_ui::{Canvas, Rect, Theme};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ActiveGame {
    Snake,
    Minesweeper,
}

pub struct GamesApp {
    pub active: ActiveGame,
    pub snake: snake::SnakeGame,
    pub mines: minesweeper::MinesweeperGame,
}

impl GamesApp {
    pub fn new() -> Self {
        Self {
            active: ActiveGame::Snake,
            snake: snake::SnakeGame::new(),
            mines: minesweeper::MinesweeperGame::new(42),
        }
    }

    pub fn tick(&mut self) -> bool {
        if self.active == ActiveGame::Snake {
            self.snake.tick()
        } else {
            false
        }
    }

    pub fn draw(&self, cv: &mut Canvas<'_>, body: Rect, mx: i32, my: i32, theme: &Theme, lang: Lang, fonts: &FontSet) {
        let pal = theme.palette();
        let hdr = Rect::new(body.x, body.y, body.w, 46);
        cv.fill_rect(hdr, pal.sidebar_bg);
        cv.fill_rect(Rect::new(hdr.x, hdr.bottom() - 1, hdr.w, 1), pal.border_subtle);

        let b1 = Rect::new(hdr.x + 12, hdr.y + 8, 120, 30);
        let b2 = Rect::new(b1.right() + 10, hdr.y + 8, 134, 30);
        let b3 = Rect::new(hdr.right() - 116, hdr.y + 8, 104, 30);
        draw_button(cv, b1, "Snake", self.active == ActiveGame::Snake, b1.contains(mx, my), &pal, &fonts.sm);
        draw_button(cv, b2, lang.pick("Démineur", "Minesweeper"), self.active == ActiveGame::Minesweeper, b2.contains(mx, my), &pal, &fonts.sm);
        draw_button(cv, b3, lang.pick("Rejouer", "Restart"), false, b3.contains(mx, my), &pal, &fonts.sm);

        let area = Rect::new(body.x, hdr.bottom(), body.w, (body.h - hdr.h).max(120));
        match self.active {
            ActiveGame::Snake => self.snake.draw(cv, area, &pal, lang, fonts),
            ActiveGame::Minesweeper => self.mines.draw(cv, area, mx, my, &pal, lang, fonts),
        }
    }

    pub fn handle_mouse_press(&mut self, body: Rect, mx: i32, my: i32, right: bool, tick: u64) -> AppAction {
        let hdr = Rect::new(body.x, body.y, body.w, 46);
        if !right && hdr.contains(mx, my) {
            if Rect::new(hdr.x + 12, hdr.y + 8, 120, 30).contains(mx, my) {
                self.active = ActiveGame::Snake;
                return AppAction::Redraw;
            }
            if Rect::new(hdr.x + 142, hdr.y + 8, 134, 30).contains(mx, my) {
                self.active = ActiveGame::Minesweeper;
                return AppAction::Redraw;
            }
            if Rect::new(hdr.right() - 116, hdr.y + 8, 104, 30).contains(mx, my) {
                match self.active {
                    ActiveGame::Snake => self.snake.reset(),
                    ActiveGame::Minesweeper => self.mines = minesweeper::MinesweeperGame::new(tick.wrapping_add(17)),
                }
                return AppAction::Redraw;
            }
        }
        let area = Rect::new(body.x, hdr.bottom(), body.w, (body.h - hdr.h).max(120));
        match self.active {
            ActiveGame::Minesweeper => {
                if self.mines.click(area, mx, my, right) { AppAction::Redraw } else { AppAction::None }
            }
            ActiveGame::Snake => {
                if !right && area.contains(mx, my) {
                    self.snake.paused = !self.snake.paused;
                    AppAction::Redraw
                } else {
                    AppAction::None
                }
            }
        }
    }

    pub fn handle_key(&mut self, code: u16) -> AppAction {
        if self.active == ActiveGame::Snake && self.snake.on_key(code) {
            AppAction::Redraw
        } else {
            AppAction::None
        }
    }
}
