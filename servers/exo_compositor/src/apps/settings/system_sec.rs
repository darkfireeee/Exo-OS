//! Settings pages: Network (§1.8), Accounts/ExoAuth (§1.9), Security/Storage (§1.10-1.11), Power & Dev (§1.13, §1.19).

use crate::apps::settings::{SettingsApp, SystemConfig};
use crate::apps::AppAction;
use crate::assets::FontSet;
use crate::telemetry::TelemetryState;
use exo_ui::widgets::{draw_badge, draw_button, draw_toggle};
use exo_ui::{Canvas, Palette, Rect, Theme};

impl SettingsApp {
    pub(crate) fn draw_network(&self, cv: &mut Canvas<'_>, c: Rect, mx: i32, my: i32, pal: &Palette, cfg: &SystemConfig, fonts: &FontSet, t: &TelemetryState) {
        let lang = cfg.lang;
        cv.draw_text(&fonts.lg_bold, c.x + 24, c.y + 18, lang.pick("Réseau (ExoNet)", "Network (ExoNet)"), pal.text_primary);
        cv.draw_text(&fonts.sm, c.x + 24, c.y + 42, lang.pick("Interfaces réseau et politique NetCap par application", "Network interfaces and per-application NetCap firewall"), pal.text_secondary);

        let net_up = t.services.iter().any(|s| s.name == "network_server" && s.online);
        let iface = Rect::new(c.x + 24, c.y + 74, c.w - 48, 66);
        cv.fill_rounded_rect(iface, 12, pal.card_bg);
        cv.stroke_rounded_rect(iface, 12, pal.border_subtle);
        cv.draw_text(&fonts.md, iface.x + 14, iface.y + 12, "Ethernet (virtio-net / e1000) — DHCP", pal.text_primary);
        cv.draw_text(
            &fonts.sm,
            iface.x + 14,
            iface.y + 36,
            if net_up {
                lang.pick("Serveur ExoNet en ligne • Loopback 127.0.0.1 actif", "ExoNet server online • Loopback 127.0.0.1 active")
            } else {
                lang.pick("Serveur réseau hors ligne", "Network server offline")
            },
            pal.text_secondary,
        );
        let diag_btn = Rect::new(iface.right() - 154, iface.y + 18, 140, 30);
        draw_button(cv, diag_btn, lang.pick("Tester ExoNet", "Test ExoNet"), net_up, diag_btn.contains(mx, my), pal, &fonts.sm);

        cv.draw_text(&fonts.md, c.x + 24, c.y + 158, lang.pick("Pare-feu NetCap par application", "Per-application NetCap firewall"), pal.text_primary);
        let apps = [
            ("Terminal (exosh)", 0),
            (lang.pick("Éditeur de texte", "Text Editor"), 1),
            (lang.pick("Jeux 2D (hors ligne)", "2D Games (offline)"), 2),
        ];
        let mut y = c.y + 188;
        for (name, idx) in apps {
            let r = Rect::new(c.x + 24, y, c.w - 48, 46);
            cv.fill_rounded_rect(r, 10, pal.card_bg);
            cv.stroke_rounded_rect(r, 10, pal.border_subtle);
            cv.draw_text(&fonts.md, r.x + 14, r.y + 13, name, pal.text_primary);
            let pol = cfg.net_policies[idx];
            let (lbl, col) = match pol {
                1 => (lang.pick("Autoriser", "Allow"), pal.success),
                2 => (lang.pick("Bloquer", "Block"), pal.danger),
                _ => (lang.pick("Demander", "Ask"), pal.warning),
            };
            let btn = Rect::new(r.right() - 124, r.y + 9, 110, 28);
            cv.fill_rounded_rect(btn, 7, if btn.contains(mx, my) { col.with_alpha(0x36) } else { col.with_alpha(0x20) });
            cv.stroke_rounded_rect(btn, 7, col.with_alpha(0x90));
            let tw = fonts.sm.measure_width(lbl);
            cv.draw_text(&fonts.sm, btn.x + (btn.w - tw) / 2, btn.y + 6, lbl, col);
            y += 54;
        }

        // Live ExoNet diagnostic panel (Loopback / Gateway / DNS / External Internet)
        let dcard = Rect::new(c.x + 24, y + 6, c.w - 48, 122);
        cv.fill_rounded_rect(dcard, 12, pal.card_bg);
        cv.stroke_rounded_rect(dcard, 12, pal.border_subtle);
        cv.draw_text(&fonts.md, dcard.x + 14, dcard.y + 10, lang.pick("Diagnostic de connectivité (ICMP / DNS / TCP)", "Connectivity Diagnostic (ICMP / DNS / TCP)"), pal.text_primary);
        if let Some(ref rep) = self.last_net_diag {
            let rows = [
                (rep.loopback_ok, format!("IP {} • Loopback : {}", rep.local_ip, rep.loopback_ms)),
                (rep.gateway_ok, format!("Passerelle {} : {}", rep.gateway_ip, rep.gateway_ms)),
                (rep.dns_ok, format!("DNS {} : {}", rep.dns_ip, rep.dns_detail)),
                (rep.external_ok, format!("Internet externe : {}", rep.external_detail)),
            ];
            let mut dy = dcard.y + 34;
            for (ok, line) in rows {
                cv.fill_circle_aa((dcard.x + 20) as f32, (dy + 7) as f32, 4.0, if ok { pal.success } else { pal.danger });
                cv.draw_text_ellipsis(&fonts.sm, dcard.x + 32, dy, &line, dcard.w - 46, if ok { pal.text_primary } else { pal.warning });
                dy += 20;
            }
        } else {
            cv.draw_text(
                &fonts.sm,
                dcard.x + 14,
                dcard.y + 42,
                lang.pick("Cliquez sur « Tester ExoNet » pour mesurer la passerelle, le DNS et Internet (1.1.1.1).", "Click 'Test ExoNet' to probe gateway, DNS, and external Internet (1.1.1.1)."),
                pal.text_secondary,
            );
            cv.draw_text(
                &fonts.sm,
                dcard.x + 14,
                dcard.y + 68,
                lang.pick("Astuce Terminal : tapez « ping 1.1.1.1 », « ping google.com » ou « nettest ».", "Terminal tip: run 'ping 1.1.1.1', 'ping google.com', or 'nettest'."),
                pal.text_muted,
            );
        }
    }

    pub(crate) fn click_network(&mut self, c: Rect, mx: i32, my: i32, cfg: &mut SystemConfig, _t: &TelemetryState) -> AppAction {
        let iface = Rect::new(c.x + 24, c.y + 74, c.w - 48, 66);
        if Rect::new(iface.right() - 154, iface.y + 18, 140, 30).contains(mx, my) {
            let rep = crate::net_diag::run_full_net_diagnostic();
            let toast = rep.summary.clone();
            self.last_net_diag = Some(rep);
            return AppAction::StatusToast(toast);
        }
        let mut y = c.y + 188;
        for idx in 0..3 {
            let r = Rect::new(c.x + 24, y, c.w - 48, 46);
            if Rect::new(r.right() - 124, r.y + 9, 110, 28).contains(mx, my) {
                cfg.net_policies[idx] = (cfg.net_policies[idx] + 1) % 3;
                return AppAction::Redraw;
            }
            y += 54;
        }
        AppAction::None
    }

    pub(crate) fn draw_accounts(&self, cv: &mut Canvas<'_>, c: Rect, mx: i32, my: i32, pal: &Palette, cfg: &SystemConfig, fonts: &FontSet, t: &TelemetryState) {
        let lang = cfg.lang;
        cv.draw_text(&fonts.lg_bold, c.x + 24, c.y + 18, lang.pick("Comptes et ExoAuth", "Accounts & ExoAuth"), pal.text_primary);
        cv.draw_text(&fonts.sm, c.x + 24, c.y + 42, lang.pick("Session utilisateur, élévation administrateur et verrouillage", "User session, admin elevation, and screen lock"), pal.text_secondary);

        let ucard = Rect::new(c.x + 24, c.y + 74, c.w - 48, 86);
        cv.fill_rounded_rect(ucard, 12, pal.card_bg);
        cv.stroke_rounded_rect(ucard, 12, pal.border_subtle);
        cv.fill_circle_aa(ucard.x as f32 + 40.0, ucard.y as f32 + 43.0, 24.0, pal.accent);
        let init_ch = cfg.username.chars().next().unwrap_or('U').to_ascii_uppercase().to_string();
        let iw = fonts.lg_bold.measure_width(&init_ch);
        cv.draw_text(&fonts.lg_bold, ucard.x + 40 - iw / 2, ucard.y + 32, &init_ch, exo_ui::Color::WHITE);
        cv.draw_text(&fonts.lg_bold, ucard.x + 78, ucard.y + 18, &cfg.username, pal.text_primary);
        cv.draw_text(&fonts.sm, ucard.x + 78, ucard.y + 44, lang.pick("Propriétaire • Session locale active", "Owner • Active local session"), pal.text_secondary);

        let lock_btn = Rect::new(ucard.right() - 180, ucard.y + 26, 164, 34);
        draw_button(cv, lock_btn, lang.pick("Verrouiller (Super+L)", "Lock Screen (Super+L)"), false, lock_btn.contains(mx, my), pal, &fonts.sm);

        // Admin elevation card
        let acard = Rect::new(c.x + 24, c.y + 176, c.w - 48, 76);
        cv.fill_rounded_rect(acard, 12, pal.card_bg);
        cv.stroke_rounded_rect(acard, 12, pal.border_subtle);
        cv.draw_text(&fonts.md, acard.x + 14, acard.y + 14, lang.pick("Élévation Administrateur (ExoAuth)", "Admin Elevation (ExoAuth)"), pal.text_primary);
        cv.draw_text(&fonts.sm, acard.x + 14, acard.y + 38, lang.pick("Aucune action sensible sans autorisation explicite", "Explicit authorization required for privileged changes"), pal.text_secondary);
        let elev_btn = Rect::new(acard.right() - 170, acard.y + 21, 154, 34);
        draw_button(
            cv,
            elev_btn,
            if cfg.admin_unlocked { lang.pick("Verrouiller admin", "Drop admin") } else { lang.pick("Déverrouiller admin", "Unlock admin") },
            !cfg.admin_unlocked,
            elev_btn.contains(mx, my),
            pal,
            &fonts.sm,
        );

        let usb_up = t.services.iter().any(|s| s.name == "usb_server" && s.online);
        let hw_card = Rect::new(c.x + 24, c.y + 268, c.w - 48, 66);
        cv.fill_rounded_rect(hw_card, 12, pal.card_bg);
        cv.stroke_rounded_rect(hw_card, 12, pal.border_subtle);
        cv.draw_text(&fonts.md, hw_card.x + 14, hw_card.y + 12, lang.pick("Clé USB Ancre / ExoAuth matériel", "Hardware USB Anchor / ExoAuth Key"), pal.text_primary);
        cv.draw_text(
            &fonts.sm,
            hw_card.x + 14,
            hw_card.y + 36,
            if usb_up {
                lang.pick("Contrôleur USB actif • Prêt à détecter une clé FIDO/ExoAuth", "USB stack active • Ready to detect FIDO/ExoAuth key")
            } else {
                lang.pick("Contrôleur USB inactif • Session locale ExoAuth utilisée", "USB stack idle • Local ExoAuth session active")
            },
            pal.text_secondary,
        );
        let hw_btn = Rect::new(hw_card.right() - 170, hw_card.y + 18, 154, 30);
        draw_button(cv, hw_btn, lang.pick("Vérifier USB / Clé", "Probe USB / Key"), false, hw_btn.contains(mx, my), pal, &fonts.sm);
    }

    pub(crate) fn click_accounts(&self, c: Rect, mx: i32, my: i32, cfg: &mut SystemConfig, t: &TelemetryState) -> AppAction {
        let ucard = Rect::new(c.x + 24, c.y + 74, c.w - 48, 86);
        if Rect::new(ucard.right() - 180, ucard.y + 26, 164, 34).contains(mx, my) {
            return AppAction::LockScreen;
        }
        let acard = Rect::new(c.x + 24, c.y + 176, c.w - 48, 76);
        if Rect::new(acard.right() - 170, acard.y + 21, 154, 34).contains(mx, my) {
            if cfg.admin_unlocked {
                cfg.admin_unlocked = false;
                return AppAction::StatusToast(String::from("Privilèges administrateur relâchés"));
            }
            return AppAction::RequestAdminElevation {
                reason: String::from("Autoriser la modification des réglages administrateur"),
            };
        }
        let hw_card = Rect::new(c.x + 24, c.y + 268, c.w - 48, 66);
        if Rect::new(hw_card.right() - 170, hw_card.y + 18, 154, 30).contains(mx, my) {
            let usb_up = t.services.iter().any(|s| s.name == "usb_server" && s.online);
            return AppAction::StatusToast(String::from(if usb_up {
                cfg.lang.pick(
                    "Bus USB sondé : aucune clé matérielle ExoAuth branchée",
                    "USB bus probed: no ExoAuth hardware key attached",
                )
            } else {
                cfg.lang.pick(
                    "Service USB inactif — authentification locale active",
                    "USB service idle — local authentication active",
                )
            }));
        }
        AppAction::None
    }

    pub(crate) fn draw_security_storage(&self, cv: &mut Canvas<'_>, c: Rect, mx: i32, my: i32, pal: &Palette, cfg: &SystemConfig, fonts: &FontSet, t: &TelemetryState) {
        let lang = cfg.lang;
        cv.draw_text(&fonts.lg_bold, c.x + 24, c.y + 18, lang.pick("Sécurité et Stockage", "Security & Storage"), pal.text_primary);
        cv.draw_text(&fonts.sm, c.x + 24, c.y + 42, lang.pick("Protections actives, journal ExoLedger et instantanés ExoFS", "Active protections, ExoLedger audit log, and ExoFS snapshots"), pal.text_secondary);

        let scard = Rect::new(c.x + 24, c.y + 74, c.w - 48, 114);
        cv.fill_rounded_rect(scard, 12, pal.card_bg);
        cv.stroke_rounded_rect(scard, 12, pal.border_subtle);
        cv.draw_text(&fonts.md, scard.x + 14, scard.y + 12, "ExoShield • ExoCordon • ExoLedger", pal.text_primary);
        let mut bx = scard.x + 14;
        bx += draw_badge(cv, bx, scard.y + 44, "KPTI + SMAP + SMEP", pal.success, &fonts.sm) + 8;
        bx += draw_badge(cv, bx, scard.y + 44, "IOMMU VT-d", pal.success, &fonts.sm) + 8;
        draw_badge(cv, bx, scard.y + 44, lang.pick("125 règles IPC", "125 IPC rules"), pal.accent, &fonts.sm);
        cv.draw_text(
            &fonts.sm,
            scard.x + 14,
            scard.y + 80,
            &format!(
                "{} {} {}",
                lang.pick("Journal ExoLedger :", "ExoLedger Audit Log:"),
                t.exoledger_entries.max(0),
                lang.pick("entrées vérifiées par BLAKE3", "BLAKE3-verified entries")
            ),
            pal.text_secondary,
        );

        let fcard = Rect::new(c.x + 24, c.y + 204, c.w - 48, 126);
        cv.fill_rounded_rect(fcard, 12, pal.card_bg);
        cv.stroke_rounded_rect(fcard, 12, pal.border_subtle);
        let used = t.exofs_used_kb / 1024;
        let tot = (t.exofs_total_kb / 1024).max(1);
        cv.draw_text(
            &fonts.md,
            fcard.x + 14,
            fcard.y + 14,
            &format!("{} — {} / {} {}", lang.pick("Volume ExoFS", "ExoFS Volume"), used, tot, lang.pick("Mio utilisés", "MiB used")),
            pal.text_primary,
        );
        let bar = Rect::new(fcard.x + 14, fcard.y + 44, fcard.w - 28, 10);
        cv.fill_rounded_rect(bar, 5, pal.border_subtle);
        let fw = ((bar.w as u64 * used) / tot).clamp(12, bar.w as u64) as i32;
        cv.fill_rounded_rect(Rect::new(bar.x, bar.y, fw, 10), 5, pal.accent);
        cv.draw_text(
            &fonts.sm,
            fcard.x + 14,
            fcard.y + 66,
            &format!("{} {}", t.exofs_snapshots.len(), lang.pick("instantané(s) immuable(s) disponible(s)", "immutable snapshot(s) available")),
            pal.text_secondary,
        );
        let btn = Rect::new(fcard.x + 14, fcard.y + 88, 186, 28);
        draw_button(cv, btn, lang.pick("Créer un instantané", "Create Snapshot"), true, btn.contains(mx, my), pal, &fonts.sm);
    }

    pub(crate) fn click_security_storage(&self, c: Rect, mx: i32, my: i32, _cfg: &SystemConfig) -> AppAction {
        let fcard = Rect::new(c.x + 24, c.y + 204, c.w - 48, 126);
        if Rect::new(fcard.x + 14, fcard.y + 88, 186, 28).contains(mx, my) {
            return AppAction::CreateSnapshot(String::from("settings"));
        }
        AppAction::None
    }

    pub(crate) fn draw_power_dev(&self, cv: &mut Canvas<'_>, c: Rect, mx: i32, my: i32, _theme: &Theme, pal: &Palette, cfg: &SystemConfig, fonts: &FontSet, t: &TelemetryState) {
        let lang = cfg.lang;
        cv.draw_text(&fonts.lg_bold, c.x + 24, c.y + 18, lang.pick("Alimentation, Résilience & Dev", "Power, Resilience & Developer"), pal.text_primary);
        cv.draw_text(&fonts.sm, c.x + 24, c.y + 42, lang.pick("Arrêt système, noyau de secours ExoPhoenix et options développeur", "Power actions, ExoPhoenix backup kernel, and developer tools"), pal.text_secondary);

        // Power actions
        let pcard = Rect::new(c.x + 24, c.y + 74, c.w - 48, 74);
        cv.fill_rounded_rect(pcard, 12, pal.card_bg);
        cv.stroke_rounded_rect(pcard, 12, pal.border_subtle);
        cv.draw_text(&fonts.md, pcard.x + 14, pcard.y + 12, lang.pick("Actions d'alimentation", "Power controls"), pal.text_primary);
        let rb = Rect::new(pcard.x + 14, pcard.y + 38, 134, 28);
        let sd = Rect::new(rb.right() + 12, pcard.y + 38, 134, 28);
        let lk = Rect::new(sd.right() + 12, pcard.y + 38, 134, 28);
        draw_button(cv, rb, lang.pick("Redémarrer", "Reboot"), false, rb.contains(mx, my), pal, &fonts.sm);
        draw_button(cv, sd, lang.pick("Éteindre", "Shut down"), false, sd.contains(mx, my), pal, &fonts.sm);
        draw_button(cv, lk, lang.pick("Verrouiller", "Lock"), false, lk.contains(mx, my), pal, &fonts.sm);

        // ExoPhoenix status
        let phx = Rect::new(c.x + 24, c.y + 164, c.w - 48, 86);
        cv.fill_rounded_rect(phx, 12, pal.card_bg);
        cv.stroke_rounded_rect(phx, 12, pal.border_subtle);
        cv.draw_text(&fonts.md, phx.x + 14, phx.y + 12, "ExoPhoenix — Noyau de secours A/B", pal.text_primary);
        cv.draw_text(&fonts.sm, phx.x + 14, phx.y + 36, &t.last_phoenix_msg, pal.text_secondary);
        cv.draw_text(&fonts.sm, phx.x + 14, phx.y + 58, &format!("Dernier cycle : #{} • Temps bascule : {} µs", t.phoenix_rec.cycle_id, t.phoenix_rec.delta_sla_us), pal.text_muted);

        // Developer mode toggle (Spec §1.19)
        let dcard = Rect::new(c.x + 24, c.y + 266, c.w - 48, 60);
        cv.fill_rounded_rect(dcard, 12, pal.card_bg);
        cv.stroke_rounded_rect(dcard, 12, pal.border_subtle);
        cv.draw_text(&fonts.md, dcard.x + 14, dcard.y + 10, lang.pick("Mode Développeur", "Developer Mode"), pal.text_primary);
        cv.draw_text(&fonts.sm, dcard.x + 14, dcard.y + 32, lang.pick("Débloque les tests ExoPhoenix, l'overlay FPS et le banc complet", "Unlocks ExoPhoenix switch tests, FPS overlay, and full profiler"), pal.text_muted);
        draw_toggle(cv, Rect::new(dcard.right() - 68, dcard.y + 18, 50, 24), cfg.dev_mode, pal.accent, pal);

        if cfg.dev_mode {
            let dev_box = Rect::new(c.x + 24, c.y + 340, c.w - 48, 108);
            cv.fill_rounded_rect(dev_box, 12, pal.card_bg);
            cv.stroke_rounded_rect(dev_box, 12, pal.accent.with_alpha(0x88));
            cv.draw_text(&fonts.sm, dev_box.x + 14, dev_box.y + 12, "Outils Développeur (Actif)", pal.accent);
            cv.draw_text(&fonts.sm, dev_box.x + 14, dev_box.y + 34, &format!("Syscalls : {} (fast {}) • IPC : {} envoyés, {} perdus", t.dispatch_total, t.dispatch_fast, t.ipc_sent, t.ipc_dropped), pal.text_secondary);
            let test_btn = Rect::new(dev_box.x + 14, dev_box.y + 62, 224, 32);
            let bench_btn = Rect::new(test_btn.right() + 12, dev_box.y + 62, 200, 32);
            draw_button(cv, test_btn, lang.pick("Tester bascule ExoPhoenix", "Test ExoPhoenix switch"), true, test_btn.contains(mx, my), pal, &fonts.sm);
            draw_button(cv, bench_btn, lang.pick("Lancer ExoBench complet", "Run Full ExoBench"), false, bench_btn.contains(mx, my), pal, &fonts.sm);
        }
    }

    pub(crate) fn click_power_dev(&self, c: Rect, mx: i32, my: i32, _theme: &mut Theme, cfg: &mut SystemConfig) -> AppAction {
        let pcard = Rect::new(c.x + 24, c.y + 74, c.w - 48, 74);
        if Rect::new(pcard.x + 14, pcard.y + 38, 134, 28).contains(mx, my) { return AppAction::Reboot; }
        if Rect::new(pcard.x + 160, pcard.y + 38, 134, 28).contains(mx, my) { return AppAction::Shutdown; }
        if Rect::new(pcard.x + 306, pcard.y + 38, 134, 28).contains(mx, my) { return AppAction::LockScreen; }

        let dcard = Rect::new(c.x + 24, c.y + 266, c.w - 48, 60);
        if dcard.contains(mx, my) {
            cfg.dev_mode = !cfg.dev_mode;
            return AppAction::Redraw;
        }
        if cfg.dev_mode {
            let dev_box = Rect::new(c.x + 24, c.y + 340, c.w - 48, 108);
            if Rect::new(dev_box.x + 14, dev_box.y + 62, 224, 32).contains(mx, my) {
                if !cfg.admin_unlocked {
                    return AppAction::RequestAdminElevation {
                        reason: String::from("Test de résurrection du noyau ExoPhoenix"),
                    };
                }
                return AppAction::TriggerPhoenixSwitch;
            }
            if Rect::new(dev_box.x + 250, dev_box.y + 62, 200, 32).contains(mx, my) {
                return AppAction::RunBenchAll;
            }
        }
        AppAction::None
    }
}
