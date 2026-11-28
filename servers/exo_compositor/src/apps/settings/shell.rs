//! Settings sidebar, global option search, shared `SystemConfig`, and category dispatcher.

use crate::apps::{draw_textfield, AppAction};
use crate::assets::FontSet;
use crate::i18n::Lang;
use crate::keymap::KeyboardLayout;
use crate::telemetry::TelemetryState;
use exo_ui::widgets::{
    compute_scrollbar, draw_button, draw_scrollbar, scroll_from_drag_y, slider_ratio_from_x,
};
use exo_ui::{AnimationMode, Canvas, Clipboard, Rect, TextField, Theme, ThemeMode};

#[derive(Clone, Debug)]
pub struct SystemConfig {
    pub hostname: String,
    pub username: String,
    pub lang: Lang,
    pub keymap: KeyboardLayout,
    pub use_24h: bool,
    pub tz_idx: usize,
    pub mouse_speed: u8,
    pub swap_mouse_buttons: bool,
    pub dev_mode: bool,
    pub admin_unlocked: bool,
    pub dnd_enabled: bool,
    pub net_policies: [u8; 3],
}

impl Default for SystemConfig {
    fn default() -> Self {
        Self {
            hostname: String::from("exo-workstation"),
            username: String::from("user"),
            lang: Lang::En,
            keymap: KeyboardLayout::AzertyFr,
            use_24h: true,
            tz_idx: 0,
            mouse_speed: 2,
            swap_mouse_buttons: false,
            dev_mode: false,
            admin_unlocked: false,
            dnd_enabled: false,
            net_policies: [1, 0, 2],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsCategory {
    About = 0,
    Display = 1,
    Personalization = 2,
    Input = 3,
    Locale = 4,
    Network = 5,
    Accounts = 6,
    SecurityStorage = 7,
    PowerDev = 8,
}

impl SettingsCategory {
    pub const ALL: [Self; 9] = [
        Self::About, Self::Display, Self::Personalization, Self::Input, Self::Locale,
        Self::Network, Self::Accounts, Self::SecurityStorage, Self::PowerDev,
    ];

    pub fn from_index(i: usize) -> Self {
        Self::ALL.get(i).copied().unwrap_or(Self::Display)
    }

    pub const fn label(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Self::About, Lang::Fr) => "Système et À propos",
            (Self::About, Lang::En) => "System & About",
            (Self::Display, Lang::Fr) => "Affichage",
            (Self::Display, Lang::En) => "Display",
            (Self::Personalization, Lang::Fr) => "Personnalisation",
            (Self::Personalization, Lang::En) => "Personalization",
            (Self::Input, Lang::Fr) => "Clavier, Souris & Accès",
            (Self::Input, Lang::En) => "Keyboard, Mouse & Access",
            (Self::Locale, Lang::Fr) => "Date, heure et langue",
            (Self::Locale, Lang::En) => "Date, Time & Language",
            (Self::Network, Lang::Fr) => "Réseau (ExoNet)",
            (Self::Network, Lang::En) => "Network (ExoNet)",
            (Self::Accounts, Lang::Fr) => "Comptes & ExoAuth",
            (Self::Accounts, Lang::En) => "Accounts & ExoAuth",
            (Self::SecurityStorage, Lang::Fr) => "Sécurité & Stockage",
            (Self::SecurityStorage, Lang::En) => "Security & Storage",
            (Self::PowerDev, Lang::Fr) => "Alimentation & Dev",
            (Self::PowerDev, Lang::En) => "Power & Developer",
        }
    }

    pub const fn keywords(self) -> &'static str {
        match self {
            Self::About => "systeme cpu ram noyau a propos machine rapport",
            Self::Display => "affichage ecran luminosite resolution vsync fps gpu",
            Self::Personalization => "theme clair sombre fond ecran couleur accent flou animation",
            Self::Input => "clavier azerty qwerty bepo souris curseur accessibilite contraste",
            Self::Locale => "date heure langue francais english fuseau abidjan paris utc",
            Self::Network => "reseau ethernet exonet pare-feu netcap dns ip",
            Self::Accounts => "compte utilisateur exoauth verrouillage admin mot de passe",
            Self::SecurityStorage => "securite exoshield exoledger stockage exofs instantane",
            Self::PowerDev => "alimentation eteindre redemarrer exophoenix developpeur dev",
        }
    }
}

pub struct SettingsApp {
    pub category: SettingsCategory,
    pub search: TextField,
    pub search_focused: bool,
    pub scroll_y: i32,
    pub dragging_scrollbar: bool,
    pub dragging_brightness: bool,
    pub dragging_alpha: bool,
    pub last_net_diag: Option<crate::net_diag::NetDiagReport>,
}

fn fold_search(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'é' | 'è' | 'ê' | 'ë' | 'É' | 'È' | 'Ê' | 'Ë' => 'e',
            'à' | 'â' | 'ä' | 'À' | 'Â' | 'Ä' => 'a',
            'î' | 'ï' | 'Î' | 'Ï' => 'i',
            'ô' | 'ö' | 'Ô' | 'Ö' => 'o',
            'ù' | 'û' | 'ü' | 'Ù' | 'Û' | 'Ü' => 'u',
            'ç' | 'Ç' => 'c',
            other => other.to_ascii_lowercase(),
        })
        .collect()
}

impl SettingsApp {
    pub fn new() -> Self {
        Self {
            category: SettingsCategory::Personalization,
            search: TextField::new(),
            search_focused: false,
            scroll_y: 0,
            dragging_scrollbar: false,
            dragging_brightness: false,
            dragging_alpha: false,
            last_net_diag: None,
        }
    }

    pub(crate) fn content_height(&self) -> i32 {
        match self.category {
            SettingsCategory::About => 580,
            SettingsCategory::Display => 560,
            SettingsCategory::Personalization => 620,
            SettingsCategory::Input => 580,
            SettingsCategory::Locale => 560,
            SettingsCategory::Network => 620,
            SettingsCategory::Accounts => 540,
            SettingsCategory::SecurityStorage => 580,
            SettingsCategory::PowerDev => 620,
        }
    }

    pub fn handle_scroll(&mut self, body: Rect, delta: i32) -> bool {
        let (_, content) = Self::layout(body);
        let max_s = (self.content_height() - content.h).max(0);
        if max_s <= 0 {
            self.scroll_y = 0;
            return false;
        }
        let next = (self.scroll_y + delta.saturating_mul(48)).clamp(0, max_s);
        if next != self.scroll_y {
            self.scroll_y = next;
            true
        } else {
            false
        }
    }

    pub(crate) fn layout(body: Rect) -> (Rect, Rect) {
        let side_w = 224.min(body.w / 3);
        (Rect::new(body.x, body.y, side_w, body.h), Rect::new(body.x + side_w, body.y, body.w - side_w, body.h))
    }

    pub(crate) fn visible_categories(&self) -> Vec<SettingsCategory> {
        let q = fold_search(self.search.text.trim());
        SettingsCategory::ALL
            .iter()
            .copied()
            .filter(|c| {
                q.is_empty()
                    || fold_search(c.label(Lang::Fr)).contains(&q)
                    || fold_search(c.label(Lang::En)).contains(&q)
                    || c.keywords().contains(&q)
            })
            .collect()
    }

    pub fn draw(&self, cv: &mut Canvas<'_>, body: Rect, mx: i32, my: i32, theme: &Theme, cfg: &SystemConfig, fonts: &FontSet, telemetry: &TelemetryState) {
        let pal = theme.palette();
        let lang = cfg.lang;
        let (sidebar, content) = Self::layout(body);

        cv.fill_rect(sidebar, pal.sidebar_bg);
        cv.fill_rect(Rect::new(sidebar.right() - 1, sidebar.y, 1, sidebar.h), pal.border_subtle);

        let sr = Rect::new(sidebar.x + 12, sidebar.y + 12, sidebar.w - 24, 32);
        cv.fill_rounded_rect(sr, 8, pal.input_bg);
        cv.stroke_rounded_rect(sr, 8, if self.search_focused { pal.accent } else { pal.border_subtle });
        draw_textfield(cv, sr.inset(10, 4), &self.search, lang.pick("Rechercher un réglage...", "Search settings..."), self.search_focused, &pal, &fonts.sm);

        let mut ty = sidebar.y + 56;
        for cat in self.visible_categories() {
            let r = Rect::new(sidebar.x + 10, ty, sidebar.w - 20, 34);
            let active = self.category == cat;
            if active {
                cv.fill_rounded_rect(r, 8, pal.accent.with_alpha(0x28));
                cv.fill_rect(Rect::new(r.x, r.y + 7, 3, r.h - 14), pal.accent);
            } else if r.contains(mx, my) {
                cv.fill_rounded_rect(r, 8, pal.hover_bg);
            }
            cv.draw_text_ellipsis(&fonts.sm, r.x + 12, r.y + 9, cat.label(lang), r.w - 20, if active { pal.accent } else { pal.text_primary });
            ty += 38;
        }

        let ch = self.content_height();
        let max_s = (ch - content.h).max(0);
        let sy = self.scroll_y.clamp(0, max_s);
        let scrolled = Rect::new(content.x, content.y - sy, content.w, content.h + sy);

        let prev = cv.push_clip(content);
        let reset_r = Rect::new(scrolled.right() - 154, scrolled.y + 16, 130, 28);
        draw_button(cv, reset_r, lang.pick("Réinitialiser", "Reset page"), false, reset_r.contains(mx, my), &pal, &fonts.sm);

        match self.category {
            SettingsCategory::About => self.draw_about(cv, scrolled, mx, my, &pal, cfg, fonts, telemetry),
            SettingsCategory::Display => self.draw_display(cv, scrolled, mx, my, theme, &pal, cfg, fonts),
            SettingsCategory::Personalization => self.draw_personalization(cv, scrolled, mx, my, theme, &pal, lang, fonts),
            SettingsCategory::Input => self.draw_input(cv, scrolled, mx, my, theme, &pal, cfg, fonts),
            SettingsCategory::Locale => self.draw_locale(cv, scrolled, mx, my, &pal, cfg, fonts, telemetry),
            SettingsCategory::Network => self.draw_network(cv, scrolled, mx, my, &pal, cfg, fonts, telemetry),
            SettingsCategory::Accounts => self.draw_accounts(cv, scrolled, mx, my, &pal, cfg, fonts, telemetry),
            SettingsCategory::SecurityStorage => self.draw_security_storage(cv, scrolled, mx, my, &pal, cfg, fonts, telemetry),
            SettingsCategory::PowerDev => self.draw_power_dev(cv, scrolled, mx, my, theme, &pal, cfg, fonts, telemetry),
        }
        cv.clip = prev;
        if let Some(sb) = compute_scrollbar(content, ch, sy) {
            draw_scrollbar(cv, sb, sb.track.inset(-3, 0).contains(mx, my), self.dragging_scrollbar, &pal);
        }
    }

    pub fn handle_mouse_press(&mut self, body: Rect, mx: i32, my: i32, theme: &mut Theme, cfg: &mut SystemConfig, telemetry: &mut TelemetryState, fonts: &FontSet) -> AppAction {
        let (sidebar, content) = Self::layout(body);
        if sidebar.contains(mx, my) {
            let sr = Rect::new(sidebar.x + 12, sidebar.y + 12, sidebar.w - 24, 32);
            if sr.contains(mx, my) {
                self.search_focused = true;
                let bi = fonts.sm.hit_test_byte_index(&self.search.text, mx - (sr.x + 10));
                self.search.mouse_press(bi, false);
                return AppAction::Redraw;
            }
            self.search_focused = false;
            let mut ty = sidebar.y + 56;
            for cat in self.visible_categories() {
                if Rect::new(sidebar.x + 10, ty, sidebar.w - 20, 34).contains(mx, my) {
                    self.category = cat;
                    self.scroll_y = 0;
                    self.dragging_scrollbar = false;
                    return AppAction::Redraw;
                }
                ty += 38;
            }
            return AppAction::None;
        }
        self.search_focused = false;
        let ch = self.content_height();
        let max_s = (ch - content.h).max(0);
        let sy = self.scroll_y.clamp(0, max_s);
        if let Some(sb) = compute_scrollbar(content, ch, sy) {
            if sb.track.inset(-4, 0).contains(mx, my) {
                self.dragging_scrollbar = true;
                self.scroll_y = scroll_from_drag_y(sb, my);
                return AppAction::Redraw;
            }
        }
        let scrolled = Rect::new(content.x, content.y - sy, content.w, content.h + sy);
        if Rect::new(scrolled.right() - 154, scrolled.y + 16, 130, 28).contains(mx, my) {
            self.reset_current_page(theme, cfg, telemetry);
            return AppAction::StatusToast(String::from(cfg.lang.pick("Page réinitialisée", "Page reset")));
        }
        match self.category {
            SettingsCategory::About => self.click_about(scrolled, mx, my, cfg, telemetry),
            SettingsCategory::Display => self.click_display(scrolled, mx, my, theme, cfg),
            SettingsCategory::Personalization => self.click_personalization(scrolled, mx, my, theme, cfg.lang),
            SettingsCategory::Input => self.click_input(scrolled, mx, my, theme, cfg),
            SettingsCategory::Locale => self.click_locale(scrolled, mx, my, cfg, telemetry),
            SettingsCategory::Network => self.click_network(scrolled, mx, my, cfg, telemetry),
            SettingsCategory::Accounts => self.click_accounts(scrolled, mx, my, cfg, telemetry),
            SettingsCategory::SecurityStorage => self.click_security_storage(scrolled, mx, my, cfg),
            SettingsCategory::PowerDev => self.click_power_dev(scrolled, mx, my, theme, cfg),
        }
    }

    pub fn handle_mouse_move(&mut self, body: Rect, mx: i32, my: i32, theme: &mut Theme) -> bool {
        let (_, content) = Self::layout(body);
        if self.dragging_scrollbar {
            if let Some(sb) = compute_scrollbar(content, self.content_height(), self.scroll_y) {
                let next = scroll_from_drag_y(sb, my);
                if next != self.scroll_y {
                    self.scroll_y = next;
                    return true;
                }
            }
            return false;
        }
        if self.dragging_brightness {
            let sr = Rect::new(content.x + 36, content.y + 116, (content.w - 92).max(160), 22);
            theme.brightness_pct = (35.0 + slider_ratio_from_x(sr, mx) * 95.0) as u16;
            return true;
        }
        if self.dragging_alpha {
            let sr = Rect::new(content.x + 36, content.y + 436, (content.w - 220).max(120), 22);
            theme.surface_alpha = (170.0 + slider_ratio_from_x(sr, mx) * 85.0) as u8;
            return true;
        }
        false
    }

    pub fn handle_mouse_release(&mut self) {
        self.dragging_scrollbar = false;
        self.dragging_brightness = false;
        self.dragging_alpha = false;
        self.search.mouse_release();
    }

    pub fn handle_key(&mut self, code: u16, ch: Option<char>, shortcut: Option<char>, ctrl: bool, shift: bool, clip: &mut Clipboard) -> AppAction {
        if !self.search_focused {
            let max_s = (self.content_height() - 360).max(0);
            return match code {
                0x0051 => { self.scroll_y = (self.scroll_y + 48).min(max_s); AppAction::Redraw }
                0x0052 => { self.scroll_y = self.scroll_y.saturating_sub(48); AppAction::Redraw }
                0x004e => { self.scroll_y = (self.scroll_y + 200).min(max_s); AppAction::Redraw }
                0x004b => { self.scroll_y = self.scroll_y.saturating_sub(200); AppAction::Redraw }
                _ => AppAction::None,
            };
        }
        if ctrl {
            match shortcut {
                Some('a') => self.search.select_all(),
                Some('c') => { self.search.copy(clip); }
                Some('x') => { self.search.cut(clip); }
                Some('v') => { self.search.paste(clip); }
                Some('z') => { self.search.undo(); }
                _ => {}
            }
            return AppAction::Redraw;
        }
        match code {
            0x0029 => self.search_focused = false,
            0x002a => self.search.backspace(),
            0x004c => self.search.delete_forward(),
            0x0050 => self.search.move_left(shift),
            0x004f => self.search.move_right(shift),
            _ => if let Some(c) = ch { self.search.insert_char(c); } else { return AppAction::None; },
        }
        AppAction::Redraw
    }

    fn reset_current_page(&mut self, theme: &mut Theme, cfg: &mut SystemConfig, t: &mut TelemetryState) {
        match self.category {
            SettingsCategory::Display => { theme.brightness_pct = 100; theme.night_light = false; theme.show_fps = false; }
            SettingsCategory::Personalization => {
                theme.mode = ThemeMode::Dark;
                theme.accent = exo_ui::AccentPreset::Indigo;
                theme.wallpaper_idx = 0;
                theme.surface_alpha = 244;
                theme.blur_enabled = true;
                theme.animations = AnimationMode::Full;
            }
            SettingsCategory::Input => { cfg.keymap = KeyboardLayout::AzertyFr; cfg.mouse_speed = 2; cfg.swap_mouse_buttons = false; theme.high_contrast = false; }
            SettingsCategory::Locale => { cfg.lang = Lang::En; cfg.tz_idx = 0; cfg.use_24h = true; t.tz_idx = 0; t.poll(); }
            SettingsCategory::Network => cfg.net_policies = [1, 0, 2],
            _ => {}
        }
    }
}
