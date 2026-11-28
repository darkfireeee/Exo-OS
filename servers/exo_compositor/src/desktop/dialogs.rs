//! System Dialogs: Lock Screen (§3.2), ExoAuth Admin Elevation (§1.1), and Capability File Picker (§2.9).

use crate::assets::ICON_START_64;
use crate::desktop::state::{DesktopShell, PowerRequest};
use crate::desktop::window::AppInstance;
use exo_ui::widgets::draw_button;
use exo_ui::{Canvas, Color, Rect};

#[derive(Clone, Debug)]
pub enum ActiveDialog {
    AdminElevation {
        reason: String,
    },
    FilePicker {
        target_win_id: u32,
        app_name: String,
        cwd: String,
        entries: Vec<(String, String, bool)>,
        selected: Option<usize>,
    },
}

impl ActiveDialog {
    pub fn new_file_picker(target_win_id: u32, app_name: String) -> Self {
        let _ = std::fs::create_dir_all("/home/user/Notes");
        let start = if std::fs::read_dir("/home/user").is_ok() {
            String::from("/home/user")
        } else {
            String::from("/tmp")
        };
        let mut d = Self::FilePicker {
            target_win_id,
            app_name,
            cwd: start,
            entries: Vec::new(),
            selected: None,
        };
        d.reload_picker();
        d
    }

    pub fn reload_picker(&mut self) {
        if let Self::FilePicker { cwd, entries, selected, .. } = self {
            entries.clear();
            *selected = None;
            if cwd != "/" {
                entries.push((String::from(".."), parent_dir(cwd), true));
            }
            if let Ok(rd) = std::fs::read_dir(&*cwd) {
                let mut rows: Vec<(String, String, bool)> = rd
                    .flatten()
                    .map(|e| {
                        let n = e.file_name().to_string_lossy().into_owned();
                        let p = if *cwd == "/" { format!("/{}", n) } else { format!("{}/{}", cwd, n) };
                        let is_d = e.metadata().map(|m| m.is_dir()).unwrap_or(false);
                        (n, p, is_d)
                    })
                    .filter(|(n, _, _)| !n.starts_with('.'))
                    .collect();
                rows.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
                entries.extend(rows);
            }
        }
    }
}

fn parent_dir(cwd: &str) -> String {
    let tr = cwd.trim_end_matches('/');
    match tr.rfind('/') {
        Some(0) | None => String::from("/"),
        Some(p) => String::from(&tr[..p]),
    }
}

fn fast_sin_cos(mut angle: f32) -> (f32, f32) {
    const PI: f32 = core::f32::consts::PI;
    const TAU: f32 = core::f32::consts::TAU;
    while angle > PI {
        angle -= TAU;
    }
    while angle < -PI {
        angle += TAU;
    }
    // Bhaskara / parabolic sine + quadrature cosine (pure Rust, zero libm)
    let sin_approx = |x: f32| -> f32 {
        const B: f32 = 4.0 / PI;
        const C: f32 = -4.0 / (PI * PI);
        let y = B * x + C * x * x.abs();
        0.225 * (y * y.abs() - y) + y
    };
    let s = sin_approx(angle);
    let mut ca = angle + PI * 0.5;
    if ca > PI {
        ca -= TAU;
    }
    (s, sin_approx(ca))
}

impl DesktopShell {
    pub(crate) fn draw_lock_screen(&self, cv: &mut Canvas<'_>) {
        let full = Rect::new(0, 0, self.width as i32, self.height as i32);
        if self.theme.blur_enabled {
            cv.blur_rect_fast(full, 5);
        }
        cv.fill_rect(full, Color::rgba(0x0B, 0x0D, 0x12, 0xB8));

        let cx = self.width as i32 / 2;
        let cy = self.height as i32 / 2;

        let time = self.telemetry.clock.format_time(self.cfg.use_24h);
        let tw = self.fonts.hero.measure_width(&time);
        cv.draw_text(&self.fonts.hero, cx - tw / 2, cy - 110, &time, Color::WHITE);

        let date = self.telemetry.clock.format_date(self.cfg.lang);
        let dw = self.fonts.lg_bold.measure_width(&date);
        cv.draw_text(&self.fonts.lg_bold, cx - dw / 2, cy - 72, &date, Color::rgb(0xD1, 0xD5, 0xDB));

        // User avatar + unlock card
        cv.fill_circle_aa(cx as f32, (cy - 10) as f32, 30.0, self.theme.accent_primary());
        let init_ch = self.cfg.username.chars().next().unwrap_or('U').to_ascii_uppercase().to_string();
        let iw = self.fonts.hero.measure_width(&init_ch);
        cv.draw_text(&self.fonts.hero, cx - iw / 2, cy - 24, &init_ch, Color::WHITE);
        let uw = self.fonts.md.measure_width(&self.cfg.username);
        cv.draw_text(&self.fonts.md, cx - uw / 2, cy + 32, &self.cfg.username, Color::WHITE);

        let pal = self.theme.palette();
        let btn = Rect::new(cx - 104, cy + 68, 208, 38);
        draw_button(
            cv,
            btn,
            self.cfg.lang.pick("Déverrouiller (Entrée)", "Unlock (Enter)"),
            true,
            btn.contains(self.mouse_x, self.mouse_y),
            &pal,
            &self.fonts.md,
        );
    }

    pub(crate) fn click_lock_screen(&mut self, mx: i32, my: i32) {
        let cx = self.width as i32 / 2;
        let cy = self.height as i32 / 2;
        if Rect::new(cx - 104, cy + 68, 208, 38).contains(mx, my) {
            self.locked = false;
            self.dirty = true;
        }
    }

    pub(crate) fn draw_active_dialog(&self, cv: &mut Canvas<'_>) {
        let Some(ref dlg) = self.dialog else { return; };
        let pal = self.theme.palette();
        let lang = self.cfg.lang;
        cv.fill_rect(Rect::new(0, 0, self.width as i32, self.height as i32), Color::rgba(0, 0, 0, 0x72));

        match dlg {
            ActiveDialog::AdminElevation { reason } => {
                let w = 440;
                let h = 210;
                let r = Rect::new((self.width as i32 - w) / 2, (self.height as i32 - h) / 2, w, h);
                cv.draw_panel(r, 14, pal.menu_bg, pal.border_strong, false, true);
                cv.draw_text(&self.fonts.lg_bold, r.x + 20, r.y + 18, lang.pick("Autorisation administrateur", "Administrator Authorization"), pal.text_primary);
                cv.draw_text(&self.fonts.sm, r.x + 20, r.y + 46, "ExoAuth — Élévation explicite requise", pal.warning);
                cv.draw_text_ellipsis(&self.fonts.md, r.x + 20, r.y + 82, reason, r.w - 40, pal.text_secondary);
                cv.draw_text(&self.fonts.sm, r.x + 20, r.y + 112, &format!("Session : {} (Propriétaire)", self.cfg.username), pal.text_muted);

                let cancel = Rect::new(r.right() - 252, r.bottom() - 48, 108, 32);
                let ok = Rect::new(r.right() - 134, r.bottom() - 48, 114, 32);
                draw_button(cv, cancel, lang.pick("Refuser", "Deny"), false, cancel.contains(self.mouse_x, self.mouse_y), &pal, &self.fonts.sm);
                draw_button(cv, ok, lang.pick("Autoriser", "Authorize"), true, ok.contains(self.mouse_x, self.mouse_y), &pal, &self.fonts.sm);
            }
            ActiveDialog::FilePicker { app_name, cwd, entries, selected, .. } => {
                let w = 520;
                let h = 390;
                let r = Rect::new((self.width as i32 - w) / 2, (self.height as i32 - h) / 2, w, h);
                cv.draw_panel(r, 14, pal.menu_bg, pal.border_strong, false, true);
                cv.draw_text(&self.fonts.lg_bold, r.x + 20, r.y + 16, lang.pick("Sélecteur de fichiers système", "System File Picker"), pal.text_primary);
                cv.draw_text_ellipsis(
                    &self.fonts.sm,
                    r.x + 20,
                    r.y + 42,
                    &format!("« {} » demande l'accès en lecture au fichier sélectionné", app_name),
                    r.w - 40,
                    pal.accent,
                );
                cv.draw_text(&self.fonts.sm, r.x + 20, r.y + 64, &format!("Dossier : {}", cwd), pal.text_muted);

                let list_r = Rect::new(r.x + 20, r.y + 88, r.w - 40, 232);
                cv.fill_rounded_rect(list_r, 9, pal.input_bg);
                cv.stroke_rounded_rect(list_r, 9, pal.border_subtle);
                let prev = cv.push_clip(list_r);
                let mut ry = list_r.y + 6;
                for (i, (name, _, is_d)) in entries.iter().enumerate().take(8) {
                    let row = Rect::new(list_r.x + 6, ry, list_r.w - 12, 26);
                    if *selected == Some(i) {
                        cv.fill_rounded_rect(row, 6, pal.accent.with_alpha(0x2C));
                    } else if row.contains(self.mouse_x, self.mouse_y) {
                        cv.fill_rounded_rect(row, 6, pal.hover_bg);
                    }
                    let label = if *is_d { format!("[Dossier] {}", name) } else { name.clone() };
                    cv.draw_text_ellipsis(&self.fonts.sm, row.x + 10, row.y + 5, &label, row.w - 20, pal.text_primary);
                    ry += 28;
                }
                cv.clip = prev;

                let cancel = Rect::new(r.right() - 294, r.bottom() - 48, 100, 32);
                let grant = Rect::new(r.right() - 184, r.bottom() - 48, 164, 32);
                draw_button(cv, cancel, lang.pick("Annuler", "Cancel"), false, cancel.contains(self.mouse_x, self.mouse_y), &pal, &self.fonts.sm);
                draw_button(cv, grant, lang.pick("Accorder et ouvrir", "Grant & Open"), true, grant.contains(self.mouse_x, self.mouse_y), &pal, &self.fonts.sm);
            }
        }
    }

    pub(crate) fn click_active_dialog(&mut self, mx: i32, my: i32) -> bool {
        let Some(dlg) = self.dialog.clone() else { return false; };
        match dlg {
            ActiveDialog::AdminElevation { .. } => {
                let r = Rect::new((self.width as i32 - 440) / 2, (self.height as i32 - 210) / 2, 440, 210);
                if Rect::new(r.right() - 252, r.bottom() - 48, 108, 32).contains(mx, my) {
                    self.dialog = None;
                    self.show_toast("Élévation refusée");
                } else if Rect::new(r.right() - 134, r.bottom() - 48, 114, 32).contains(mx, my) {
                    self.cfg.admin_unlocked = true;
                    self.dialog = None;
                    self.show_toast("Session administrateur autorisée (ExoAuth)");
                }
                self.dirty = true;
                true
            }
            ActiveDialog::FilePicker { target_win_id, entries, selected, .. } => {
                let r = Rect::new((self.width as i32 - 520) / 2, (self.height as i32 - 390) / 2, 520, 390);
                let list_r = Rect::new(r.x + 20, r.y + 88, r.w - 40, 232);
                if list_r.contains(mx, my) {
                    let mut ry = list_r.y + 6;
                    for (i, (_, path, is_d)) in entries.iter().enumerate().take(8) {
                        if Rect::new(list_r.x + 6, ry, list_r.w - 12, 26).contains(mx, my) {
                            if *is_d {
                                if let Some(ActiveDialog::FilePicker { cwd, .. }) = self.dialog.as_mut() {
                                    *cwd = path.clone();
                                }
                                if let Some(d) = self.dialog.as_mut() { d.reload_picker(); }
                            } else if let Some(ActiveDialog::FilePicker { selected: ref mut s, .. }) = self.dialog.as_mut() {
                                *s = Some(i);
                            }
                            self.dirty = true;
                            return true;
                        }
                        ry += 28;
                    }
                }
                if Rect::new(r.right() - 294, r.bottom() - 48, 100, 32).contains(mx, my) {
                    self.dialog = None;
                    self.dirty = true;
                    return true;
                }
                if Rect::new(r.right() - 184, r.bottom() - 48, 164, 32).contains(mx, my) {
                    if let Some(idx) = selected {
                        if let Some((_, path, false)) = entries.get(idx) {
                            if let Some(w) = self.windows.iter_mut().find(|w| w.id == target_win_id) {
                                if let AppInstance::Editor(ref mut ed) = w.app {
                                    ed.load_file(path);
                                    w.title = format!("{} — {}", w.kind.title(self.cfg.lang), path);
                                }
                            }
                            self.dialog = None;
                            self.show_toast(format!("Capacité lecture accordée : {}", path));
                        }
                    }
                    self.dirty = true;
                    return true;
                }
                true
            }
        }
    }

    /// Smooth 60 Hz full-screen shutdown / reboot transition overlay.
    pub(crate) fn draw_power_transition(&self, cv: &mut Canvas<'_>) {
        let Some(pt) = self.power_transition else { return; };
        let w = self.width as i32;
        let h = self.height as i32;
        let full = Rect::new(0, 0, w, h);
        let p = pt.progress.clamp(0.0, 1.0);
        let is_reboot = pt.req == PowerRequest::Reboot;

        // 1. Smooth ease-out obsidian backdrop veil over the live desktop
        let veil_t = (p / 0.28).clamp(0.0, 1.0);
        let veil_ease = 1.0 - (1.0 - veil_t) * (1.0 - veil_t) * (1.0 - veil_t);
        let veil_alpha = (veil_ease * 246.0) as u8;
        cv.fill_rect(full, Color::rgba(0x07, 0x0A, 0x12, veil_alpha));

        // 2. Center emblem, 60 Hz orbiting spinner ring, bilingual status, and progress pill
        let fade_in = ((p - 0.08) / 0.22).clamp(0.0, 1.0);
        let fade_out = (1.0 - ((p - 0.80) / 0.18).clamp(0.0, 1.0)).clamp(0.0, 1.0);
        let content_vis = fade_in * fade_out;

        if content_vis > 0.02 {
            let cx = w / 2;
            let cy = h / 2 - 26;
            let accent = if is_reboot {
                Color::rgb(0x38, 0xBD, 0xF8) // Sky cyan for Reboot
            } else {
                Color::rgb(0x63, 0x66, 0xF1) // Signature ExoOS Indigo for Shutdown
            };
            let glow_a = (content_vis * 34.0) as u8;
            cv.fill_circle_aa(cx as f32, cy as f32, 82.0, accent.with_alpha(glow_a / 2));
            cv.fill_circle_aa(cx as f32, cy as f32, 60.0, accent.with_alpha(glow_a));

            // Dark glass medallion + crisp native 64x64 ExoOS hex emblem
            cv.fill_circle_aa(cx as f32, cy as f32, 42.0, Color::rgba(0x11, 0x16, 0x24, (content_vis * 238.0) as u8));
            let ico_a = (content_vis * 255.0) as u8;
            cv.blit_rgba(cx - 32, cy - 32, 64, 64, ICON_START_64, 64, 64, ico_a);

            // 12-bead orbiting 60 Hz halo ring around the emblem
            const DOTS: usize = 12;
            let base_angle = p * core::f32::consts::TAU * 2.25;
            for i in 0..DOTS {
                let frac = i as f32 / DOTS as f32;
                let ang = base_angle - frac * core::f32::consts::TAU;
                let (sn, cs) = fast_sin_cos(ang);
                let dx = cx as f32 + cs * 58.0;
                let dy = cy as f32 + sn * 58.0;
                let inv = 1.0 - frac;
                let trail = inv * inv;
                let r = 2.0 + trail * 2.6;
                let a = (content_vis * (36.0 + trail * 219.0)) as u8;
                let col = if i < 3 { Color::rgb(0x93, 0xC5, 0xFD) } else { accent };
                cv.fill_circle_aa(dx, dy, r, col.with_alpha(a));
            }

            let lang = self.cfg.lang;
            let title = if is_reboot {
                lang.pick("Redémarrage d'ExoOS...", "Restarting ExoOS...")
            } else {
                lang.pick("Arrêt d'ExoOS...", "Shutting down ExoOS...")
            };
            let status = if p < 0.36 {
                lang.pick("Fermeture des applications et services...", "Closing applications and services...")
            } else if p < 0.72 {
                lang.pick("Synchronisation sécurisée d'ExoFS...", "Syncing ExoFS volumes...")
            } else if is_reboot {
                lang.pick("Réinitialisation matérielle...", "Performing hardware reset...")
            } else {
                lang.pick("Mise hors tension ACPI...", "Powering off via ACPI...")
            };

            let title_col = Color::rgba(0xF8, 0xFA, 0xFC, ico_a);
            let sub_col = Color::rgba(0x94, 0xA3, 0xB8, (content_vis * 235.0) as u8);
            let tw = self.fonts.hero.measure_width(title);
            cv.draw_text(&self.fonts.hero, cx - tw / 2, cy + 78, title, title_col);
            let sw = self.fonts.md.measure_width(status);
            cv.draw_text(&self.fonts.md, cx - sw / 2, cy + 114, status, sub_col);

            // Smooth cubic-eased progress pill bar
            let bar_w = 260;
            let bar_r = Rect::new(cx - bar_w / 2, cy + 148, bar_w, 5);
            cv.fill_rounded_rect(bar_r, 3, Color::rgba(0xFF, 0xFF, 0xFF, (content_vis * 32.0) as u8));
            let eased_p = if p < 0.5 {
                2.0 * p * p
            } else {
                let q = -2.0 * p + 2.0;
                1.0 - (q * q) * 0.5
            };
            let fill_w = (((bar_w as f32) * eased_p + 0.5) as i32).clamp(12, bar_w);
            cv.fill_rounded_rect(Rect::new(bar_r.x, bar_r.y, fill_w, 5), 3, accent.with_alpha(ico_a));
        }

        // 3. Final fade-to-pure-black curtain before hardware poweroff / reset
        if p > 0.82 {
            let black_t = ((p - 0.82) / 0.18).clamp(0.0, 1.0);
            let black_a = (black_t * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
            cv.fill_rect(full, Color::rgba(0x00, 0x00, 0x00, black_a));
        }
    }
}
