//! System Monitor Overview & Live Graphs tabs (radial gauges, 6-axis radar, area sparklines, bars).

use crate::apps::monitor::MonitorApp;
use crate::assets::FontSet;
use crate::i18n::Lang;
use crate::telemetry::TelemetryState;
use exo_ui::charts::{
    draw_area_chart, draw_hexagon_radar, draw_horizontal_bars, draw_module_map, draw_radial_gauge,
    BarEntry,
};
use exo_ui::{Canvas, Color, Palette, Rect};

impl MonitorApp {
    pub(crate) fn draw_overview(&self, cv: &mut Canvas<'_>, c: Rect, pal: &Palette, lang: Lang, fonts: &FontSet, t: &TelemetryState) {
        let pad = 16;
        let gap = 12;
        let gw = ((c.w - pad * 2 - gap * 3) / 4).max(120);
        let gh = 144;
        let gy = c.y + pad;

        let score = t.overall_health_score();
        let (up, tot) = t.critical_counts();
        let svc_pct = if tot > 0 { (up as f32 * 100.0) / tot as f32 } else { 100.0 };

        draw_radial_gauge(cv, Rect::new(c.x + pad, gy, gw, gh), score as f32, pal.success, &format!("{}%", score), lang.pick("Santé globale", "Overall health"), lang.pick("Système nominal", "Nominal"), pal, &fonts.sm, &fonts.hero);
        draw_radial_gauge(cv, Rect::new(c.x + pad + gw + gap, gy, gw, gh), t.cpu_load_pct as f32, Color::rgb(0x38, 0xBD, 0xF8), &format!("{}%", t.cpu_load_pct), lang.pick("Processeur", "CPU Load"), &format!("{} cœurs", t.cpu_cores), pal, &fonts.sm, &fonts.hero);
        draw_radial_gauge(cv, Rect::new(c.x + pad + (gw + gap) * 2, gy, gw, gh), t.mem_used_pct as f32, Color::rgb(0xA8, 0x55, 0xF7), &format!("{}%", t.mem_used_pct), lang.pick("Mémoire RAM", "Memory"), &format!("{} / {} Mio", t.used_ram_mb, t.total_ram_mb), pal, &fonts.sm, &fonts.hero);
        draw_radial_gauge(cv, Rect::new(c.x + pad + (gw + gap) * 3, gy, gw, gh), svc_pct, Color::rgb(0xF5, 0x9E, 0x0B), &format!("{}/{}", up, tot), lang.pick("Services critiques", "Core services"), "IPC actif", pal, &fonts.sm, &fonts.hero);

        let by = gy + gh + gap;
        let bh = (c.bottom() - by - pad).max(160);
        let left_w = ((c.w - pad * 2 - gap) * 44) / 100;
        let right_w = c.w - pad * 2 - gap - left_w;

        let axes = t.radar_scores();
        draw_hexagon_radar(cv, Rect::new(c.x + pad, by, left_w, bh), lang.pick("Équilibre des sous-systèmes", "Subsystem balance"), &axes, pal.accent, pal, &fonts.sm, &fonts.md);

        let rx = c.x + pad + left_w + gap;
        let map_h = (bh * 46) / 100;
        let nodes = [
            ("Noyau Ring 0", if t.phoenix_state >= 1 { "A/B prêt • VT-d" } else { "Dégradé" }, pal.success),
            ("Services Ring 1", if up >= tot { "Tous en ligne" } else { "Récupération" }, Color::rgb(0x38, 0xBD, 0xF8)),
            ("Interface UI", "Compositeur isolé", Color::rgb(0xA8, 0x55, 0xF7)),
        ];
        draw_module_map(cv, Rect::new(rx, by, right_w, map_h), lang.pick("État temps réel du micro-noyau", "Live Microkernel Status"), nodes, pal, &fonts.sm, &fonts.md);

        let last_sys = t.syscall_rate_hist.last().copied().unwrap_or(0);
        draw_area_chart(
            cv,
            Rect::new(rx, by + map_h + gap, right_w, (bh - map_h - gap).max(90)),
            &t.syscall_rate_hist,
            250,
            pal.accent,
            lang.pick("Appels système / tick", "Syscalls / tick"),
            &format!("{} /s", last_sys),
            pal,
            &fonts.sm,
            &fonts.md,
        );
    }

    pub(crate) fn draw_live_graphs(&self, cv: &mut Canvas<'_>, c: Rect, pal: &Palette, lang: Lang, fonts: &FontSet, t: &TelemetryState) {
        let pad = 16;
        let gap = 12;
        let hw = (c.w - pad * 2 - gap) / 2;
        let hh = (c.h - pad * 2 - gap) / 2;

        let sys_v = t.syscall_rate_hist.last().copied().unwrap_or(0);
        let ipc_v = t.ipc_rate_hist.last().copied().unwrap_or(0);

        draw_area_chart(cv, Rect::new(c.x + pad, c.y + pad, hw, hh), &t.syscall_rate_hist, 250, pal.accent, lang.pick("Débit Appels Système", "Syscall Throughput"), &format!("{} ({} tot.)", sys_v, t.dispatch_total), pal, &fonts.sm, &fonts.md);
        draw_area_chart(cv, Rect::new(c.x + pad + hw + gap, c.y + pad, hw, hh), &t.ipc_rate_hist, 160, Color::rgb(0x38, 0xBD, 0xF8), lang.pick("Trafic IPC ExoCordon", "IPC Traffic"), &format!("{} ({} env.)", ipc_v, t.ipc_sent), pal, &fonts.sm, &fonts.md);
        draw_area_chart(cv, Rect::new(c.x + pad, c.y + pad + hh + gap, hw, hh), &t.cpu_load_hist, 100, pal.success, lang.pick("Charge CPU (%)", "CPU Usage (%)"), &format!("{} %", t.cpu_load_pct), pal, &fonts.sm, &fonts.md);

        let tot = t.dispatch_total.max(1) as f32;
        let fast_t = format!("{}", t.dispatch_fast);
        let slow_t = format!("{}", t.dispatch_slow);
        let rpc_t = format!("{}", t.ipc_rpc);
        let fut_t = format!("{} / {}", t.futex_waits, t.futex_wakes);
        let drp_t = format!("{}", t.ipc_dropped);
        let bars = [
            BarEntry { label: "Fast-Path Syscalls", value_text: &fast_t, ratio: (t.dispatch_fast as f32 / tot).clamp(0.05, 1.0), color: pal.success },
            BarEntry { label: "Slow-Path Syscalls", value_text: &slow_t, ratio: (t.dispatch_slow as f32 / tot).clamp(0.05, 1.0), color: Color::rgb(0x38, 0xBD, 0xF8) },
            BarEntry { label: "Appels RPC Synchrones", value_text: &rpc_t, ratio: ((t.ipc_rpc as f32) / (t.ipc_sent.max(1) as f32)).clamp(0.05, 1.0), color: Color::rgb(0xA8, 0x55, 0xF7) },
            BarEntry { label: "Futex Waits / Wakes", value_text: &fut_t, ratio: 0.22, color: pal.warning },
            BarEntry { label: "Messages IPC perdus", value_text: &drp_t, ratio: if t.ipc_dropped == 0 { 0.02 } else { 0.7 }, color: if t.ipc_dropped == 0 { pal.success } else { pal.danger } },
        ];
        draw_horizontal_bars(cv, Rect::new(c.x + pad + hw + gap, c.y + pad + hh + gap, hw, hh), lang.pick("Répartition Noyau & IPC", "Kernel & IPC Breakdown"), &bars, None, pal, &fonts.sm, &fonts.md);
    }
}
