//! ExoFS storage I/O & 2D Graphics/Compositor rasterization benchmarks.

use crate::assets::FontSet;
use crate::bench::kernel_ipc::{avg_score, score_lower_better_ns};
use crate::bench::{now_ns, BenchCategory, BenchMetric, ModuleBenchReport};
use exo_ui::{Canvas, Rect, Theme};

pub fn bench_exofs_storage() -> ModuleBenchReport {
    let t_start = now_ns();
    let mut metrics = Vec::new();
    let bench_dir = "/tmp/.exo_bench_tmp";
    let _ = std::fs::create_dir_all(bench_dir);

    // 1. Metadata create + write + read + stat + delete (20 files)
    let file_count = 20u64;
    let payload = [0x42u8; 4096];
    let t0 = now_ns();
    let mut ok_ops = 0u64;
    for i in 0..file_count {
        let p = format!("{}/f_{}.dat", bench_dir, i);
        if std::fs::write(&p, payload).is_ok() {
            if let Ok(read_back) = std::fs::read(&p) {
                if read_back.len() == payload.len() {
                    ok_ops += 1;
                }
            }
            let _ = std::fs::remove_file(&p);
        }
    }
    let dt_meta = (now_ns().saturating_sub(t0)).max(1);
    let us_per_file = (dt_meta / file_count.max(1)) / 1000;
    metrics.push(BenchMetric {
        name: String::from("Cycle ExoFS (Crée/Lit/Suppr)"),
        value_text: format!("{} µs/fich.", us_per_file.max(1)),
        score_100: if ok_ops == 0 { 40 } else { score_lower_better_ns(dt_meta / file_count, 40_000, 3_000_000) },
        detail: format!("{}/{} fichiers 4 Kio vérifiés", ok_ops, file_count),
    });

    // 2. Sequential 128 KiB write + read throughput
    let seq_path = format!("{}/seq_128k.bin", bench_dir);
    let big = vec![0x7Eu8; 128 * 1024];
    let t1 = now_ns();
    let w_ok = std::fs::write(&seq_path, &big).is_ok();
    let r_len = std::fs::read(&seq_path).map(|b| b.len()).unwrap_or(0);
    let _ = std::fs::remove_file(&seq_path);
    let _ = std::fs::remove_dir(bench_dir);
    let dt_seq = (now_ns().saturating_sub(t1)).max(1);
    let io_bytes = (big.len() as u128) * 2;
    let mib_s = ((io_bytes * 1_000_000_000) / ((dt_seq as u128) * 1_048_576)) as u64;
    metrics.push(BenchMetric {
        name: String::from("Débit Séquentiel ExoFS"),
        value_text: format!("{} Mio/s", mib_s.max(1)),
        score_100: if w_ok && r_len == big.len() { ((mib_s * 100) / 120).clamp(40, 100) as u32 } else { 40 },
        detail: String::from("Écriture + Lecture 128 Kio"),
    });

    // 3. Directory enumeration speed
    let t2 = now_ns();
    let entries = std::fs::read_dir("/")
        .map(|rd| rd.count())
        .unwrap_or(0);
    let dt_dir = (now_ns().saturating_sub(t2)).max(1);
    metrics.push(BenchMetric {
        name: String::from("Scan Répertoire Racine"),
        value_text: format!("{} µs", (dt_dir / 1000).max(1)),
        score_100: score_lower_better_ns(dt_dir, 50_000, 4_000_000),
        detail: format!("{} objets listés dans /", entries),
    });

    let score = avg_score(&metrics);
    ModuleBenchReport {
        category: BenchCategory::ExoFsStorage,
        score_100: score,
        elapsed_us: (now_ns().saturating_sub(t_start)) / 1000,
        metrics,
    }
}

pub fn bench_graphics_ui(fonts: &FontSet) -> ModuleBenchReport {
    let t_start = now_ns();
    let mut metrics = Vec::new();
    let (w, h) = (320u32, 200u32);
    let mut fb = vec![0xFF18_1A20u32; (w * h) as usize];
    let pal = Theme::new().palette();

    // 1. Alpha-blended SDF rounded rectangles (120 panels)
    let rect_iters = 120u64;
    let t0 = now_ns();
    {
        let mut cv = Canvas::new(&mut fb, w, h, w as usize);
        for i in 0..rect_iters {
            let x = ((i * 7) % 160) as i32;
            let y = ((i * 5) % 100) as i32;
            cv.fill_rounded_rect(Rect::new(x, y, 140, 80), 12, pal.accent.with_alpha(0x80));
        }
    }
    let dt_rect = (now_ns().saturating_sub(t0)).max(1);
    let rects_per_sec = (rect_iters * 1_000_000_000) / dt_rect;
    metrics.push(BenchMetric {
        name: String::from("Rendu Cartes SDF AA"),
        value_text: format!("{} /s", rects_per_sec),
        score_100: ((rects_per_sec * 100) / 18_000).clamp(40, 100) as u32,
        detail: String::from("120 cartes 140x80 alpha-blend"),
    });

    // 2. Anti-aliased text rasterization (200 lines)
    let text_iters = 200u64;
    let sample = "ExoOS Interface Moderne — Évaluation typographique 12345";
    let glyphs = sample.chars().count() as u64 * text_iters;
    let t1 = now_ns();
    {
        let mut cv = Canvas::new(&mut fb, w, h, w as usize);
        for i in 0..text_iters {
            cv.draw_text(&fonts.md, 8, ((i * 11) % 170) as i32, sample, pal.text_primary);
        }
    }
    let dt_txt = (now_ns().saturating_sub(t1)).max(1);
    let kglyphs_s = (glyphs * 1_000_000) / dt_txt;
    metrics.push(BenchMetric {
        name: String::from("Rastérisation Texte AA"),
        value_text: format!("{} kGlyphes/s", kglyphs_s.max(1)),
        score_100: ((kglyphs_s * 100) / 800).clamp(40, 100) as u32,
        detail: String::from("Atlas 8-bits Inter anti-aliasé"),
    });

    // 3. Backdrop blur throughput (6 passes x 320x200)
    let blur_iters = 6u64;
    let t2 = now_ns();
    {
        let mut cv = Canvas::new(&mut fb, w, h, w as usize);
        for _ in 0..blur_iters {
            cv.blur_rect_fast(Rect::new(0, 0, w as i32, h as i32), 4);
        }
    }
    let dt_blur = (now_ns().saturating_sub(t2)).max(1);
    let px_total = (w as u64) * (h as u64) * blur_iters;
    let mpx_s = (px_total * 1_000) / dt_blur.max(1);
    metrics.push(BenchMetric {
        name: String::from("Flou Séparable 2 Passes"),
        value_text: format!("{} MPx/s", mpx_s.max(1)),
        score_100: ((mpx_s * 100) / 35).clamp(40, 100) as u32,
        detail: String::from("Sous-échantillonné 2x virgule fixe + cache 2D"),
    });

    let score = avg_score(&metrics);
    ModuleBenchReport {
        category: BenchCategory::GraphicsUi,
        score_100: score,
        elapsed_us: (now_ns().saturating_sub(t_start)) / 1000,
        metrics,
    }
}
