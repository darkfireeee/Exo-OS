//! Properties & ExoFS Capabilities inspector side-pane (ExoOS UI Spec §2.4).

use crate::apps::files::{format_bytes, FilesApp};
use crate::assets::FontSet;
use crate::i18n::Lang;
use exo_ui::widgets::{draw_badge, draw_button};
use exo_ui::{Canvas, Palette, Rect};

impl FilesApp {
    pub(crate) fn draw_inspector(
        &self,
        cv: &mut Canvas<'_>,
        insp: Rect,
        mx: i32,
        my: i32,
        pal: &Palette,
        lang: Lang,
        fonts: &FontSet,
    ) {
        cv.fill_rect(insp, pal.sidebar_bg);
        cv.fill_rect(Rect::new(insp.x, insp.y, 1, insp.h), pal.border_subtle);

        let mut iy = insp.y + 14;
        cv.draw_text(&fonts.sm, insp.x + 14, iy, lang.pick("PROPRIÉTÉS & CAPACITÉS", "PROPERTIES & CAPS"), pal.text_muted);
        iy += 24;

        let name = self.inspector_path.rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or("/");
        cv.draw_text_ellipsis(&fonts.md, insp.x + 14, iy, name, insp.w - 28, pal.text_primary);
        iy += 28;

        if let Some(ref info) = self.inspector_info {
            let mut bx = insp.x + 14;
            bx += draw_badge(cv, bx, iy, "BLAKE3", pal.success, &fonts.sm) + 6;
            if info.read_only_zone {
                draw_badge(cv, bx, iy, lang.pick("Lecture seule", "Read-only"), pal.warning, &fonts.sm);
            } else {
                draw_badge(cv, bx, iy, "ExoFS", pal.accent, &fonts.sm);
            }
            iy += 34;

            let rows = [
                (lang.pick("Taille", "Size"), format_bytes(info.size)),
                (lang.pick("Capacités", "Capabilities"), info.cap_summary.clone()),
                (lang.pick("Époque", "Epoch"), format!("#{}", info.generation)),
                ("Object ID", info.object_id_hex.chars().take(16).collect()),
                ("Hash", info.content_hash_hex.chars().take(18).collect()),
            ];
            for (k, v) in rows {
                cv.draw_text(&fonts.sm, insp.x + 14, iy, k, pal.text_muted);
                iy += 17;
                cv.draw_text_ellipsis(&fonts.sm, insp.x + 14, iy, &v, insp.w - 28, pal.text_primary);
                iy += 24;
            }
        }

        let snap_btn = Rect::new(insp.x + 14, insp.bottom() - 44, insp.w - 28, 30);
        draw_button(
            cv,
            snap_btn,
            lang.pick("Créer un instantané", "Create Snapshot"),
            false,
            snap_btn.contains(mx, my),
            pal,
            &fonts.sm,
        );
    }
}
