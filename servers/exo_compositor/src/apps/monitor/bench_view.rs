//! Interactive ExoBench UI tab: all-module performance profiler + visual breakdown + final report export.

use crate::apps::monitor::MonitorApp;
use crate::apps::AppAction;
use crate::assets::FontSet;
use crate::bench::{BenchCategory, BenchSuite};
use crate::i18n::Lang;
use exo_ui::charts::{draw_horizontal_bars, draw_radial_gauge, BarEntry};
use exo_ui::widgets::{compute_scrollbar, draw_button, draw_scrollbar};
use exo_ui::{Canvas, Color, Palette, Rect};

impl MonitorApp {
    pub(crate) fn draw_bench(&self, cv: &mut Canvas<'_>, c: Rect, mx: i32, my: i32, pal: &Palette, lang: Lang, fonts: &FontSet, bench: &BenchSuite) {
        let pad = 16;
        let gap = 12;

        // Top Action Bar
        let bar = Rect::new(c.x + pad, c.y + pad, c.w - pad * 2, 46);
        cv.fill_rounded_rect(bar, 10, pal.card_bg);
        cv.stroke_rounded_rect(bar, 10, pal.border_subtle);

        let run_all = Rect::new(bar.x + 10, bar.y + 8, 196, 30);
        let copy_rep = Rect::new(run_all.right() + 10, bar.y + 8, 152, 30);
        let save_rep = Rect::new(copy_rep.right() + 10, bar.y + 8, 168, 30);
        draw_button(cv, run_all, lang.pick("Mesurer tout le système", "Benchmark All Modules"), true, run_all.contains(mx, my), pal, &fonts.sm);
        draw_button(cv, copy_rep, lang.pick("Copier le rapport", "Copy Report"), false, copy_rep.contains(mx, my), pal, &fonts.sm);
        draw_button(cv, save_rep, lang.pick("Exporter ExoFS (/tmp)", "Export to ExoFS"), false, save_rep.contains(mx, my), pal, &fonts.sm);

        // Per-module run strip
        let mod_y = bar.bottom() + 10;
        let mw = ((c.w - pad * 2 - 4 * 8) / 5).max(110);
        for (i, &cat) in BenchCategory::ALL.iter().enumerate() {
            let r = Rect::new(c.x + pad + (i as i32) * (mw + 8), mod_y, mw, 30);
            let sel = self.bench_selected_mod == i;
            let lbl = match cat {
                BenchCategory::KernelSyscalls => lang.pick("Noyau & Syscalls", "Kernel & Syscalls"),
                BenchCategory::IpcSecurity => "IPC & ExoCordon",
                BenchCategory::MemoryCpu => lang.pick("Mémoire & CPU", "Memory & CPU"),
                BenchCategory::ExoFsStorage => lang.pick("Stockage ExoFS", "ExoFS Storage"),
                BenchCategory::GraphicsUi => lang.pick("Graphique 2D", "2D Graphics"),
            };
            draw_button(cv, r, lbl, sel, r.contains(mx, my), pal, &fonts.sm);
        }

        let main_y = mod_y + 42;
        let main_h = (c.bottom() - main_y - pad).max(180);

        if bench.reports.is_empty() {
            let empty = Rect::new(c.x + pad, main_y, c.w - pad * 2, main_h);
            cv.fill_rounded_rect(empty, 12, pal.card_bg);
            cv.stroke_rounded_rect(empty, 12, pal.border_subtle);
            let msg = lang.pick(
                "Cliquez sur « Mesurer tout le système » pour évaluer Noyau, IPC, Mémoire/CPU, ExoFS et Graphique 2D.",
                "Click 'Benchmark All Modules' to profile Kernel, IPC, Memory/CPU, ExoFS Storage, and 2D Graphics.",
            );
            cv.draw_text_ellipsis(&fonts.md, empty.x + 24, empty.y + 40, msg, empty.w - 48, pal.text_secondary);
            return;
        }

        // Left: Global Score radial gauge + Summary bars across all 5 modules
        let left_w = ((c.w - pad * 2 - gap) * 44) / 100;
        let right_w = c.w - pad * 2 - gap - left_w;

        let gauge_h = 152;
        draw_radial_gauge(
            cv,
            Rect::new(c.x + pad, main_y, left_w, gauge_h),
            bench.global_score as f32,
            if bench.global_score >= 80 { pal.success } else if bench.global_score >= 60 { pal.warning } else { pal.danger },
            &format!("{}", bench.grade()),
            &format!("{} : {}/100", lang.pick("Score global", "Global Score"), bench.global_score),
            &format!("{}.{} ms ({} modules)", bench.total_elapsed_us / 1000, (bench.total_elapsed_us % 1000) / 100, bench.reports.len()),
            pal,
            &fonts.sm,
            &fonts.hero,
        );

        let colors = [
            Color::rgb(0x63, 0x66, 0xF1),
            Color::rgb(0x38, 0xBD, 0xF8),
            Color::rgb(0x10, 0xB9, 0x81),
            Color::rgb(0xF5, 0x9E, 0x0B),
            Color::rgb(0xF4, 0x3F, 0x5E),
        ];
        let val_strs: Vec<String> = bench.reports.iter().map(|r| format!("{}/100", r.score_100)).collect();
        let bars: Vec<BarEntry<'_>> = bench
            .reports
            .iter()
            .enumerate()
            .map(|(i, r)| BarEntry {
                label: if lang.is_en() { r.category.title_en() } else { r.category.title_fr() },
                value_text: &val_strs[i],
                ratio: (r.score_100 as f32 / 100.0).clamp(0.05, 1.0),
                color: colors[i % colors.len()],
            })
            .collect();
        draw_horizontal_bars(
            cv,
            Rect::new(c.x + pad, main_y + gauge_h + gap, left_w, (main_h - gauge_h - gap).max(120)),
            lang.pick("Score par sous-système", "Score by subsystem"),
            &bars,
            Some(0.80),
            pal,
            &fonts.sm,
            &fonts.md,
        );

        // Right: Detailed metrics for selected module + Final Report preview
        let rcard = Rect::new(c.x + pad + left_w + gap, main_y, right_w, main_h);
        cv.fill_rounded_rect(rcard, 12, pal.card_bg);
        cv.stroke_rounded_rect(rcard, 12, pal.border_subtle);

        let sel_cat = BenchCategory::ALL.get(self.bench_selected_mod).copied().unwrap_or(BenchCategory::KernelSyscalls);
        if let Some(rep) = bench.reports.iter().find(|r| r.category == sel_cat).or_else(|| bench.reports.first()) {
            let title = if lang.is_en() { rep.category.title_en() } else { rep.category.title_fr() };
            cv.draw_text(&fonts.lg_bold, rcard.x + 16, rcard.y + 14, title, pal.text_primary);
            cv.draw_text(&fonts.sm, rcard.x + 16, rcard.y + 38, &format!("Score module : {}/100  •  Durée : {} µs", rep.score_100, rep.elapsed_us), pal.accent);

            let list_r = Rect::new(rcard.x + 8, rcard.y + 64, rcard.w - 16, (rcard.bottom() - (rcard.y + 64) - 32).max(60));
            let ch = (rep.metrics.len() as i32) * 60 + 8;
            let sy = self.scroll_bench.clamp(0, (ch - list_r.h).max(0));
            let prev_m = cv.push_clip(list_r);
            let mut my_pos = list_r.y + 4 - sy;
            for m in &rep.metrics {
                if my_pos + 56 >= list_r.y && my_pos <= list_r.bottom() {
                    let mbox = Rect::new(rcard.x + 14, my_pos, rcard.w - 34, 52);
                    cv.fill_rounded_rect(mbox, 9, pal.elevated_bg);
                    cv.stroke_rounded_rect(mbox, 9, pal.border_subtle);
                    cv.draw_text(&fonts.md, mbox.x + 12, mbox.y + 8, &m.name, pal.text_primary);
                    let vw = fonts.md.measure_width(&m.value_text);
                    cv.draw_text(&fonts.md, mbox.right() - vw - 12, mbox.y + 8, &m.value_text, pal.accent);
                    cv.draw_text_ellipsis(&fonts.sm, mbox.x + 12, mbox.y + 29, &m.detail, mbox.w - 92, pal.text_secondary);
                    let sc = format!("{}/100", m.score_100);
                    let sw = fonts.sm.measure_width(&sc);
                    cv.draw_text(&fonts.sm, mbox.right() - sw - 12, mbox.y + 29, &sc, if m.score_100 >= 80 { pal.success } else { pal.warning });
                }
                my_pos += 60;
            }
            cv.clip = prev_m;
            if let Some(sb) = compute_scrollbar(list_r, ch, sy) {
                draw_scrollbar(cv, sb, sb.track.inset(-3, 0).contains(mx, my), self.dragging_scrollbar, pal);
            }

            if !bench.last_export_path.is_empty() {
                cv.draw_text_ellipsis(&fonts.sm, rcard.x + 16, rcard.bottom() - 26, &format!("Rapport enregistré : {}", bench.last_export_path), rcard.w - 32, pal.success);
            }
        }
    }

    pub(crate) fn click_bench(&mut self, c: Rect, mx: i32, my: i32, bench: &BenchSuite) -> AppAction {
        let pad = 16;
        let bar = Rect::new(c.x + pad, c.y + pad, c.w - pad * 2, 46);
        let run_all = Rect::new(bar.x + 10, bar.y + 8, 196, 30);
        let copy_rep = Rect::new(run_all.right() + 10, bar.y + 8, 152, 30);
        let save_rep = Rect::new(copy_rep.right() + 10, bar.y + 8, 168, 30);

        if run_all.contains(mx, my) { return AppAction::RunBenchAll; }
        if copy_rep.contains(mx, my) {
            if bench.reports.is_empty() { return AppAction::RunBenchAll; }
            return AppAction::CopyText(bench.format_final_report("exo-workstation"));
        }
        if save_rep.contains(mx, my) { return AppAction::ExportBenchReport; }

        let mod_y = bar.bottom() + 10;
        let mw = ((c.w - pad * 2 - 4 * 8) / 5).max(110);
        for (i, &cat) in BenchCategory::ALL.iter().enumerate() {
            if Rect::new(c.x + pad + (i as i32) * (mw + 8), mod_y, mw, 30).contains(mx, my) {
                self.bench_selected_mod = i;
                self.scroll_bench = 0;
                if !bench.reports.iter().any(|r| r.category == cat) {
                    return AppAction::RunBenchModule(cat);
                }
                return AppAction::Redraw;
            }
        }
        AppAction::None
    }
}
