//! Settings pages: Keyboard, Mouse & Accessibility (§1.6, §1.7, §1.15) and Date/Time/Language (§1.14).

use crate::apps::settings::{SettingsApp, SystemConfig};
use crate::apps::AppAction;
use crate::assets::FontSet;
use crate::i18n::{Lang, TIMEZONES};
use crate::keymap::KeyboardLayout;
use crate::telemetry::TelemetryState;
use exo_ui::widgets::{draw_button, draw_state_pill, draw_toggle, OptionState};
use exo_ui::{Canvas, Palette, Rect, Theme};

impl SettingsApp {
    pub(crate) fn draw_input(&self, cv: &mut Canvas<'_>, c: Rect, mx: i32, my: i32, theme: &Theme, pal: &Palette, cfg: &SystemConfig, fonts: &FontSet) {
        let lang = cfg.lang;
        cv.draw_text(&fonts.lg_bold, c.x + 24, c.y + 18, lang.pick("Clavier, Souris et Accessibilité", "Keyboard, Mouse & Accessibility"), pal.text_primary);
        cv.draw_text(&fonts.sm, c.x + 24, c.y + 42, lang.pick("Disposition clavier temps réel, pointeur et confort visuel", "Live keyboard layout, pointer speed, and visual accessibility"), pal.text_secondary);

        // 1. Live Keyboard Layout (AZERTY FR / QWERTY US / BÉPO)
        let kcard = Rect::new(c.x + 24, c.y + 74, c.w - 48, 92);
        cv.fill_rounded_rect(kcard, 12, pal.card_bg);
        cv.stroke_rounded_rect(kcard, 12, pal.border_subtle);
        cv.draw_text(&fonts.md, kcard.x + 14, kcard.y + 12, lang.pick("Disposition du clavier", "Keyboard layout"), pal.text_primary);
        draw_state_pill(cv, kcard.right() - 102, kcard.y + 11, OptionState::Applied, lang.is_en(), pal, &fonts.sm);
        let mut kx = kcard.x + 14;
        for &lay in &KeyboardLayout::ALL {
            let r = Rect::new(kx, kcard.y + 46, 134, 32);
            draw_button(cv, r, lay.code(), cfg.keymap == lay, r.contains(mx, my), pal, &fonts.sm);
            kx += 144;
        }

        // 2. Mouse speed & left-handed swap
        let mcard = Rect::new(c.x + 24, c.y + 180, c.w - 48, 118);
        cv.fill_rounded_rect(mcard, 12, pal.card_bg);
        cv.stroke_rounded_rect(mcard, 12, pal.border_subtle);
        cv.draw_text(&fonts.md, mcard.x + 14, mcard.y + 12, lang.pick("Vitesse du pointeur", "Pointer speed"), pal.text_primary);
        let mut sx = mcard.x + 14;
        for (spd, lbl) in [(1u8, lang.pick("Précise (1x)", "Precise (1x)")), (2, lang.pick("Normale (2x)", "Normal (2x)")), (3, lang.pick("Rapide (3x)", "Fast (3x)"))] {
            let r = Rect::new(sx, mcard.y + 42, 134, 28);
            draw_button(cv, r, lbl, cfg.mouse_speed == spd, r.contains(mx, my), pal, &fonts.sm);
            sx += 144;
        }
        cv.draw_text(&fonts.sm, mcard.x + 14, mcard.y + 84, lang.pick("Inverser bouton gauche et droit (gaucher)", "Swap left and right mouse buttons"), pal.text_secondary);
        draw_toggle(cv, Rect::new(mcard.right() - 68, mcard.y + 80, 50, 24), cfg.swap_mouse_buttons, pal.accent, pal);

        // 3. Accessibility: High Contrast
        let acard = Rect::new(c.x + 24, c.y + 312, c.w - 48, 62);
        cv.fill_rounded_rect(acard, 12, pal.card_bg);
        cv.stroke_rounded_rect(acard, 12, pal.border_subtle);
        cv.draw_text(&fonts.md, acard.x + 14, acard.y + 10, lang.pick("Contraste élevé (Accessibilité)", "High contrast borders"), pal.text_primary);
        cv.draw_text(&fonts.sm, acard.x + 14, acard.y + 32, lang.pick("Renforce la lisibilité des contours et cartes", "Strengthens border contrast across all apps"), pal.text_muted);
        draw_toggle(cv, Rect::new(acard.right() - 68, acard.y + 19, 50, 24), theme.high_contrast, pal.accent, pal);

        // 4. Shortcuts reference card
        let sc = Rect::new(c.x + 24, c.y + 388, c.w - 48, 96);
        cv.fill_rounded_rect(sc, 12, pal.card_bg);
        cv.stroke_rounded_rect(sc, 12, pal.border_subtle);
        cv.draw_text(&fonts.sm, sc.x + 14, sc.y + 10, lang.pick("Raccourcis clavier système", "System keyboard shortcuts"), pal.accent);
        cv.draw_text_ellipsis(
            &fonts.sm,
            sc.x + 14,
            sc.y + 32,
            lang.pick(
                "Super : Lanceur  •  Super+E : Fichiers  •  Super+T : Terminal  •  Super+I : Paramètres",
                "Super: Launcher  •  Super+E: Files  •  Super+T: Terminal  •  Super+I: Settings",
            ),
            sc.w - 28,
            pal.text_secondary,
        );
        cv.draw_text_ellipsis(
            &fonts.sm,
            sc.x + 14,
            sc.y + 52,
            lang.pick(
                "Super+L : Verrouiller  •  Super+D : Bureau  •  Super+←/→ : Ancrer  •  Alt+Tab : Fenêtre",
                "Super+L: Lock  •  Super+D: Desktop  •  Super+←/→: Snap  •  Alt+Tab: Switch Window",
            ),
            sc.w - 28,
            pal.text_secondary,
        );
        cv.draw_text_ellipsis(
            &fonts.sm,
            sc.x + 14,
            sc.y + 72,
            lang.pick(
                "Ctrl+A/C/X/V/Z/Y : Sélectionner / Copier / Couper / Coller / Annuler (+ clic droit)",
                "Ctrl+A/C/X/V/Z/Y: Select All / Copy / Cut / Paste / Undo (+ right-click menu)",
            ),
            sc.w - 28,
            pal.text_muted,
        );
    }

    pub(crate) fn click_input(&self, c: Rect, mx: i32, my: i32, theme: &mut Theme, cfg: &mut SystemConfig) -> AppAction {
        let kcard = Rect::new(c.x + 24, c.y + 74, c.w - 48, 92);
        let mut kx = kcard.x + 14;
        for &lay in &KeyboardLayout::ALL {
            if Rect::new(kx, kcard.y + 46, 134, 32).contains(mx, my) {
                cfg.keymap = lay;
                return AppAction::StatusToast(format!("{}: {}", cfg.lang.pick("Clavier", "Keyboard"), lay.code()));
            }
            kx += 144;
        }
        let mcard = Rect::new(c.x + 24, c.y + 180, c.w - 48, 118);
        let mut sx = mcard.x + 14;
        for spd in [1u8, 2, 3] {
            if Rect::new(sx, mcard.y + 42, 134, 28).contains(mx, my) {
                cfg.mouse_speed = spd;
                return AppAction::Redraw;
            }
            sx += 144;
        }
        if Rect::new(mcard.x, mcard.y + 74, mcard.w, 44).contains(mx, my) {
            cfg.swap_mouse_buttons = !cfg.swap_mouse_buttons;
            return AppAction::Redraw;
        }
        let acard = Rect::new(c.x + 24, c.y + 312, c.w - 48, 62);
        if acard.contains(mx, my) {
            theme.high_contrast = !theme.high_contrast;
            return AppAction::Redraw;
        }
        AppAction::None
    }

    pub(crate) fn draw_locale(&self, cv: &mut Canvas<'_>, c: Rect, mx: i32, my: i32, pal: &Palette, cfg: &SystemConfig, fonts: &FontSet, t: &TelemetryState) {
        let lang = cfg.lang;
        cv.draw_text(&fonts.lg_bold, c.x + 24, c.y + 18, lang.pick("Date, heure et langue", "Date, Time & Language"), pal.text_primary);
        cv.draw_text(&fonts.sm, c.x + 24, c.y + 42, &format!("{}  •  {}", t.clock.format_date(lang), t.clock.format_time(cfg.use_24h)), pal.accent);

        // 1. Live Language switch (English default primary / Français secondary)
        let lcard = Rect::new(c.x + 24, c.y + 74, c.w - 48, 86);
        cv.fill_rounded_rect(lcard, 12, pal.card_bg);
        cv.stroke_rounded_rect(lcard, 12, pal.border_subtle);
        cv.draw_text(&fonts.md, lcard.x + 14, lcard.y + 12, lang.pick("Langue de l'interface", "Interface language"), pal.text_primary);
        let en_r = Rect::new(lcard.x + 14, lcard.y + 44, 150, 30);
        let fr_r = Rect::new(en_r.right() + 12, lcard.y + 44, 150, 30);
        draw_button(cv, en_r, "English (EN)", cfg.lang == Lang::En, en_r.contains(mx, my), pal, &fonts.sm);
        draw_button(cv, fr_r, "Français (FR)", cfg.lang == Lang::Fr, fr_r.contains(mx, my), pal, &fonts.sm);

        // 2. 24-hour clock toggle
        let hcard = Rect::new(c.x + 24, c.y + 174, c.w - 48, 54);
        cv.fill_rounded_rect(hcard, 12, pal.card_bg);
        cv.stroke_rounded_rect(hcard, 12, pal.border_subtle);
        cv.draw_text(&fonts.md, hcard.x + 14, hcard.y + 16, lang.pick("Format 24 heures", "24-hour time format"), pal.text_primary);
        draw_toggle(cv, Rect::new(hcard.right() - 68, hcard.y + 15, 50, 24), cfg.use_24h, pal.accent, pal);

        // 3. Timezone selector
        cv.draw_text(&fonts.md, c.x + 24, c.y + 244, lang.pick("Fuseau horaire", "Time zone"), pal.text_primary);
        let mut ty = c.y + 272;
        for (i, tz) in TIMEZONES.iter().enumerate() {
            let r = Rect::new(c.x + 24, ty, c.w - 48, 36);
            let active = cfg.tz_idx == i;
            cv.fill_rounded_rect(r, 9, if active { pal.accent.with_alpha(0x28) } else if r.contains(mx, my) { pal.hover_bg } else { pal.card_bg });
            cv.stroke_rounded_rect(r, 9, if active { pal.accent } else { pal.border_subtle });
            cv.draw_text(&fonts.sm, r.x + 14, r.y + 10, tz.name, if active { pal.accent } else { pal.text_primary });
            ty += 42;
        }
    }

    pub(crate) fn click_locale(&self, c: Rect, mx: i32, my: i32, cfg: &mut SystemConfig, t: &mut TelemetryState) -> AppAction {
        let lcard = Rect::new(c.x + 24, c.y + 74, c.w - 48, 86);
        let en_r = Rect::new(lcard.x + 14, lcard.y + 44, 150, 30);
        let fr_r = Rect::new(en_r.right() + 12, lcard.y + 44, 150, 30);
        if en_r.contains(mx, my) { cfg.lang = Lang::En; return AppAction::Redraw; }
        if fr_r.contains(mx, my) { cfg.lang = Lang::Fr; return AppAction::Redraw; }
        let hcard = Rect::new(c.x + 24, c.y + 174, c.w - 48, 54);
        if hcard.contains(mx, my) {
            cfg.use_24h = !cfg.use_24h;
            return AppAction::Redraw;
        }
        let mut ty = c.y + 272;
        for i in 0..TIMEZONES.len() {
            if Rect::new(c.x + 24, ty, c.w - 48, 36).contains(mx, my) {
                cfg.tz_idx = i;
                t.tz_idx = i;
                t.poll();
                return AppAction::Redraw;
            }
            ty += 42;
        }
        AppAction::None
    }
}
