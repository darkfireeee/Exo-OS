//! Clean Dark/Light renderer for Files (toolbar, sidebar, Grid/List viewport, Trash, Snapshots).

use crate::apps::draw_textfield;
use crate::apps::files::{format_bytes, is_read_only_zone, FilesApp, FilesSection, SortBy, ViewMode, SIDEBAR_PLACES};
use crate::assets::{FontSet, IconId};
use crate::i18n::Lang;
use crate::telemetry::TelemetryState;
use exo_ui::widgets::{compute_scrollbar, draw_badge, draw_button, draw_scrollbar};
use exo_ui::{Canvas, Color, Palette, Rect, Theme};

impl FilesApp {
    pub(crate) fn content_height(&self, viewport: Rect, count: usize) -> i32 {
        match self.view_mode {
            ViewMode::Grid => {
                let cell_w = 104;
                let cell_h = 92;
                let gap = 10;
                let cols = ((viewport.w - 28) / (cell_w + gap)).max(1) as usize;
                let rows = (count + cols - 1) / cols;
                12 + (rows as i32) * (cell_h + gap) + 12
            }
            ViewMode::List => 34 + (count as i32) * 32 + 12,
        }
    }

    pub(crate) fn item_rects(&self, viewport: Rect, filtered: &[usize]) -> Vec<(usize, Rect)> {
        let mut out = Vec::with_capacity(filtered.len());
        let ch = self.content_height(viewport, filtered.len());
        let eff_scroll = self.scroll_y.clamp(0, (ch - viewport.h).max(0));
        match self.view_mode {
            ViewMode::Grid => {
                let cell_w = 104;
                let cell_h = 92;
                let gap = 10;
                let cols = ((viewport.w - 28) / (cell_w + gap)).max(1);
                for (pos, &idx) in filtered.iter().enumerate() {
                    let c = (pos as i32) % cols;
                    let r = (pos as i32) / cols;
                    let x = viewport.x + 12 + c * (cell_w + gap);
                    let y = viewport.y + 12 + r * (cell_h + gap) - eff_scroll;
                    out.push((idx, Rect::new(x, y, cell_w, cell_h)));
                }
            }
            ViewMode::List => {
                let row_h = 32;
                for (pos, &idx) in filtered.iter().enumerate() {
                    let y = viewport.y + 34 + (pos as i32) * row_h - eff_scroll;
                    out.push((idx, Rect::new(viewport.x + 10, y, viewport.w - 28, row_h - 2)));
                }
            }
        }
        out
    }

    pub fn draw(
        &self,
        cv: &mut Canvas<'_>,
        body: Rect,
        mx: i32,
        my: i32,
        theme: &Theme,
        lang: Lang,
        fonts: &FontSet,
        telemetry: &TelemetryState,
    ) {
        let pal = theme.palette();
        let (toolbar, sidebar, viewport, inspector, statusbar) = Self::layout(body);

        // 1. Toolbar
        cv.fill_rect(toolbar, pal.sidebar_bg);
        cv.fill_rect(Rect::new(toolbar.x, toolbar.bottom() - 1, toolbar.w, 1), pal.border_subtle);
        for (lbl, dx) in [("←", 10), ("→", 42), ("↑", 74), ("R", 106)] {
            let br = Rect::new(toolbar.x + dx, toolbar.y + 9, 28, 28);
            draw_button(cv, br, lbl, false, br.contains(mx, my), &pal, &fonts.sm);
        }

        // Breadcrumb bar
        let search_w = 166.min(toolbar.w / 4);
        let crumb_x = toolbar.x + 144;
        let right_zone = 278 + search_w;
        let crumb_w = (toolbar.w - 154 - right_zone).max(90);
        let crumb_r = Rect::new(crumb_x, toolbar.y + 8, crumb_w, 30);
        cv.fill_rounded_rect(crumb_r, 8, pal.input_bg);
        cv.stroke_rounded_rect(crumb_r, 8, pal.border_subtle);

        let mut cx = crumb_r.x + 8;
        let prev_c = cv.push_clip(crumb_r.inset(4, 2));
        for seg in breadcrumb_segments(&self.cwd) {
            let sw = fonts.sm.measure_width(&seg) + 12;
            let sr = Rect::new(cx, crumb_r.y + 4, sw, 22);
            if sr.contains(mx, my) {
                cv.fill_rounded_rect(sr, 5, pal.accent.with_alpha(0x28));
            }
            cv.draw_text(&fonts.sm, sr.x + 6, sr.y + 3, &seg, pal.text_primary);
            cx += sw + 2;
            cv.draw_text(&fonts.sm, cx, crumb_r.y + 7, "/", pal.text_muted);
            cx += 9;
        }
        cv.clip = prev_c;

        // Action buttons: +Dossier, +Fichier, Vue, Tri
        let mut qx = crumb_r.right() + 8;
        let sort_lbl = match self.sort_by {
            SortBy::Name => lang.pick("Tri:Nom", "Sort:Name"),
            SortBy::Size => lang.pick("Tri:Taille", "Sort:Size"),
            SortBy::Kind => lang.pick("Tri:Type", "Sort:Type"),
        };
        let quick = [
            (lang.pick("+ Dossier", "+ Folder"), 74),
            (lang.pick("+ Fichier", "+ File"), 68),
            (if self.view_mode == ViewMode::Grid { lang.pick("Liste", "List") } else { lang.pick("Grille", "Grid") }, 50),
            (sort_lbl, 66),
        ];
        for (lbl, bw) in quick {
            let r = Rect::new(qx, toolbar.y + 9, bw, 28);
            draw_button(cv, r, lbl, false, r.contains(mx, my), &pal, &fonts.sm);
            qx += bw + 5;
        }

        // Search input
        let search_r = Rect::new(toolbar.right() - search_w - 10, toolbar.y + 8, search_w, 30);
        cv.fill_rounded_rect(search_r, 8, pal.input_bg);
        cv.stroke_rounded_rect(search_r, 8, if self.search_focused { pal.accent } else { pal.border_subtle });
        draw_textfield(
            cv,
            search_r.inset(8, 4),
            &self.search,
            lang.pick("Rechercher...", "Search..."),
            self.search_focused,
            &pal,
            &fonts.sm,
        );

        // 2. Sidebar
        self.draw_sidebar(cv, sidebar, mx, my, &pal, lang, fonts, telemetry);

        // 3. Main Viewport
        let prev_vp = cv.push_clip(viewport);
        match self.section {
            FilesSection::Snapshots => self.draw_snapshots(cv, viewport, mx, my, &pal, lang, fonts, telemetry),
            FilesSection::Trash | FilesSection::Directory => {
                self.draw_directory(cv, viewport, mx, my, &pal, lang, fonts);
            }
        }
        cv.clip = prev_vp;

        // 4. Properties & Capabilities Inspector
        if inspector.w > 0 {
            self.draw_inspector(cv, inspector, mx, my, &pal, lang, fonts);
        }

        // 5. Status bar
        cv.fill_rect(statusbar, pal.sidebar_bg);
        cv.fill_rect(Rect::new(statusbar.x, statusbar.y, statusbar.w, 1), pal.border_subtle);
        let ro_tag = if is_read_only_zone(&self.cwd) { lang.pick(" [Lecture seule]", " [Read-only]") } else { "" };
        let st = format!("{}{}", self.status_line, ro_tag);
        cv.draw_text_ellipsis(&fonts.sm, statusbar.x + 12, statusbar.y + 6, &st, statusbar.w - 190, pal.text_secondary);
        let cnt = format!("{} | {} {}", self.entries.len(), self.selected.len(), lang.pick("sél.", "sel."));
        let cw = fonts.sm.measure_width(&cnt);
        cv.draw_text(&fonts.sm, statusbar.right() - cw - 12, statusbar.y + 6, &cnt, pal.text_muted);
    }

    fn draw_sidebar(
        &self,
        cv: &mut Canvas<'_>,
        sb: Rect,
        mx: i32,
        my: i32,
        pal: &Palette,
        lang: Lang,
        fonts: &FontSet,
        telemetry: &TelemetryState,
    ) {
        cv.fill_rect(sb, pal.sidebar_bg);
        cv.fill_rect(Rect::new(sb.right() - 1, sb.y, 1, sb.h), pal.border_subtle);

        let mut sy = sb.y + 12;
        cv.draw_text(&fonts.sm, sb.x + 14, sy, lang.pick("EMPLACEMENTS", "PLACES"), pal.text_muted);
        sy += 22;
        for &(fr, en, path) in &SIDEBAR_PLACES {
            let r = Rect::new(sb.x + 8, sy, sb.w - 16, 28);
            let active = self.section == FilesSection::Directory && self.cwd == path;
            if active {
                cv.fill_rounded_rect(r, 7, pal.accent.with_alpha(0x28));
                cv.fill_rect(Rect::new(r.x, r.y + 6, 3, r.h - 12), pal.accent);
            } else if r.contains(mx, my) {
                cv.fill_rounded_rect(r, 7, pal.hover_bg);
            }
            cv.draw_text_ellipsis(
                &fonts.sm,
                r.x + 12,
                r.y + 6,
                lang.pick(fr, en),
                r.w - 18,
                if active { pal.accent } else { pal.text_primary },
            );
            sy += 30;
        }

        sy += 10;
        cv.draw_text(&fonts.sm, sb.x + 14, sy, "EXOFS", pal.text_muted);
        sy += 22;
        for (lbl, sec, ico) in [
            (lang.pick("Corbeille", "Trash"), FilesSection::Trash, IconId::Trash),
            (lang.pick("Instantanés", "Snapshots"), FilesSection::Snapshots, IconId::Files),
        ] {
            let r = Rect::new(sb.x + 8, sy, sb.w - 16, 28);
            let active = self.section == sec;
            if active {
                cv.fill_rounded_rect(r, 7, pal.accent.with_alpha(0x28));
            } else if r.contains(mx, my) {
                cv.fill_rounded_rect(r, 7, pal.hover_bg);
            }
            cv.blit_rgba(r.x + 8, r.y + 5, 18, 18, ico.rgba_40(), 40, 40, 255);
            cv.draw_text(&fonts.sm, r.x + 32, r.y + 6, lbl, if active { pal.accent } else { pal.text_primary });
            sy += 30;
        }

        // Disk capacity bar
        let qy = sb.bottom() - 56;
        if qy > sy + 12 {
            cv.draw_text(&fonts.sm, sb.x + 14, qy, lang.pick("Stockage ExoFS", "ExoFS Storage"), pal.text_secondary);
            let used = telemetry.exofs_used_kb / 1024;
            let tot = (telemetry.exofs_total_kb / 1024).max(1);
            cv.draw_text(&fonts.sm, sb.x + 14, qy + 17, &format!("{} / {} Mio", used, tot), pal.text_muted);
            let bar = Rect::new(sb.x + 14, qy + 36, sb.w - 28, 6);
            cv.fill_rounded_rect(bar, 3, pal.border_subtle);
            let fw = ((bar.w as u64 * used) / tot).clamp(8, bar.w as u64) as i32;
            cv.fill_rounded_rect(Rect::new(bar.x, bar.y, fw, 6), 3, pal.accent);
        }
    }

    fn draw_directory(&self, cv: &mut Canvas<'_>, vp: Rect, mx: i32, my: i32, pal: &Palette, lang: Lang, fonts: &FontSet) {
        let mut content_vp = vp;
        if self.section == FilesSection::Trash {
            let banner = Rect::new(vp.x + 10, vp.y + 8, vp.w - 20, 38);
            cv.fill_rounded_rect(banner, 9, pal.card_bg);
            cv.stroke_rounded_rect(banner, 9, pal.border_subtle);
            cv.draw_text(&fonts.sm, banner.x + 12, banner.y + 11, lang.pick("Corbeille — les éléments peuvent être restaurés", "Trash — items can be restored"), pal.text_secondary);
            let r_btn = Rect::new(banner.right() - 216, banner.y + 5, 96, 28);
            let e_btn = Rect::new(banner.right() - 112, banner.y + 5, 102, 28);
            draw_button(cv, r_btn, lang.pick("Restaurer", "Restore"), false, r_btn.contains(mx, my), pal, &fonts.sm);
            draw_button(cv, e_btn, lang.pick("Vider", "Empty"), true, e_btn.contains(mx, my), pal, &fonts.sm);
            content_vp = Rect::new(vp.x, vp.y + 50, vp.w, (vp.h - 50).max(40));
        }

        if self.view_mode == ViewMode::List {
            cv.fill_rect(Rect::new(content_vp.x, content_vp.y, content_vp.w, 28), pal.sidebar_bg);
            cv.draw_text(&fonts.sm, content_vp.x + 42, content_vp.y + 7, lang.pick("Nom", "Name"), pal.text_muted);
            cv.draw_text(&fonts.sm, content_vp.right() - 186, content_vp.y + 7, lang.pick("Type", "Type"), pal.text_muted);
            cv.draw_text(&fonts.sm, content_vp.right() - 86, content_vp.y + 7, lang.pick("Taille", "Size"), pal.text_muted);
        }

        let filtered = self.filtered_indices();
        let rects = self.item_rects(content_vp, &filtered);
        for (idx, r) in rects {
            if r.bottom() < content_vp.y || r.y > content_vp.bottom() { continue; }
            let ent = &self.entries[idx];
            let sel = self.selected.contains(&idx);
            if sel {
                cv.fill_rounded_rect(r, 9, pal.accent.with_alpha(0x28));
                cv.stroke_rounded_rect(r, 9, pal.accent.with_alpha(0xA0));
            } else if r.contains(mx, my) {
                cv.fill_rounded_rect(r, 9, pal.hover_bg);
            }
            let ico = if ent.is_dir { IconId::Files } else if ent.is_exec { IconId::Terminal } else { IconId::Editor };
            match self.view_mode {
                ViewMode::Grid => {
                    cv.blit_rgba(r.x + (r.w - 40) / 2, r.y + 10, 40, 40, ico.rgba_40(), 40, 40, 255);
                    if self.renaming_idx == Some(idx) {
                        let rr = Rect::new(r.x + 4, r.y + 58, r.w - 8, 24);
                        cv.fill_rounded_rect(rr, 5, pal.input_bg);
                        cv.stroke_rounded_rect(rr, 5, pal.accent);
                        draw_textfield(cv, rr.inset(4, 2), &self.rename_field, "", true, pal, &fonts.sm);
                    } else {
                        let lw = fonts.sm.measure_width(&ent.name).min(r.w - 10);
                        cv.draw_text_ellipsis(&fonts.sm, r.x + (r.w - lw) / 2, r.y + 60, &ent.name, r.w - 10, pal.text_primary);
                    }
                }
                ViewMode::List => {
                    cv.blit_rgba(r.x + 6, r.y + 5, 20, 20, ico.rgba_40(), 40, 40, 255);
                    cv.draw_text_ellipsis(&fonts.sm, r.x + 34, r.y + 7, &ent.name, r.w - 220, pal.text_primary);
                    let kind = if ent.is_dir { lang.pick("Dossier", "Folder") } else if ent.is_exec { "ELF" } else { lang.pick("Fichier", "File") };
                    cv.draw_text(&fonts.sm, r.right() - 180, r.y + 7, kind, pal.text_secondary);
                    let sz = if ent.is_dir { String::from("—") } else { format_bytes(ent.size) };
                    cv.draw_text(&fonts.sm, r.right() - 80, r.y + 7, &sz, pal.text_muted);
                }
            }
        }

        if self.marquee_active {
            let mr = Rect::from_points(self.marquee_start.0, self.marquee_start.1, self.marquee_cur.0, self.marquee_cur.1);
            if mr.w >= 2 || mr.h >= 2 {
                let draw_r = Rect::new(mr.x, mr.y, mr.w.max(2), mr.h.max(2));
                cv.fill_rounded_rect(draw_r, 4, Color::rgba(0x3B, 0x82, 0xF6, 0x44));
                cv.stroke_rounded_rect(draw_r, 4, Color::rgb(0x60, 0xA5, 0xFA));
            }
        }
        let ch = self.content_height(content_vp, filtered.len());
        let eff_scroll = self.scroll_y.clamp(0, (ch - content_vp.h).max(0));
        if let Some(sb) = compute_scrollbar(content_vp, ch, eff_scroll) {
            draw_scrollbar(cv, sb, sb.track.inset(-3, 0).contains(mx, my), self.dragging_scrollbar, pal);
        }
    }

    fn draw_snapshots(&self, cv: &mut Canvas<'_>, vp: Rect, mx: i32, my: i32, pal: &Palette, lang: Lang, fonts: &FontSet, t: &TelemetryState) {
        cv.draw_text(&fonts.lg_bold, vp.x + 16, vp.y + 14, lang.pick("Instantanés ExoFS (Époques)", "ExoFS Snapshots (Epochs)"), pal.text_primary);
        let btn = Rect::new(vp.right() - 178, vp.y + 12, 162, 30);
        draw_button(cv, btn, lang.pick("+ Nouvel instantané", "+ New Snapshot"), true, btn.contains(mx, my), pal, &fonts.sm);
        let mut y = vp.y + 56;
        for (i, snap) in t.exofs_snapshots.iter().enumerate() {
            let card = Rect::new(vp.x + 16, y, vp.w - 32, 46);
            cv.fill_rounded_rect(card, 10, pal.card_bg);
            cv.stroke_rounded_rect(card, 10, pal.border_subtle);
            cv.draw_text_ellipsis(&fonts.md, card.x + 14, card.y + 14, snap, card.w - 130, pal.text_primary);
            draw_badge(cv, card.right() - 104, card.y + 11, if i == 0 { "Actif" } else { "Scellé" }, Color::rgb(0x10, 0xB9, 0x81), &fonts.sm);
            y += 54;
        }
    }
}

pub(crate) fn breadcrumb_segments(cwd: &str) -> Vec<String> {
    let mut out = vec![String::from("Root")];
    for part in cwd.split('/').filter(|s| !s.is_empty()) {
        out.push(String::from(part));
    }
    out
}
