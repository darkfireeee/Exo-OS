//! Clean flagship window chrome (neutral Dark & Light, snapping, resize, no tech clutter).

use crate::apps::editor::EditorApp;
use crate::apps::files::FilesApp;
use crate::apps::games::GamesApp;
use crate::apps::monitor::MonitorApp;
use crate::apps::settings::{SettingsApp, SystemConfig};
use crate::apps::terminal::TerminalApp;
use crate::apps::AppKind;
use crate::assets::FontSet;
use crate::bench::BenchSuite;
use crate::telemetry::TelemetryState;
use exo_ui::{Canvas, Color, Rect, Theme};

pub const TITLEBAR_H: i32 = 38;
pub const MIN_WIN_W: i32 = 480;
pub const MIN_WIN_H: i32 = 320;

pub enum AppInstance {
    Files(FilesApp),
    Terminal(TerminalApp),
    Monitor(MonitorApp),
    Settings(SettingsApp),
    Editor(EditorApp),
    Games(GamesApp),
}

pub struct Window {
    pub id: u32,
    pub kind: AppKind,
    pub title: String,
    pub rect: Rect,
    pub saved_rect: Rect,
    pub minimized: bool,
    pub maximized: bool,
    pub z_order: u32,
    pub focused: bool,
    pub app: AppInstance,
    pub(crate) cache_pixels: Vec<u32>,
    pub(crate) cache_w: i32,
    pub(crate) cache_h: i32,
    pub(crate) cache_dirty: bool,
}

impl Window {
    pub fn new(id: u32, kind: AppKind, title: String, rect: Rect, z_order: u32, app: AppInstance) -> Self {
        Self {
            id,
            kind,
            title,
            rect,
            saved_rect: rect,
            minimized: false,
            maximized: false,
            z_order,
            focused: true,
            app,
            cache_pixels: Vec::new(),
            cache_w: 0,
            cache_h: 0,
            cache_dirty: true,
        }
    }

    #[inline]
    pub fn invalidate_cache(&mut self) {
        self.cache_dirty = true;
    }

    #[inline]
    pub fn titlebar_rect(&self) -> Rect {
        Rect::new(self.rect.x, self.rect.y, self.rect.w, TITLEBAR_H)
    }

    #[inline]
    pub fn body_rect(&self) -> Rect {
        Rect::new(
            self.rect.x + 1,
            self.rect.y + TITLEBAR_H,
            (self.rect.w - 2).max(1),
            (self.rect.h - TITLEBAR_H - 1).max(1),
        )
    }

    /// Exact guaranteed-opaque interior rectangle used for front-to-back occlusion culling.
    /// Excludes the 12px rounded corner arcs and 1px hairline border on non-maximized windows.
    #[inline]
    pub fn opaque_rect(&self) -> Rect {
        if self.maximized {
            self.rect
        } else {
            Rect::new(
                self.rect.x + 1,
                self.rect.y + 12,
                (self.rect.w - 2).max(0),
                (self.rect.h - 24).max(0),
            )
        }
    }

    #[inline]
    pub fn close_btn(&self) -> Rect {
        Rect::new(self.rect.right() - 34, self.rect.y + 8, 24, 22)
    }

    #[inline]
    pub fn max_btn(&self) -> Rect {
        Rect::new(self.rect.right() - 62, self.rect.y + 8, 24, 22)
    }

    #[inline]
    pub fn min_btn(&self) -> Rect {
        Rect::new(self.rect.right() - 90, self.rect.y + 8, 24, 22)
    }

    #[inline]
    pub fn resize_grip(&self) -> Rect {
        Rect::new(self.rect.right() - 22, self.rect.bottom() - 22, 22, 22)
    }

    pub fn toggle_maximize(&mut self, work_area: Rect) {
        if self.maximized {
            self.rect = self.saved_rect;
            self.maximized = false;
        } else {
            self.saved_rect = self.rect;
            self.rect = work_area;
            self.maximized = true;
        }
        self.cache_dirty = true;
    }

    pub fn snap_left(&mut self, work_area: Rect) {
        if !self.maximized {
            self.saved_rect = self.rect;
        }
        self.maximized = false;
        self.rect = Rect::new(work_area.x, work_area.y, work_area.w / 2, work_area.h);
        self.cache_dirty = true;
    }

    pub fn snap_right(&mut self, work_area: Rect) {
        if !self.maximized {
            self.saved_rect = self.rect;
        }
        self.maximized = false;
        let hw = work_area.w / 2;
        self.rect = Rect::new(work_area.x + hw, work_area.y, work_area.w - hw, work_area.h);
        self.cache_dirty = true;
    }

    /// Re-rasterizes this window's private backing surface ONLY when invalidated or resized.
    /// Never wastes CPU re-rendering unchanged focused or background windows!
    #[allow(clippy::too_many_arguments)]
    pub fn ensure_cache(
        &mut self,
        mx: i32,
        my: i32,
        theme: &Theme,
        cfg: &SystemConfig,
        fonts: &FontSet,
        telemetry: &TelemetryState,
        bench: &BenchSuite,
    ) {
        let w = self.rect.w.max(1);
        let h = self.rect.h.max(1);
        let need_len = (w as usize).saturating_mul(h as usize);
        let cache_valid = !self.cache_dirty
            && self.cache_w == w
            && self.cache_h == h
            && self.cache_pixels.len() == need_len;
        if cache_valid {
            return;
        }
        let mut buf = core::mem::take(&mut self.cache_pixels);
        if buf.len() != need_len {
            buf.resize(need_len, 0xFF18_1A20);
        }
        self.cache_w = w;
        self.cache_h = h;
        let orig_rect = self.rect;
        self.rect = Rect::new(0, 0, w, h);
        {
            let mut lcv = Canvas::new(&mut buf, w as u32, h as u32, w as usize);
            let pal = theme.palette();
            lcv.fill_rect(Rect::new(0, 0, w, h), pal.window_bg.with_alpha(255));
            self.draw_inner(&mut lcv, mx - orig_rect.x, my - orig_rect.y, theme, cfg, fonts, telemetry, bench, false);
        }
        self.rect = orig_rect;
        self.cache_pixels = buf;
        self.cache_dirty = false;
    }

    /// Ultra-fast clipped scanline blit from this window's cached backing surface (zero overdraw shadow).
    pub fn blit_cached_clipped(&self, cv: &mut Canvas<'_>, theme: &Theme, draw_shadow: bool) {
        if self.minimized || self.cache_pixels.is_empty() {
            return;
        }
        let w = self.cache_w;
        let h = self.cache_h;
        let clip = cv.clip;
        let clip_inside_rect = clip.x >= self.rect.x
            && clip.y >= self.rect.y
            && clip.right() <= self.rect.right()
            && clip.bottom() <= self.rect.bottom();

        // Non-overlapping 2-ring outer drop shadow (zero double-blending, skipped when clip is interior)
        if draw_shadow && !self.maximized && !clip_inside_rect {
            let r = self.rect;
            // Ring 1 (inner 4px): strictly outside r, shifted +3px down
            let c1 = Color::rgba(0, 0, 0, 26);
            let i_x0 = r.x - 4;
            let i_x1 = r.right() + 4;
            let i_y0 = r.y - 1;
            let i_y1 = r.bottom() + 7;
            cv.fill_rect(Rect::new(i_x0, i_y0, i_x1 - i_x0, (r.y - i_y0).max(0)), c1);
            cv.fill_rect(Rect::new(i_x0, r.bottom(), i_x1 - i_x0, i_y1 - r.bottom()), c1);
            cv.fill_rect(Rect::new(i_x0, r.y, 4, r.h), c1);
            cv.fill_rect(Rect::new(r.right(), r.y, 4, r.h), c1);

            // Ring 2 (outer 4px): strictly outside Ring 1 (zero overlap!)
            let c2 = Color::rgba(0, 0, 0, 11);
            let o_x0 = i_x0 - 4;
            let o_x1 = i_x1 + 4;
            let o_y0 = i_y0 - 3;
            let o_y1 = i_y1 + 5;
            cv.fill_rect(Rect::new(o_x0, o_y0, o_x1 - o_x0, (i_y0 - o_y0).max(0)), c2);
            cv.fill_rect(Rect::new(o_x0, i_y1, o_x1 - o_x0, o_y1 - i_y1), c2);
            cv.fill_rect(Rect::new(o_x0, i_y0, 4, i_y1 - i_y0), c2);
            cv.fill_rect(Rect::new(i_x1, i_y0, 4, i_y1 - i_y0), c2);
        }

        // Fast clipped scanline memcpy from cached window backing surface
        let rad = if self.maximized { 0 } else { 12 };
        let vis = self.rect.intersect(clip);
        if vis.is_empty() {
            return;
        }
        const CORNER_INSET_12: [i32; 12] = [7, 5, 4, 3, 2, 2, 1, 1, 1, 0, 0, 0];
        let stride_dst = cv.stride;
        let stride_src = w as usize;
        let src_len = self.cache_pixels.len();
        let dst_len = cv.pixels.len();
        for gy in vis.y..vis.bottom() {
            let ly = gy - self.rect.y;
            let inset = if rad == 12 {
                if ly < 12 {
                    CORNER_INSET_12[ly as usize]
                } else if ly >= h - 12 {
                    CORNER_INSET_12[(h - 1 - ly) as usize]
                } else {
                    0
                }
            } else {
                0
            };
            let gx0 = vis.x.max(self.rect.x + inset);
            let gx1 = vis.right().min(self.rect.right() - inset);
            if gx0 < gx1 {
                let lx0 = (gx0 - self.rect.x) as usize;
                let count = (gx1 - gx0) as usize;
                let src_off = (ly as usize) * stride_src + lx0;
                let dst_off = (gy as usize) * stride_dst + (gx0 as usize);
                if src_off + count <= src_len && dst_off + count <= dst_len {
                    cv.pixels[dst_off..dst_off + count]
                        .copy_from_slice(&self.cache_pixels[src_off..src_off + count]);
                }
            }
        }
        // Only stroke rounded border if clip actually touches the window perimeter
        let touches_border = clip.x <= self.rect.x + rad
            || clip.y <= self.rect.y + rad
            || clip.right() >= self.rect.right() - rad
            || clip.bottom() >= self.rect.bottom() - rad;
        if touches_border {
            let pal = theme.palette();
            let border = if self.focused { pal.border_strong } else { pal.border_subtle };
            cv.stroke_rounded_rect(self.rect, rad, border);
        }
    }

    /// Cached 60 Hz compositor path: blits backing surface via scanline memcpy.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_cached(
        &mut self,
        cv: &mut Canvas<'_>,
        mx: i32,
        my: i32,
        theme: &Theme,
        cfg: &SystemConfig,
        fonts: &FontSet,
        telemetry: &TelemetryState,
        bench: &BenchSuite,
        dragging_this: bool,
    ) {
        if self.minimized {
            return;
        }
        // Live frosted-glass path when user lowered window opacity below 92% (and not mid-drag)
        if theme.surface_alpha < 235 && !dragging_this {
            self.draw(cv, mx, my, theme, cfg, fonts, telemetry, bench);
            return;
        }
        self.ensure_cache(mx, my, theme, cfg, fonts, telemetry, bench);
        self.blit_cached_clipped(cv, theme, true);
    }

    pub fn draw(
        &self,
        cv: &mut Canvas<'_>,
        mx: i32,
        my: i32,
        theme: &Theme,
        cfg: &SystemConfig,
        fonts: &FontSet,
        telemetry: &TelemetryState,
        bench: &BenchSuite,
    ) {
        if self.minimized {
            return;
        }
        self.draw_inner(cv, mx, my, theme, cfg, fonts, telemetry, bench, true);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_inner(
        &self,
        cv: &mut Canvas<'_>,
        mx: i32,
        my: i32,
        theme: &Theme,
        cfg: &SystemConfig,
        fonts: &FontSet,
        telemetry: &TelemetryState,
        bench: &BenchSuite,
        with_outer_panel: bool,
    ) {
        let pal = theme.palette();
        let rad = if self.maximized { 0 } else { 12 };
        let border = if self.focused { pal.border_strong } else { pal.border_subtle };

        if with_outer_panel {
            cv.draw_panel(self.rect, rad, pal.window_bg, border, theme.blur_enabled, !self.maximized);
        }

        let tb = self.titlebar_rect();
        cv.fill_rect(Rect::new(tb.x + 1, tb.bottom() - 1, (tb.w - 2).max(1), 1), pal.border_subtle);

        // App icon + clean title (zero technical clutter)
        cv.blit_rgba(tb.x + 12, tb.y + 8, 22, 22, self.kind.icon().rgba_40(), 40, 40, 255);
        cv.draw_text_ellipsis(
            &fonts.md,
            tb.x + 42,
            tb.y + 10,
            &self.title,
            (tb.w - 150).max(80),
            if self.focused { pal.text_primary } else { pal.text_secondary },
        );

        // Minimal neutral window controls (Minimize, Maximize, Close)
        let min_r = self.min_btn();
        let max_r = self.max_btn();
        let cls_r = self.close_btn();

        let min_h = min_r.contains(mx, my);
        if min_h { cv.fill_rounded_rect(min_r, 6, pal.hover_bg); }
        cv.fill_rect(Rect::new(min_r.x + 7, min_r.y + 12, 10, 2), pal.text_secondary);

        let max_h = max_r.contains(mx, my);
        if max_h { cv.fill_rounded_rect(max_r, 6, pal.hover_bg); }
        cv.stroke_rounded_rect(Rect::new(max_r.x + 7, max_r.y + 6, 10, 10), 2, pal.text_secondary);

        let cls_h = cls_r.contains(mx, my);
        if cls_h {
            cv.fill_rounded_rect(cls_r, 6, pal.danger);
        } else {
            cv.fill_rounded_rect(cls_r, 6, pal.danger.with_alpha(0x1E));
        }
        let xcol = if cls_h { Color::WHITE } else { pal.danger };
        cv.draw_line_aa((cls_r.x + 8) as f32, (cls_r.y + 7) as f32, (cls_r.x + 16) as f32, (cls_r.y + 15) as f32, 1.5, xcol);
        cv.draw_line_aa((cls_r.x + 16) as f32, (cls_r.y + 7) as f32, (cls_r.x + 8) as f32, (cls_r.y + 15) as f32, 1.5, xcol);

        let body = self.body_rect();
        let prev = cv.push_clip(body);
        match &self.app {
            AppInstance::Files(a) => a.draw(cv, body, mx, my, theme, cfg.lang, fonts, telemetry),
            AppInstance::Terminal(a) => a.draw(cv, body, mx, my, theme, fonts),
            AppInstance::Monitor(a) => a.draw(cv, body, mx, my, theme, cfg.lang, fonts, telemetry, bench),
            AppInstance::Settings(a) => a.draw(cv, body, mx, my, theme, cfg, fonts, telemetry),
            AppInstance::Editor(a) => a.draw(cv, body, mx, my, theme, cfg.lang, fonts),
            AppInstance::Games(a) => a.draw(cv, body, mx, my, theme, cfg.lang, fonts),
        }
        cv.clip = prev;

        if !self.maximized {
            let rx = self.rect.right() - 12;
            let ry = self.rect.bottom() - 12;
            let dot = pal.text_muted.with_alpha(0x88);
            for (dx, dy) in [(6, 6), (2, 6), (6, 2)] {
                cv.fill_rect(Rect::new(rx + dx, ry + dy, 2, 2), dot);
            }
        }
    }
}
