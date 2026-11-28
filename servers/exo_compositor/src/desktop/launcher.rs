//! Clean Application Launcher overlay (ExoOS UI Spec §3.5).

use crate::apps::{draw_textfield, AppAction, AppKind};
use crate::assets::{IconId, ICON_START_64, WALLPAPER_NAMES_EN, WALLPAPER_NAMES_FR};
use crate::desktop::state::{DesktopShell, DOCK_H, TOP_BAR_H};
use exo_ui::widgets::draw_button;
use exo_ui::{Canvas, Rect, ThemeMode};

#[derive(Clone, Copy)]
pub(crate) struct LauncherItem {
    pub kind: Option<AppKind>,
    pub trash: bool,
    pub label_fr: &'static str,
    pub label_en: &'static str,
    pub desc_fr: &'static str,
    pub desc_en: &'static str,
    pub icon: IconId,
}

const LAUNCHER_ITEMS: [LauncherItem; 8] = [
    LauncherItem { kind: Some(AppKind::Files), trash: false, label_fr: "Fichiers", label_en: "Files", desc_fr: "Dossiers, corbeille et ExoFS", desc_en: "Folders, Trash & ExoFS", icon: IconId::Files },
    LauncherItem { kind: Some(AppKind::Terminal), trash: false, label_fr: "Terminal", label_en: "Terminal", desc_fr: "Shell exosh multi-onglets", desc_en: "Multi-tab exosh shell", icon: IconId::Terminal },
    LauncherItem { kind: Some(AppKind::Monitor), trash: false, label_fr: "Moniteur", label_en: "Monitor", desc_fr: "Graphes, processus et capacités", desc_en: "Graphs, processes & caps", icon: IconId::Monitor },
    LauncherItem { kind: Some(AppKind::Settings), trash: false, label_fr: "Paramètres", label_en: "Settings", desc_fr: "Affichage, thème, clavier, réseau", desc_en: "Display, theme, keyboard, net", icon: IconId::Settings },
    LauncherItem { kind: Some(AppKind::Editor), trash: false, label_fr: "Éditeur", label_en: "Editor", desc_fr: "Texte, recherche et coloration", desc_en: "Text, find & syntax highlight", icon: IconId::Editor },
    LauncherItem { kind: Some(AppKind::Bench), trash: false, label_fr: "ExoBench", label_en: "ExoBench", desc_fr: "Mesure complète et rapport final", desc_en: "Full profiler & final report", icon: IconId::Bench },
    LauncherItem { kind: Some(AppKind::Games), trash: false, label_fr: "Jeux", label_en: "Games", desc_fr: "Snake et Démineur 2D", desc_en: "2D Snake & Minesweeper", icon: IconId::Games },
    LauncherItem { kind: None, trash: true, label_fr: "Corbeille", label_en: "Trash", desc_fr: "Restaurer ou vider les fichiers", desc_en: "Restore or empty deleted files", icon: IconId::Trash },
];

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

impl DesktopShell {
    pub(crate) fn launcher_rect(&self) -> Rect {
        let w = 620.min(self.width as i32 - 32);
        let h = 448.min(self.height as i32 - TOP_BAR_H - DOCK_H - 34);
        let x = (self.width as i32 - w) / 2;
        let y = self.dock_rect().y - h - 12;
        Rect::new(x, y.max(TOP_BAR_H + 8), w, h)
    }

    pub(crate) fn filtered_launcher_items(&self) -> Vec<LauncherItem> {
        let q = fold_search(self.launcher_search.text.trim());
        LAUNCHER_ITEMS
            .iter()
            .copied()
            .filter(|t| {
                q.is_empty()
                    || fold_search(t.label_fr).contains(&q)
                    || fold_search(t.label_en).contains(&q)
                    || fold_search(t.desc_fr).contains(&q)
                    || fold_search(t.desc_en).contains(&q)
            })
            .collect()
    }

    pub(crate) fn draw_launcher(&self, cv: &mut Canvas<'_>) {
        if !self.launcher_open { return; }
        let pal = self.theme.palette();
        let lang = self.cfg.lang;
        let lr = self.launcher_rect();
        cv.draw_panel(lr, 18, pal.menu_bg, pal.border_strong, self.theme.blur_enabled, true);

        // Clean header with user's crystal hexagon emblem
        cv.blit_rgba(lr.x + 22, lr.y + 14, 48, 48, ICON_START_64, 64, 64, 255);
        cv.draw_text(&self.fonts.lg_bold, lr.x + 82, lr.y + 16, "ExoOS", pal.text_primary);
        cv.draw_text(&self.fonts.sm, lr.x + 82, lr.y + 40, lang.pick("Applications et actions rapides", "Applications and quick actions"), pal.text_secondary);

        // Search input
        let sr = Rect::new(lr.x + 22, lr.y + 74, lr.w - 44, 34);
        cv.fill_rounded_rect(sr, 10, pal.input_bg);
        cv.stroke_rounded_rect(sr, 10, pal.accent);
        draw_textfield(
            cv,
            sr.inset(12, 4),
            &self.launcher_search,
            lang.pick("Rechercher une application...", "Search applications..."),
            true,
            &pal,
            &self.fonts.md,
        );

        // 4x2 Harmonized Multi-Color App Grid
        let items = self.filtered_launcher_items();
        let cols = 4;
        let gap = 10;
        let tile_w = (lr.w - 44 - gap * (cols - 1)) / cols;
        let tile_h = 92;
        let gx = lr.x + 22;
        let gy = lr.y + 122;

        for (i, item) in items.iter().enumerate().take(8) {
            let col = (i as i32) % cols;
            let row = (i as i32) / cols;
            let tr = Rect::new(gx + col * (tile_w + gap), gy + row * (tile_h + gap), tile_w, tile_h);
            let hov = tr.contains(self.mouse_x, self.mouse_y);
            cv.fill_rounded_rect(tr, 12, if hov { pal.hover_bg } else { pal.card_bg });
            cv.stroke_rounded_rect(tr, 12, if hov { pal.accent } else { pal.border_subtle });
            cv.blit_rgba(tr.x + (tr.w - 40) / 2, tr.y + 10, 40, 40, item.icon.rgba_40(), 40, 40, 255);
            let lbl = if lang.is_en() { item.label_en } else { item.label_fr };
            let lw = self.fonts.sm.measure_width(lbl);
            cv.draw_text(&self.fonts.sm, tr.x + (tr.w - lw) / 2, tr.y + 55, lbl, pal.text_primary);
            let desc = if lang.is_en() { item.desc_en } else { item.desc_fr };
            cv.draw_text_ellipsis(&self.fonts.sm, tr.x + 8, tr.y + 72, desc, tr.w - 16, pal.text_muted);
        }

        // Quick appearance strip (Light/Dark + Wallpaper)
        let sy = lr.bottom() - 92;
        let th_r = Rect::new(lr.x + 22, sy, 152, 30);
        let wp_r = Rect::new(th_r.right() + 10, sy, 152, 30);
        draw_button(
            cv,
            th_r,
            if self.theme.mode == ThemeMode::Dark { lang.pick("Passer en Clair", "Switch to Light") } else { lang.pick("Passer en Sombre", "Switch to Dark") },
            false,
            th_r.contains(self.mouse_x, self.mouse_y),
            &pal,
            &self.fonts.sm,
        );
        draw_button(cv, wp_r, lang.pick("Fond suivant", "Next Wallpaper"), false, wp_r.contains(self.mouse_x, self.mouse_y), &pal, &self.fonts.sm);

        // Footer: User profile + Lock / Reboot / Shutdown
        let fy = lr.bottom() - 48;
        cv.fill_rect(Rect::new(lr.x + 1, fy, lr.w - 2, 1), pal.border_subtle);
        cv.fill_circle_aa((lr.x + 34) as f32, (fy + 24) as f32, 12.0, pal.accent);
        let init_ch = self.cfg.username.chars().next().unwrap_or('U').to_ascii_uppercase().to_string();
        let iw = self.fonts.sm.measure_width(&init_ch);
        cv.draw_text(&self.fonts.sm, lr.x + 34 - iw / 2, fy + 16, &init_ch, exo_ui::Color::WHITE);
        cv.draw_text(&self.fonts.sm, lr.x + 54, fy + 16, &self.cfg.username, pal.text_primary);

        let lk = Rect::new(lr.right() - 314, fy + 9, 92, 30);
        let rb = Rect::new(lr.right() - 214, fy + 9, 98, 30);
        let sd = Rect::new(lr.right() - 108, fy + 9, 88, 30);
        draw_button(cv, lk, lang.pick("Verrouiller", "Lock"), false, lk.contains(self.mouse_x, self.mouse_y), &pal, &self.fonts.sm);
        draw_button(cv, rb, lang.pick("Redémarrer", "Reboot"), false, rb.contains(self.mouse_x, self.mouse_y), &pal, &self.fonts.sm);
        draw_button(cv, sd, lang.pick("Éteindre", "Shut down"), true, sd.contains(self.mouse_x, self.mouse_y), &pal, &self.fonts.sm);
    }

    pub(crate) fn click_launcher(&mut self, mx: i32, my: i32, shift: bool) -> bool {
        if !self.launcher_open { return false; }
        let lr = self.launcher_rect();
        if !lr.contains(mx, my) {
            self.launcher_open = false;
            self.dirty = true;
            return false;
        }
        let sr = Rect::new(lr.x + 22, lr.y + 74, lr.w - 44, 34);
        if sr.contains(mx, my) {
            let bi = self.fonts.md.hit_test_byte_index(&self.launcher_search.text, mx - (sr.x + 12));
            self.launcher_search.mouse_press(bi, shift);
            self.dirty = true;
            return true;
        }

        let items = self.filtered_launcher_items();
        let cols = 4;
        let gap = 10;
        let tile_w = (lr.w - 44 - gap * (cols - 1)) / cols;
        let gx = lr.x + 22;
        let gy = lr.y + 122;
        for (i, item) in items.iter().enumerate().take(8) {
            let col = (i as i32) % cols;
            let row = (i as i32) / cols;
            if Rect::new(gx + col * (tile_w + gap), gy + row * 102, tile_w, 92).contains(mx, my) {
                self.launch_item(*item);
                return true;
            }
        }

        let sy = lr.bottom() - 92;
        if Rect::new(lr.x + 22, sy, 152, 30).contains(mx, my) {
            self.theme.mode = if self.theme.mode == ThemeMode::Dark { ThemeMode::Light } else { ThemeMode::Dark };
            self.dirty = true;
            return true;
        }
        if Rect::new(lr.x + 184, sy, 152, 30).contains(mx, my) {
            self.theme.wallpaper_idx = (self.theme.wallpaper_idx + 1) % 4;
            let n = if self.cfg.lang.is_en() { WALLPAPER_NAMES_EN[self.theme.wallpaper_idx as usize] } else { WALLPAPER_NAMES_FR[self.theme.wallpaper_idx as usize] };
            self.show_toast(n);
            return true;
        }

        let fy = lr.bottom() - 48;
        if Rect::new(lr.right() - 314, fy + 9, 92, 30).contains(mx, my) {
            self.process_action(AppAction::LockScreen, None);
        } else if Rect::new(lr.right() - 214, fy + 9, 98, 30).contains(mx, my) {
            self.launcher_open = false;
            self.process_action(AppAction::Reboot, None);
        } else if Rect::new(lr.right() - 108, fy + 9, 88, 30).contains(mx, my) {
            self.launcher_open = false;
            self.process_action(AppAction::Shutdown, None);
        }
        true
    }

    pub(crate) fn launch_item(&mut self, item: LauncherItem) {
        self.launcher_open = false;
        if item.trash {
            self.process_action(AppAction::OpenFilesAt(String::from(crate::apps::files::TRASH_DIR)), None);
        } else if let Some(kind) = item.kind {
            self.open_or_focus_app(kind);
        }
    }
}
