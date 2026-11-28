//! Settings pages: System & About (§1.2), Display (§1.3), and Personalization (§1.4).

use crate::apps::settings::{SettingsApp, SystemConfig};
use crate::apps::AppAction;
use crate::assets::{FontSet, ICON_START_64, WALLPAPER_NAMES_EN, WALLPAPER_NAMES_FR};
use crate::i18n::Lang;
use crate::telemetry::TelemetryState;
use exo_ui::widgets::{draw_badge, draw_button, draw_slider, draw_state_pill, draw_toggle, slider_ratio_from_x, OptionState};
use exo_ui::{AccentPreset, AnimationMode, Canvas, Color, Palette, Rect, Theme, ThemeMode};

impl SettingsApp {
    pub(crate) fn draw_about(&self, cv: &mut Canvas<'_>, c: Rect, mx: i32, my: i32, pal: &Palette, cfg: &SystemConfig, fonts: &FontSet, t: &TelemetryState) {
        let lang = cfg.lang;
        cv.draw_text(&fonts.lg_bold, c.x + 24, c.y + 18, lang.pick("Système et À propos", "System & About"), pal.text_primary);
        cv.draw_text(&fonts.sm, c.x + 24, c.y + 42, lang.pick("Identité de la machine, matériel et informations système", "Machine identity, hardware, and OS details"), pal.text_secondary);

        let hero = Rect::new(c.x + 24, c.y + 68, c.w - 48, 86);
        cv.fill_rounded_rect(hero, 12, pal.card_bg);
        cv.stroke_rounded_rect(hero, 12, pal.border_subtle);
        cv.blit_rgba(hero.x + 18, hero.y + 15, 56, 56, ICON_START_64, 64, 64, 255);
        cv.draw_text(&fonts.lg_bold, hero.x + 88, hero.y + 16, "ExoOS 0.2.0", pal.text_primary);
        cv.draw_text(&fonts.sm, hero.x + 88, hero.y + 40, &format!("{}  •  {}", cfg.hostname, lang.pick("Micro-noyau Rust x86_64", "x86_64 Rust Microkernel")), pal.text_secondary);
        draw_badge(cv, hero.x + 88, hero.y + 60, "SMAP / SMEP / KPTI / VT-d", pal.success, &fonts.sm);

        let copy_btn = Rect::new(hero.right() - 196, hero.y + 26, 180, 34);
        draw_button(cv, copy_btn, lang.pick("Copier le rapport système", "Copy system report"), true, copy_btn.contains(mx, my), pal, &fonts.sm);

        let (up, tot) = t.critical_counts();
        let cards = [
            (
                lang.pick("Processeur", "Processor"),
                format!("x86_64 ({} {})", t.cpu_cores, lang.pick("cœurs", "cores")),
                format!("{} {} %", lang.pick("Charge :", "Load:"), t.cpu_load_pct),
            ),
            (
                lang.pick("Mémoire vive", "Memory"),
                format!("{} / {} {}", t.used_ram_mb, t.total_ram_mb, lang.pick("Mio", "MiB")),
                format!("{} % {}", t.mem_used_pct, lang.pick("utilisés", "used")),
            ),
            (
                lang.pick("Backend graphique", "Graphics Backend"),
                String::from("VirtIO-GPU 1.2 / 60 Hz"),
                String::from(lang.pick("Compositeur isolé", "Isolated Compositor")),
            ),
            (
                lang.pick("Services & Audit", "Services & Audit"),
                format!("{}/{} {}", up, tot, lang.pick("actifs", "online")),
                format!("ExoLedger: {} {}", t.exoledger_entries.max(0), lang.pick("entrées", "entries")),
            ),
        ];
        let cw = (c.w - 60) / 2;
        for (i, (k, v, sub)) in cards.iter().enumerate() {
            let col = (i % 2) as i32;
            let row = (i / 2) as i32;
            let r = Rect::new(c.x + 24 + col * (cw + 12), c.y + 170 + row * 94, cw, 82);
            cv.fill_rounded_rect(r, 10, pal.card_bg);
            cv.stroke_rounded_rect(r, 10, pal.border_subtle);
            cv.draw_text(&fonts.sm, r.x + 14, r.y + 12, k, pal.text_muted);
            cv.draw_text(&fonts.md, r.x + 14, r.y + 32, v, pal.text_primary);
            cv.draw_text(&fonts.sm, r.x + 14, r.y + 56, sub, pal.text_secondary);
        }

        let bench_btn = Rect::new(c.x + 24, c.y + 372, 240, 36);
        draw_button(cv, bench_btn, lang.pick("Ouvrir le banc ExoBench", "Open ExoBench Profiler"), false, bench_btn.contains(mx, my), pal, &fonts.sm);
    }

    pub(crate) fn click_about(&self, c: Rect, mx: i32, my: i32, cfg: &SystemConfig, t: &TelemetryState) -> AppAction {
        let hero = Rect::new(c.x + 24, c.y + 68, c.w - 48, 86);
        if Rect::new(hero.right() - 196, hero.y + 26, 180, 34).contains(mx, my) {
            return AppAction::CopyText(t.system_report_text(&cfg.hostname));
        }
        if Rect::new(c.x + 24, c.y + 372, 240, 36).contains(mx, my) {
            return AppAction::OpenApp(crate::apps::AppKind::Bench);
        }
        AppAction::None
    }

    pub(crate) fn draw_display(&self, cv: &mut Canvas<'_>, c: Rect, mx: i32, my: i32, theme: &Theme, pal: &Palette, cfg: &SystemConfig, fonts: &FontSet) {
        let lang = cfg.lang;
        cv.draw_text(&fonts.lg_bold, c.x + 24, c.y + 18, lang.pick("Affichage", "Display"), pal.text_primary);
        cv.draw_text(&fonts.sm, c.x + 24, c.y + 42, lang.pick("Luminosité en temps réel, résolution et backend graphique", "Real-time brightness, resolution, and display backend"), pal.text_secondary);

        // Brightness card
        let bcard = Rect::new(c.x + 24, c.y + 72, c.w - 48, 114);
        cv.fill_rounded_rect(bcard, 12, pal.card_bg);
        cv.stroke_rounded_rect(bcard, 12, pal.border_subtle);
        cv.draw_text(&fonts.md, bcard.x + 14, bcard.y + 12, lang.pick("Luminosité de l'écran", "Screen brightness"), pal.text_primary);
        let pct_txt = format!("{} %", theme.brightness_pct);
        let pw = fonts.lg_bold.measure_width(&pct_txt);
        cv.draw_text(&fonts.lg_bold, bcard.right() - pw - 16, bcard.y + 10, &pct_txt, pal.accent);

        let sr = Rect::new(c.x + 36, c.y + 116, (c.w - 92).max(160), 22);
        let ratio = (theme.brightness_pct.saturating_sub(35) as f32) / 95.0;
        draw_slider(cv, sr, ratio, pal.accent, pal, sr.contains(mx, my) || self.dragging_brightness);

        let mut px = bcard.x + 14;
        for (lbl, val) in [
            (lang.pick("Nuit 65%", "Night 65%"), 65u16),
            (lang.pick("Éco 85%", "Eco 85%"), 85),
            ("Normal 100%", 100),
            (lang.pick("Éclat 120%", "Bright 120%"), 120),
        ] {
            let pr = Rect::new(px, bcard.y + 76, 104, 26);
            draw_button(cv, pr, lbl, theme.brightness_pct == val, pr.contains(mx, my), pal, &fonts.sm);
            px += 112;
        }

        // Resolution & Backend rows + live Night Light toggle
        let res_str = format!("{} × {} px • 60 Hz (32-bit ARGB)", cv.width, cv.height);
        let rows = [
            (lang.pick("Résolution & Fréquence", "Resolution & Refresh"), res_str.as_str(), OptionState::Applied),
            (lang.pick("Accélération GPU & Compositeur", "GPU & Compositor Engine"), lang.pick("VirtIO-GPU 1.2 / Cache 2D 60 Hz", "VirtIO-GPU 1.2 / 60 Hz 2D Cache"), OptionState::Applied),
        ];
        let mut ry = c.y + 202;
        for (title, val, st) in rows {
            let r = Rect::new(c.x + 24, ry, c.w - 48, 52);
            cv.fill_rounded_rect(r, 10, pal.card_bg);
            cv.stroke_rounded_rect(r, 10, pal.border_subtle);
            cv.draw_text(&fonts.md, r.x + 14, r.y + 8, title, pal.text_primary);
            cv.draw_text(&fonts.sm, r.x + 14, r.y + 28, val, pal.text_secondary);
            draw_state_pill(cv, r.right() - 140, r.y + 15, st, lang.is_en(), pal, &fonts.sm);
            ry += 60;
        }

        // Night Light (Warm blue-light filter) toggle row — whole card clickable!
        let ncard = Rect::new(c.x + 24, ry, c.w - 48, 52);
        cv.fill_rounded_rect(ncard, 10, if ncard.contains(mx, my) { pal.hover_bg } else { pal.card_bg });
        cv.stroke_rounded_rect(ncard, 10, pal.border_subtle);
        cv.draw_text(&fonts.md, ncard.x + 14, ncard.y + 8, lang.pick("Éclairage nocturne (Filtre lumière bleue)", "Night Light (Blue-light filter)"), pal.text_primary);
        cv.draw_text(&fonts.sm, ncard.x + 14, ncard.y + 28, lang.pick("Teinte chaude reposante appliquée en temps réel", "Applies a warm evening tint across the screen"), pal.text_muted);
        draw_toggle(cv, Rect::new(ncard.right() - 68, ncard.y + 14, 50, 24), theme.night_light, pal.accent, pal);
        ry += 60;

        // FPS debug toggle — whole card clickable!
        let fcard = Rect::new(c.x + 24, ry + 4, c.w - 48, 54);
        cv.fill_rounded_rect(fcard, 10, if fcard.contains(mx, my) { pal.hover_bg } else { pal.card_bg });
        cv.stroke_rounded_rect(fcard, 10, pal.border_subtle);
        cv.draw_text(&fonts.md, fcard.x + 14, fcard.y + 10, lang.pick("Compteur d'images par seconde (FPS)", "FPS performance overlay"), pal.text_primary);
        cv.draw_text(&fonts.sm, fcard.x + 14, fcard.y + 30, lang.pick("Affiche le temps de rendu du compositeur", "Shows live compositor frame time"), pal.text_muted);
        let tog = Rect::new(fcard.right() - 68, fcard.y + 15, 50, 24);
        draw_toggle(cv, tog, theme.show_fps, pal.accent, pal);
    }

    pub(crate) fn click_display(&mut self, c: Rect, mx: i32, my: i32, theme: &mut Theme, _cfg: &SystemConfig) -> AppAction {
        let sr = Rect::new(c.x + 36, c.y + 116, (c.w - 92).max(160), 22);
        if sr.inset(-4, -8).contains(mx, my) {
            self.dragging_brightness = true;
            theme.brightness_pct = (35.0 + slider_ratio_from_x(sr, mx) * 95.0) as u16;
            return AppAction::Redraw;
        }
        let bcard = Rect::new(c.x + 24, c.y + 72, c.w - 48, 114);
        let mut px = bcard.x + 14;
        for val in [65u16, 85, 100, 120] {
            if Rect::new(px, bcard.y + 76, 104, 26).contains(mx, my) {
                theme.brightness_pct = val;
                return AppAction::Redraw;
            }
            px += 112;
        }
        let ncard = Rect::new(c.x + 24, c.y + 322, c.w - 48, 52);
        if ncard.contains(mx, my) {
            theme.night_light = !theme.night_light;
            return AppAction::Redraw;
        }
        let fcard = Rect::new(c.x + 24, c.y + 386, c.w - 48, 54);
        if fcard.contains(mx, my) {
            theme.show_fps = !theme.show_fps;
            return AppAction::Redraw;
        }
        AppAction::None
    }

    pub(crate) fn draw_personalization(&self, cv: &mut Canvas<'_>, c: Rect, mx: i32, my: i32, theme: &Theme, pal: &Palette, lang: Lang, fonts: &FontSet) {
        cv.draw_text(&fonts.lg_bold, c.x + 24, c.y + 18, lang.pick("Personnalisation", "Personalization"), pal.text_primary);
        cv.draw_text(&fonts.sm, c.x + 24, c.y + 42, lang.pick("Thème clair ou sombre, couleurs harmonisées et transitions", "Light or dark theme, harmonized accents, and smooth transitions"), pal.text_secondary);

        // 1. Dark / Light mode cards
        let mw = (c.w - 60) / 2;
        let dark_r = Rect::new(c.x + 24, c.y + 72, mw, 52);
        let light_r = Rect::new(dark_r.right() + 12, c.y + 72, mw, 52);
        draw_button(cv, dark_r, lang.pick("Thème Sombre (Graphite)", "Dark Theme (Graphite)"), theme.mode == ThemeMode::Dark, dark_r.contains(mx, my), pal, &fonts.md);
        draw_button(cv, light_r, lang.pick("Thème Clair (Porcelaine)", "Light Theme (Porcelain)"), theme.mode == ThemeMode::Light, light_r.contains(mx, my), pal, &fonts.md);

        // 2. Wallpaper selector (4 harmonious wallpapers)
        cv.draw_text(&fonts.md, c.x + 24, c.y + 140, lang.pick("Fond d'écran", "Wallpaper"), pal.text_primary);
        let wp_names = if lang.is_en() { &WALLPAPER_NAMES_EN } else { &WALLPAPER_NAMES_FR };
        let wp_w = ((c.w - 72) / 4).max(100);
        for (i, &name) in wp_names.iter().enumerate() {
            let r = Rect::new(c.x + 24 + (i as i32) * (wp_w + 8), c.y + 166, wp_w, 78);
            let active = theme.wallpaper_idx == i as u8;
            let (c1, c2) = match i {
                0 => (Color::rgb(0x28, 0x42, 0x52), Color::rgb(0x7B, 0x5E, 0x8C)),
                1 => (Color::rgb(0xE2, 0xEC, 0xF5), Color::rgb(0xF4, 0xE2, 0xD8)),
                2 => (Color::rgb(0x1F, 0x29, 0x3D), Color::rgb(0x6B, 0x36, 0x3D)),
                _ => (Color::rgb(0x16, 0x19, 0x1F), Color::rgb(0x1F, 0x31, 0x2E)),
            };
            let prev = cv.push_clip(r);
            cv.fill_gradient_v(r, c1, c2);
            cv.fill_rect(Rect::new(r.x, r.bottom() - 24, r.w, 24), Color::rgba(0x10, 0x12, 0x16, 0xB8));
            cv.draw_text_ellipsis(&fonts.sm, r.x + 8, r.bottom() - 19, name, r.w - 12, Color::WHITE);
            cv.clip = prev;
            cv.stroke_rounded_rect(r, 10, if active { pal.accent } else { pal.border_subtle });
            if active {
                draw_badge(cv, r.x + 6, r.y + 6, lang.pick("Actif", "Active"), pal.accent, &fonts.sm);
            }
        }

        // 3. Six harmonized accent color swatches
        cv.draw_text(&fonts.md, c.x + 24, c.y + 260, lang.pick("Couleur d'accentuation", "Accent color"), pal.text_primary);
        let aw = ((c.w - 88) / 6).max(68);
        for (i, &preset) in AccentPreset::ALL.iter().enumerate() {
            let r = Rect::new(c.x + 24 + (i as i32) * (aw + 8), c.y + 286, aw, 42);
            let active = theme.accent == preset;
            cv.fill_rounded_rect(r, 9, if active { preset.primary().with_alpha(0x2A) } else { pal.card_bg });
            cv.stroke_rounded_rect(r, 9, if active { preset.primary() } else { pal.border_subtle });
            cv.fill_circle_aa(r.x as f32 + 16.0, r.y as f32 + 21.0, 6.5, preset.primary());
            cv.draw_text_ellipsis(&fonts.sm, r.x + 28, r.y + 13, preset.name_i18n(lang.is_en()), r.w - 32, pal.text_primary);
        }

        // 4. Animations selector (Full / Reduced / Off)
        cv.draw_text(&fonts.md, c.x + 24, c.y + 344, lang.pick("Transitions et animations", "Transitions & animations"), pal.text_primary);
        let anims = [
            (AnimationMode::Full, lang.pick("Fluides", "Smooth")),
            (AnimationMode::Reduced, lang.pick("Réduites", "Reduced")),
            (AnimationMode::Off, lang.pick("Désactivées", "Off")),
        ];
        let mut ax = c.x + 24;
        for (mode, lbl) in anims {
            let r = Rect::new(ax, c.y + 370, 120, 30);
            draw_button(cv, r, lbl, theme.animations == mode, r.contains(mx, my), pal, &fonts.sm);
            ax += 128;
        }

        // 5. Translucency slider & Blur toggle
        let gcard = Rect::new(c.x + 24, c.y + 410, c.w - 48, 62);
        cv.fill_rounded_rect(gcard, 12, pal.card_bg);
        cv.stroke_rounded_rect(gcard, 12, pal.border_subtle);
        let alpha_pct = ((theme.surface_alpha as u32 * 100) / 255).min(100);
        cv.draw_text(&fonts.sm, gcard.x + 14, gcard.y + 8, &format!("{} ({}%)", lang.pick("Opacité des fenêtres", "Window opacity"), alpha_pct), pal.text_secondary);
        let gsr = Rect::new(c.x + 36, c.y + 436, (c.w - 220).max(120), 22);
        let gr = (theme.surface_alpha.saturating_sub(170) as f32) / 85.0;
        draw_slider(cv, gsr, gr, pal.accent, pal, gsr.contains(mx, my) || self.dragging_alpha);
        cv.draw_text(&fonts.sm, gcard.right() - 142, gcard.y + 10, lang.pick("Flou d'arrière-plan", "Backdrop blur"), pal.text_secondary);
        draw_toggle(cv, Rect::new(gcard.right() - 74, gcard.y + 30, 50, 24), theme.blur_enabled, pal.accent, pal);
    }

    pub(crate) fn click_personalization(&mut self, c: Rect, mx: i32, my: i32, theme: &mut Theme, lang: Lang) -> AppAction {
        let mw = (c.w - 60) / 2;
        if Rect::new(c.x + 24, c.y + 72, mw, 52).contains(mx, my) {
            theme.mode = ThemeMode::Dark;
            return AppAction::Redraw;
        }
        if Rect::new(c.x + 36 + mw, c.y + 72, mw, 52).contains(mx, my) {
            theme.mode = ThemeMode::Light;
            return AppAction::Redraw;
        }
        let wp_w = ((c.w - 72) / 4).max(100);
        for i in 0..4 {
            if Rect::new(c.x + 24 + (i as i32) * (wp_w + 8), c.y + 166, wp_w, 78).contains(mx, my) {
                theme.wallpaper_idx = i as u8;
                let n = if lang.is_en() { WALLPAPER_NAMES_EN[i] } else { WALLPAPER_NAMES_FR[i] };
                return AppAction::StatusToast(format!("{} : {}", lang.pick("Fond", "Wallpaper"), n));
            }
        }
        let aw = ((c.w - 88) / 6).max(68);
        for (i, &preset) in AccentPreset::ALL.iter().enumerate() {
            if Rect::new(c.x + 24 + (i as i32) * (aw + 8), c.y + 286, aw, 42).contains(mx, my) {
                theme.accent = preset;
                return AppAction::Redraw;
            }
        }
        let mut ax = c.x + 24;
        for mode in [AnimationMode::Full, AnimationMode::Reduced, AnimationMode::Off] {
            if Rect::new(ax, c.y + 370, 120, 30).contains(mx, my) {
                theme.animations = mode;
                return AppAction::Redraw;
            }
            ax += 128;
        }
        let gcard = Rect::new(c.x + 24, c.y + 410, c.w - 48, 62);
        let gsr = Rect::new(c.x + 36, c.y + 436, (c.w - 220).max(120), 22);
        if gsr.inset(-4, -10).contains(mx, my) {
            self.dragging_alpha = true;
            theme.surface_alpha = (170.0 + slider_ratio_from_x(gsr, mx) * 85.0) as u8;
            return AppAction::Redraw;
        }
        if Rect::new(gcard.right() - 156, gcard.y, 156, gcard.h).contains(mx, my) {
            theme.blur_enabled = !theme.blur_enabled;
            return AppAction::Redraw;
        }
        AppAction::None
    }
}
