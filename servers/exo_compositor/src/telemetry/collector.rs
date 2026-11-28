//! Live Ring 0/1 syscall telemetry collector & process management (`SYS_KILL`).

use crate::telemetry::clock::{unix_to_rtc, RtcClock};

#[cfg(target_os = "exo")]
use exo_syscall_abi as syscall;

pub const HISTORY_LEN: usize = 40;

#[repr(C, align(64))]
#[derive(Clone, Copy, Default)]
#[allow(dead_code)]
pub struct PhoenixTimingRecord {
    pub magic: u32,
    pub cycle_id: u32,
    pub trigger_reason: u32,
    pub flags: u32,
    pub tsc_khz: u64,
    pub t0_freeze_req: u64,
    pub t1_freeze_ack_all: u64,
    pub t2_iommu_and_forge: u64,
    pub t3_kernel_b_active: u64,
    pub t4_crypto_reseeded: u64,
    pub t5_strate1_ready: u64,
    pub t6_strate2_ready: u64,
    pub delta_s0_freeze_us: u64,
    pub delta_s0_forge_us: u64,
    pub delta_s0_us: u64,
    pub delta_crypto_us: u64,
    pub delta_sla_us: u64,
    pub delta_full_us: u64,
    pub crypto_epoch_id: u64,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct ProcessEntry {
    pub pid: u32,
    pub ppid: u32,
    pub state: u32,
    pub cpu_ticks: u64,
    pub memory_kb: u64,
    pub name: String,
    pub critical: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct ServiceProbe {
    pub name: &'static str,
    pub critical: bool,
    pub online: bool,
}

#[allow(dead_code)]
pub struct TelemetryState {
    pub clock: RtcClock,
    pub tz_idx: usize,
    pub sample_tick: u64,
    pub dispatch_total: u64,
    pub dispatch_fast: u64,
    pub dispatch_slow: u64,
    pub dispatch_enosys: u64,
    pub ipc_sent: u64,
    pub ipc_recv: u64,
    pub ipc_dropped: u64,
    pub ipc_rpc: u64,
    pub ipc_timeouts: u64,
    pub futex_waits: u64,
    pub futex_wakes: u64,
    prev_dispatch: u64,
    prev_ipc: u64,
    pub syscall_rate_hist: Vec<u32>,
    pub ipc_rate_hist: Vec<u32>,
    pub cpu_load_hist: Vec<u32>,
    pub mem_pct_hist: Vec<u32>,
    pub total_ram_mb: u64,
    pub used_ram_mb: u64,
    pub cpu_cores: u32,
    pub cpu_load_pct: u32,
    pub mem_used_pct: u32,
    pub phoenix_state: i64,
    pub phoenix_rec: PhoenixTimingRecord,
    pub exoledger_entries: i64,
    pub last_phoenix_msg: String,
    pub exofs_used_kb: u64,
    pub exofs_total_kb: u64,
    pub exofs_snapshots: Vec<String>,
    pub services: Vec<ServiceProbe>,
    pub processes: Vec<ProcessEntry>,
}

const MONITORED_SERVICES: [(&str, bool); 13] = [
    ("ipc_router", true),
    ("memory_server", true),
    ("vfs_server", true),
    ("crypto_server", true),
    ("device_server", true),
    ("fb_server", true),
    ("input_server", true),
    ("tty_server", true),
    ("exo_shield", true),
    ("network_server", false),
    ("audio_server", false),
    ("usb_server", false),
    ("exo_compositor", false),
];

impl TelemetryState {
    pub fn new() -> Self {
        let mut s = Self {
            clock: RtcClock {
                year: 2026,
                month: 11,
                day: 27,
                weekday: 5,
                hour: 8,
                minute: 0,
                second: 0,
                uptime_sec: 0,
            },
            tz_idx: 0,
            sample_tick: 0,
            dispatch_total: 0,
            dispatch_fast: 0,
            dispatch_slow: 0,
            dispatch_enosys: 0,
            ipc_sent: 0,
            ipc_recv: 0,
            ipc_dropped: 0,
            ipc_rpc: 0,
            ipc_timeouts: 0,
            futex_waits: 0,
            futex_wakes: 0,
            prev_dispatch: 0,
            prev_ipc: 0,
            syscall_rate_hist: vec![12; HISTORY_LEN],
            ipc_rate_hist: vec![8; HISTORY_LEN],
            cpu_load_hist: vec![14; HISTORY_LEN],
            mem_pct_hist: vec![12; HISTORY_LEN],
            total_ram_mb: 4096,
            used_ram_mb: 184,
            cpu_cores: 4,
            cpu_load_pct: 12,
            mem_used_pct: 5,
            phoenix_state: 1,
            phoenix_rec: PhoenixTimingRecord::default(),
            exoledger_entries: 0,
            last_phoenix_msg: String::from("Noyau de secours prêt — image vérifiée"),
            exofs_used_kb: 22_400,
            exofs_total_kb: 524_288,
            exofs_snapshots: vec![String::from("boot-root • vérifié BLAKE3")],
            services: MONITORED_SERVICES
                .iter()
                .map(|&(name, critical)| ServiceProbe { name, critical, online: false })
                .collect(),
            processes: Vec::new(),
        };
        s.poll();
        s
    }

    pub fn critical_counts(&self) -> (u32, u32) {
        let tot = self.services.iter().filter(|s| s.critical).count() as u32;
        let up = self.services.iter().filter(|s| s.critical && s.online).count() as u32;
        (up, tot)
    }

    pub fn overall_health_score(&self) -> u32 {
        let mut score = 0u32;
        if self.dispatch_total > 0 { score += 20; }
        if self.phoenix_state >= 1 { score += 20; }
        if self.exoledger_entries >= 0 { score += 15; }
        let (up, tot) = self.critical_counts();
        if tot > 0 { score += (up * 35) / tot; }
        if self.ipc_dropped == 0 { score += 10; }
        score.min(100)
    }

    pub fn radar_scores(&self) -> [(&'static str, f32); 6] {
        let (up, tot) = self.critical_counts();
        let cpu_s = (1.0 - (self.cpu_load_pct as f32 / 140.0)).clamp(0.35, 0.98);
        let mem_s = (1.0 - (self.mem_used_pct as f32 / 120.0)).clamp(0.35, 0.98);
        let ipc_s = if self.ipc_dropped == 0 { 0.96 } else { 0.72 };
        let fs_s = (1.0 - (self.exofs_used_kb as f32 / self.exofs_total_kb.max(1) as f32) * 0.5)
            .clamp(0.5, 0.97);
        let shield_up = self.services.iter().any(|s| s.name == "exo_shield" && s.online);
        let shield_s = if shield_up { 0.95 } else { 0.45 };
        let phx_s = if self.phoenix_state >= 1 && up == tot { 0.98 } else { 0.75 };
        [
            ("CPU", cpu_s),
            ("Mémoire", mem_s),
            ("IPC", ipc_s),
            ("ExoFS", fs_s),
            ("Sécurité", shield_s),
            ("Résilience", phx_s),
        ]
    }

    /// Lightweight clock-only poll (2 syscalls) used when no live telemetry view is visible.
    pub fn poll_clock_only(&mut self) {
        self.poll_clock();
    }

    pub fn poll(&mut self) {
        self.sample_tick += 1;
        self.poll_clock();
        self.poll_kernel_perf();
        self.poll_services();
        self.poll_processes();
        self.poll_phoenix();

        let d_sys = self.dispatch_total.saturating_sub(self.prev_dispatch);
        let d_ipc = self.ipc_sent.saturating_sub(self.prev_ipc);
        self.prev_dispatch = self.dispatch_total;
        self.prev_ipc = self.ipc_sent;

        let sys_rate = if self.sample_tick == 1 {
            (self.dispatch_total % 800).max(40) as u32
        } else {
            (d_sys as u32).min(50_000)
        };
        let ipc_rate = if self.sample_tick == 1 {
            (self.ipc_sent % 400).max(20) as u32
        } else {
            (d_ipc as u32).min(25_000)
        };

        let raw_load = ((sys_rate / 25) + (ipc_rate / 15) + 4).clamp(3, 96);
        self.cpu_load_pct = (self.cpu_load_pct * 2 + raw_load) / 3;
        let proc_mem_kb: u64 = self.processes.iter().map(|p| p.memory_kb).sum();
        if proc_mem_kb > 0 {
            self.used_ram_mb = (proc_mem_kb / 1024).max(64);
        }
        self.mem_used_pct = ((self.used_ram_mb * 100) / self.total_ram_mb.max(1)).clamp(2, 99) as u32;

        push_ring(&mut self.syscall_rate_hist, sys_rate);
        push_ring(&mut self.ipc_rate_hist, ipc_rate);
        push_ring(&mut self.cpu_load_hist, self.cpu_load_pct);
        push_ring(&mut self.mem_pct_hist, self.mem_used_pct);
    }

    /// Stop a non-critical user process via real `SYS_KILL(pid, SIGTERM = 15)`.
    pub fn terminate_process(&mut self, pid: u32) -> Result<(), &'static str> {
        if let Some(p) = self.processes.iter().find(|p| p.pid == pid) {
            if p.critical || pid <= 2 {
                return Err("Processus système critique protégé par la politique");
            }
        }
        #[cfg(target_os = "exo")]
        {
            let rc = unsafe { syscall::syscall2(syscall::SYS_KILL, pid as u64, 15) };
            if rc < 0 {
                return Err("Refusé par le noyau / capacité manquante");
            }
        }
        #[cfg(not(target_os = "exo"))]
        {
            self.processes.retain(|p| p.pid != pid);
        }
        Ok(())
    }

    pub fn trigger_phoenix_switch(&mut self) -> bool {
        #[cfg(target_os = "exo")]
        {
            let mut rec = PhoenixTimingRecord::default();
            let rc = unsafe {
                syscall::syscall3(
                    syscall::SYS_PHOENIX_FORCE,
                    1,
                    &mut rec as *mut PhoenixTimingRecord as u64,
                    core::mem::size_of::<PhoenixTimingRecord>() as u64,
                )
            };
            if rc == 0 {
                self.phoenix_rec = rec;
                self.last_phoenix_msg = format!(
                    "Bascule A/B réussie — cycle #{} ({} µs)",
                    rec.cycle_id, rec.delta_sla_us
                );
                return true;
            }
            self.last_phoenix_msg = format!("Refusé par politique (rc={})", rc);
            false
        }
        #[cfg(not(target_os = "exo"))]
        {
            self.phoenix_rec.cycle_id += 1;
            self.phoenix_rec.crypto_epoch_id += 1;
            self.phoenix_rec.delta_s0_freeze_us = 412;
            self.phoenix_rec.delta_s0_forge_us = 9840;
            self.phoenix_rec.delta_s0_us = 10_420;
            self.phoenix_rec.delta_crypto_us = 1180;
            self.phoenix_rec.delta_sla_us = 11_600;
            self.last_phoenix_msg = format!(
                "Test de résurrection #{} : SLA = 11,6 ms",
                self.phoenix_rec.cycle_id
            );
            true
        }
    }

    pub fn create_exofs_snapshot(&mut self, tag: &str) -> String {
        let label = format!(
            "snap-{:02}{:02}{:02} ({})",
            self.clock.hour, self.clock.minute, self.clock.second, tag
        );
        #[cfg(target_os = "exo")]
        {
            let cname = format!("{}\0", tag);
            let rc = unsafe {
                syscall::syscall3(
                    syscall::SYS_EXOFS_SNAPSHOT_CREATE,
                    cname.as_ptr() as u64,
                    tag.len() as u64,
                    0,
                )
            };
            if rc >= 0 {
                let s = format!("{} [id={}]", label, rc);
                self.exofs_snapshots.insert(0, s.clone());
                return s;
            }
        }
        self.exofs_snapshots.insert(0, label.clone());
        self.exofs_snapshots.truncate(16);
        label
    }

    pub fn system_report_text(&self, hostname: &str) -> String {
        let (up, tot) = self.critical_counts();
        format!(
            "=== Rapport Système ExoOS ===\n\
             Machine      : {}\n\
             Système      : ExoOS 0.2.0 (x86_64 Micro-noyau Rust)\n\
             Processeur   : x86_64 ({} cœurs, charge {} %)\n\
             Mémoire      : {} Mio / {} Mio ({} %)\n\
             Services     : {}/{} services critiques actifs\n\
             ExoPhoenix   : état={}, dernier cycle={}, SLA={} µs\n\
             ExoLedger    : {} entrées d'audit vérifiées\n\
             Syscalls/IPC : {} appels système, {} messages IPC ({} perdus)\n",
            hostname,
            self.cpu_cores,
            self.cpu_load_pct,
            self.used_ram_mb,
            self.total_ram_mb,
            self.mem_used_pct,
            up,
            tot,
            self.phoenix_state,
            self.phoenix_rec.cycle_id,
            self.phoenix_rec.delta_sla_us,
            self.exoledger_entries.max(0),
            self.dispatch_total,
            self.ipc_sent,
            self.ipc_dropped
        )
    }

    fn poll_clock(&mut self) {
        #[cfg(target_os = "exo")]
        {
            #[repr(C)]
            #[derive(Default)]
            struct Timespec { tv_sec: i64, tv_nsec: i64 }
            let mut ts = Timespec::default();
            let mut mono = Timespec::default();
            let _ = unsafe { syscall::syscall2(syscall::SYS_CLOCK_GETTIME, 1, &mut mono as *mut Timespec as u64) };
            let rc = unsafe { syscall::syscall2(syscall::SYS_CLOCK_GETTIME, 0, &mut ts as *mut Timespec as u64) };
            if rc == 0 && ts.tv_sec > 0 {
                self.clock = unix_to_rtc(ts.tv_sec, self.tz_idx, mono.tv_sec.max(0) as u64);
            }
        }
        #[cfg(not(target_os = "exo"))]
        {
            if let Ok(d) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
                self.clock = unix_to_rtc(d.as_secs() as i64, self.tz_idx, self.sample_tick);
            }
        }
    }

    fn poll_kernel_perf(&mut self) {
        #[cfg(target_os = "exo")]
        {
            let read = |id: u64| -> u64 {
                let v = unsafe { syscall::syscall2(syscall::SYS_EXO_PERF_READ, id, 0) };
                if v > 0 { v as u64 } else { 0 }
            };
            self.dispatch_total = read(syscall::EXO_PERF_DISPATCH_TOTAL);
            self.dispatch_fast = read(syscall::EXO_PERF_DISPATCH_FAST_PATH);
            self.dispatch_slow = read(syscall::EXO_PERF_DISPATCH_SLOW_PATH);
            self.dispatch_enosys = read(syscall::EXO_PERF_DISPATCH_ENOSYS);
            self.ipc_sent = read(syscall::EXO_PERF_IPC_MESSAGES_SENT);
            self.ipc_recv = read(syscall::EXO_PERF_IPC_MESSAGES_RECEIVED);
            self.ipc_dropped = read(syscall::EXO_PERF_IPC_MESSAGES_DROPPED);
            self.ipc_rpc = read(syscall::EXO_PERF_IPC_RPC_CALLS);
            self.ipc_timeouts = read(syscall::EXO_PERF_IPC_RPC_TIMEOUTS);
            self.futex_waits = read(syscall::EXO_PERF_IPC_FUTEX_WAITS);
            self.futex_wakes = read(syscall::EXO_PERF_IPC_FUTEX_WAKES);
        }
        #[cfg(not(target_os = "exo"))]
        {
            self.dispatch_total += 320 + (self.sample_tick * 17) % 180;
            self.dispatch_fast = (self.dispatch_total * 68) / 100;
            self.dispatch_slow = self.dispatch_total - self.dispatch_fast;
            self.ipc_sent += 140 + (self.sample_tick * 13) % 90;
            self.ipc_recv = self.ipc_sent;
            self.ipc_rpc += 48;
            self.futex_waits += 12;
            self.futex_wakes += 12;
        }
    }

    fn poll_services(&mut self) {
        #[cfg(target_os = "exo")]
        {
            for svc in &mut self.services {
                let b = svc.name.as_bytes();
                let rc = unsafe { syscall::syscall3(syscall::SYS_IPC_LOOKUP, b.as_ptr() as u64, b.len() as u64, 0) };
                svc.online = rc > 0;
            }
        }
        #[cfg(not(target_os = "exo"))]
        {
            for svc in &mut self.services { svc.online = true; }
        }
    }

    fn poll_processes(&mut self) {
        #[cfg(target_os = "exo")]
        {
            let mut raw = [syscall::ExoProcessInfo::zeroed(); 48];
            let rc = unsafe { syscall::syscall2(syscall::SYS_EXO_PROCESS_LIST, raw.as_mut_ptr() as u64, raw.len() as u64) };
            if rc > 0 {
                let count = (rc as usize).min(raw.len());
                self.processes.clear();
                for p in &raw[..count] {
                    let end = p.name.iter().position(|&b| b == 0).unwrap_or(p.name.len());
                    let name = String::from_utf8_lossy(&p.name[..end]).into_owned();
                    let crit = p.pid <= 14 || name.ends_with("-server") || name == "exo-shield" || name == "exo-ipc-router";
                    self.processes.push(ProcessEntry {
                        pid: p.pid,
                        ppid: p.ppid,
                        state: p.state,
                        cpu_ticks: p.utime_ns.saturating_add(p.stime_ns) / 1_000_000,
                        memory_kb: (p.threads as u64).max(1) * 4096,
                        name: if name.is_empty() { format!("task-{}", p.pid) } else { name },
                        critical: crit,
                    });
                }
            }
        }
        #[cfg(not(target_os = "exo"))]
        {
            if self.processes.is_empty() {
                self.processes = vec![
                    ProcessEntry { pid: 1, ppid: 0, state: 1, cpu_ticks: 420, memory_kb: 4096, name: String::from("exo-init-server"), critical: true },
                    ProcessEntry { pid: 2, ppid: 1, state: 1, cpu_ticks: 980, memory_kb: 2048, name: String::from("exo-ipc-router"), critical: true },
                    ProcessEntry { pid: 5, ppid: 1, state: 1, cpu_ticks: 610, memory_kb: 16384, name: String::from("exo-vfs-server"), critical: true },
                    ProcessEntry { pid: 10, ppid: 1, state: 1, cpu_ticks: 540, memory_kb: 8192, name: String::from("exo-shield"), critical: true },
                    ProcessEntry { pid: 13, ppid: 1, state: 1, cpu_ticks: 1120, memory_kb: 24576, name: String::from("exo-fb-server"), critical: true },
                    ProcessEntry { pid: 18, ppid: 1, state: 1, cpu_ticks: 1450, memory_kb: 32768, name: String::from("exo-compositor"), critical: false },
                    ProcessEntry { pid: 22, ppid: 18, state: 1, cpu_ticks: 95, memory_kb: 6144, name: String::from("exosh"), critical: false },
                ];
            }
        }
    }

    fn poll_phoenix(&mut self) {
        #[cfg(target_os = "exo")]
        {
            self.phoenix_state = unsafe { syscall::syscall0(syscall::SYS_EXO_PHOENIX_STATE_GET) };
            let mut rec = PhoenixTimingRecord::default();
            let mrc = unsafe {
                syscall::syscall3(syscall::SYS_EXOPHOENIX_TEST, 0, &mut rec as *mut PhoenixTimingRecord as u64, core::mem::size_of::<PhoenixTimingRecord>() as u64)
            };
            if mrc > 0 && rec.magic == 0x504D_4554 { self.phoenix_rec = rec; }
            let lrc = unsafe { syscall::syscall3(syscall::SYS_EXOPHOENIX_TEST, 1, 0, 0) };
            if lrc >= 0 { self.exoledger_entries = lrc; }
        }
        #[cfg(not(target_os = "exo"))]
        {
            self.phoenix_state = 1;
            self.exoledger_entries = 112;
            if self.phoenix_rec.delta_s0_us == 0 {
                self.phoenix_rec.cycle_id = 1;
                self.phoenix_rec.crypto_epoch_id = 2;
                self.phoenix_rec.delta_s0_freeze_us = 438;
                self.phoenix_rec.delta_s0_forge_us = 9968;
                self.phoenix_rec.delta_s0_us = 10_645;
                self.phoenix_rec.delta_crypto_us = 1273;
                self.phoenix_rec.delta_sla_us = 12_178;
            }
        }
    }
}

fn push_ring(hist: &mut Vec<u32>, val: u32) {
    if hist.len() >= HISTORY_LEN {
        hist.remove(0);
    }
    hist.push(val);
}
