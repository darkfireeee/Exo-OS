//! Built-in shell command execution + `exo bench` / `exo doctor` / `/bin/*` dispatch.

use crate::apps::files::USER_HOME;
use crate::apps::terminal::TerminalApp;
use crate::apps::AppAction;
use crate::assets::FontSet;
use crate::bench::BenchSuite;
use crate::i18n::Lang;
use crate::net_diag;
use crate::telemetry::TelemetryState;
use exo_ui::Color;

const FG_MAIN: Color = Color::rgb(0xF3, 0xF4, 0xF6);
const FG_DIM: Color = Color::rgb(0x9C, 0xA3, 0xAF);
const FG_OK: Color = Color::rgb(0x10, 0xB9, 0x81);
const FG_WARN: Color = Color::rgb(0xF5, 0x9E, 0x0B);
const FG_ERR: Color = Color::rgb(0xF4, 0x3F, 0x5E);
const FG_ACC: Color = Color::rgb(0x38, 0xBD, 0xF8);

fn display_cwd(cwd: &str) -> String {
    if cwd == USER_HOME {
        String::from("~")
    } else if let Some(rest) = cwd.strip_prefix("/home/user/") {
        format!("~/{}", rest)
    } else {
        String::from(cwd)
    }
}

impl TerminalApp {
    pub(crate) fn execute_command(
        &mut self,
        raw: &str,
        telemetry: &mut TelemetryState,
        bench: &mut BenchSuite,
        fonts: &FontSet,
    ) -> AppAction {
        let cmd = raw.trim();
        let tab = self.cur_mut();
        let prompt = format!("user@exo:{} $ {}", display_cwd(&tab.cwd), cmd);
        tab.push_line(prompt, FG_ACC);
        if cmd.is_empty() { return AppAction::Redraw; }
        tab.history.push(String::from(cmd));
        tab.history_idx = None;

        let parts: Vec<&str> = cmd.split_whitespace().collect();
        let head = parts[0];
        let args = &parts[1..];

        match head {
            "help" => {
                tab.push_line("Available commands:", FG_MAIN);
                tab.push_line("  ls [dir]      List ExoFS entries      cd [dir|~]   Change directory", FG_DIM);
                tab.push_line("  pwd / cat     Path / read file        mkdir/touch  Create dir / file", FG_DIM);
                tab.push_line("  rm <name>     Remove entry            ps / meminfo Processes & RAM", FG_DIM);
                tab.push_line("  ping <host>   External ICMP/TCP ping  dns <domain> UDP DNS lookup", FG_DIM);
                tab.push_line("  nettest / ip  ExoNet diagnostics      clear        Clear screen", FG_DIM);
                tab.push_line("  exo bench     Full performance bench  exo doctor   System health check", FG_DIM);
            }
            "clear" => {
                tab.lines.clear();
                tab.sel_anchor = None;
                tab.sel_cursor = None;
            }
            "pwd" => { let c = tab.cwd.clone(); tab.push_line(c, FG_MAIN); }
            "date" => tab.push_line(format!("{} {}", telemetry.clock.format_date(Lang::En), telemetry.clock.format_time_sec()), FG_MAIN),
            "uname" => tab.push_line("ExoOS 0.2.0 x86_64 Microkernel (Rust)", FG_MAIN),
            "echo" => tab.push_line(args.join(" "), FG_MAIN),
            "cd" => {
                let raw_arg = args.first().copied().unwrap_or("~");
                let target = if raw_arg == "~" {
                    String::from(USER_HOME)
                } else if let Some(rest) = raw_arg.strip_prefix("~/") {
                    format!("{}/{}", USER_HOME, rest)
                } else {
                    resolve_rel(&tab.cwd, raw_arg)
                };
                if std::fs::read_dir(&target).is_ok() { tab.cwd = target; }
                else { tab.push_line(format!("cd: introuvable : {}", target), FG_ERR); }
            }
            "ping" => {
                let target = args.first().copied().unwrap_or("1.1.1.1");
                let count = args.get(1).and_then(|s| s.parse::<usize>().ok()).unwrap_or(4);
                for (line, kind) in net_diag::run_ping_command(target, count) {
                    let col = match kind {
                        1 => FG_OK,
                        2 => FG_WARN,
                        3 => FG_ERR,
                        4 => FG_ACC,
                        _ => FG_MAIN,
                    };
                    tab.push_line(line, col);
                }
            }
            "dns" | "nslookup" => {
                let host = args.first().copied().unwrap_or("cloudflare.com");
                match net_diag::resolve_dns_a(host) {
                    Ok((ip, srv, us)) => {
                        tab.push_line(
                            format!("Serveur DNS : {} | {} -> {} ({})", net_diag::fmt_ipv4(srv), host, net_diag::fmt_ipv4(ip), net_diag::fmt_ms(us)),
                            FG_OK,
                        );
                    }
                    Err(e) => tab.push_line(e, FG_ERR),
                }
            }
            "ip" | "ifconfig" => {
                let dns = net_diag::fmt_ipv4(net_diag::read_dns_server());
                tab.push_line("lo0    : 127.0.0.1/8    (UP, LOOPBACK)", FG_OK);
                tab.push_line("eth0   : 10.0.2.15/24   (virtio-net / e1000, DHCP)", FG_ACC);
                tab.push_line(format!("Route  : défaut via 10.0.2.2 | DNS : {}", dns), FG_MAIN);
            }
            "nettest" => {
                tab.push_line("Running full ExoNet connectivity diagnostic...", FG_ACC);
                let r = net_diag::run_full_net_diagnostic();
                tab.push_line(
                    format!("  [{}] Loopback 127.0.0.1 : {}", if r.loopback_ok { "OK" } else { "FAIL" }, r.loopback_ms),
                    if r.loopback_ok { FG_OK } else { FG_ERR },
                );
                tab.push_line(
                    format!("  [{}] Gateway 10.0.2.2   : {}", if r.gateway_ok { "OK" } else { "FAIL" }, r.gateway_ms),
                    if r.gateway_ok { FG_OK } else { FG_ERR },
                );
                tab.push_line(
                    format!("  [{}] DNS Resolution     : {}", if r.dns_ok { "OK" } else { "FAIL" }, r.dns_detail),
                    if r.dns_ok { FG_OK } else { FG_WARN },
                );
                tab.push_line(
                    format!("  [{}] External Internet  : {}", if r.external_ok { "OK" } else { "FAIL" }, r.external_detail),
                    if r.external_ok { FG_OK } else { FG_ERR },
                );
            }
            "ls" => {
                let dir = args.first().map(|a| resolve_rel(&tab.cwd, a)).unwrap_or_else(|| tab.cwd.clone());
                match std::fs::read_dir(&dir) {
                    Ok(rd) => {
                        let mut names: Vec<String> = rd.flatten().map(|e| {
                            let n = e.file_name().to_string_lossy().into_owned();
                            if e.metadata().map(|m| m.is_dir()).unwrap_or(false) { format!("{}/", n) } else { n }
                        }).collect();
                        names.sort();
                        for ch in names.chunks(4) {
                            tab.push_line(ch.iter().map(|s| format!("{:<20}", s)).collect::<String>(), FG_MAIN);
                        }
                    }
                    Err(e) => tab.push_line(format!("ls: {}", e), FG_ERR),
                }
            }
            "cat" => {
                if let Some(&f) = args.first() {
                    let p = resolve_rel(&tab.cwd, f);
                    match std::fs::read(&p) {
                        Ok(b) => for l in String::from_utf8_lossy(&b[..b.len().min(4096)]).lines().take(60) {
                            tab.push_line(l, FG_MAIN);
                        },
                        Err(e) => tab.push_line(format!("cat: {}", e), FG_ERR),
                    }
                } else { tab.push_line("Usage: cat <fichier>", FG_WARN); }
            }
            "mkdir" => if let Some(&f) = args.first() {
                let p = resolve_rel(&tab.cwd, f);
                match std::fs::create_dir(&p) {
                    Ok(()) => tab.push_line(format!("Créé : {}", p), FG_OK),
                    Err(e) => tab.push_line(format!("mkdir: {}", e), FG_ERR),
                }
            },
            "touch" => if let Some(&f) = args.first() {
                let p = resolve_rel(&tab.cwd, f);
                match std::fs::write(&p, b"") {
                    Ok(()) => tab.push_line(format!("Créé : {}", p), FG_OK),
                    Err(e) => tab.push_line(format!("touch: {}", e), FG_ERR),
                }
            },
            "rm" => if let Some(&f) = args.first() {
                let p = resolve_rel(&tab.cwd, f);
                if std::fs::remove_file(&p).or_else(|_| std::fs::remove_dir(&p)).is_ok() {
                    tab.push_line(format!("Supprimé : {}", p), FG_OK);
                } else {
                    tab.push_line(format!("rm: échec {}", p), FG_ERR);
                }
            },
            "ps" => {
                tab.push_line("  PID  RAM(K)    CPU(ms)  NOM", FG_DIM);
                for p in &telemetry.processes {
                    tab.push_line(format!("{:>5}  {:>6}  {:>8}  {}", p.pid, p.memory_kb, p.cpu_ticks, p.name), FG_MAIN);
                }
            }
            "meminfo" => {
                tab.push_line(format!("RAM: {} / {} Mio ({} %) | {} processus", telemetry.used_ram_mb, telemetry.total_ram_mb, telemetry.mem_used_pct, telemetry.processes.len()), FG_OK);
            }
            "exo" => match args.first().copied().unwrap_or("doctor") {
                "bench" => {
                    bench.run_all(fonts);
                    let rep = bench.format_final_report("exo-workstation");
                    for l in rep.lines() { tab.push_line(l, if l.starts_with("---") { FG_ACC } else if l.starts_with("Score") { FG_OK } else { FG_MAIN }); }
                    let _ = bench.export_report("exo-workstation");
                    tab.push_line("Rapport exporté -> /tmp/exo_perf_report.txt", FG_OK);
                }
                "doctor" => {
                    let (up, tot) = telemetry.critical_counts();
                    tab.push_line(format!("[OK] Health: {}/100 | Services: {}/{} | ExoLedger: {} entries", telemetry.overall_health_score(), up, tot, telemetry.exoledger_entries.max(0)), FG_OK);
                }
                "metrics" => {
                    tab.push_line(format!("Syscalls total={} fast={} slow={} | IPC sent={} drop={}", telemetry.dispatch_total, telemetry.dispatch_fast, telemetry.dispatch_slow, telemetry.ipc_sent, telemetry.ipc_dropped), FG_MAIN);
                }
                _ => tab.push_line("Usage: exo [bench|doctor|metrics]", FG_WARN),
            },
            other => {
                let bin = if other.starts_with('/') { String::from(other) } else { format!("/bin/{}", other) };
                match std::process::Command::new(&bin).args(args).output() {
                    Ok(out) => {
                        for l in String::from_utf8_lossy(&out.stdout).lines().take(50) { tab.push_line(l, FG_MAIN); }
                        for l in String::from_utf8_lossy(&out.stderr).lines().take(16) { tab.push_line(l, FG_ERR); }
                    }
                    Err(_) => tab.push_line(format!("Commande inconnue : '{}'", other), FG_ERR),
                }
            }
        }
        AppAction::Redraw
    }
}

fn resolve_rel(cwd: &str, arg: &str) -> String {
    if arg == ".." {
        let tr = cwd.trim_end_matches('/');
        match tr.rfind('/') {
            Some(0) | None => String::from("/"),
            Some(pos) => String::from(&tr[..pos]),
        }
    } else if arg.starts_with('/') {
        String::from(arg)
    } else if cwd == "/" {
        format!("/{}", arg)
    } else {
        format!("{}/{}", cwd.trim_end_matches('/'), arg)
    }
}
