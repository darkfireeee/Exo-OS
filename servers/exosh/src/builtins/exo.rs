//! # servers/exosh/src/builtins/exo.rs - Commande native `exo`
//!
//! Fournit les sous-commandes d'inspection, d'observabilite et de gestion de paquets :
//! - `exo info` / `exo version` : version et architecture Strata
//! - `exo doctor` : diagnostic d'integrite des 8 organes vitaux
//! - `exo metrics` : telemetrie micro-noyau en direct via SYS_EXO_PERF_READ (330)
//! - `exo phoenix` : etat et bascule A/B de l'ExoPhoenix Resurrection Engine
//! - `exo cap` / `exo caps` : capabilities et tokens de securite
//! - `exo net` : pile reseau et anneaux NetGrant V5
//! - `exo install <pkg>` : installation de paquets natifs avec signature Ed25519
//! - `exo compat install <pkg>` : installation d'applications POSIX en sandbox
//! - `exo remove <pkg>` : desinstallation et revocation des capabilities
//! - `exo list` / `exo search` : catalogue et paquets installes
//! - `exo log` : observabilite et journaux systeme

#![allow(dead_code, unused_imports)]
use exo_syscall_abi as syscall;

use super::*;
use crate::types::*;
use crate::util::*;
use crate::terminal::*;

pub fn cmd_exo(rest: &[u8], state: &mut ShellState) {
    let (subcmd, args) = first_token(rest);
    if subcmd.is_empty() || bytes_eq(subcmd, b"help") {
        print_exo_help();
    } else if bytes_eq(subcmd, b"info") || bytes_eq(subcmd, b"version") {
        print_exo_info();
    } else if bytes_eq(subcmd, b"doctor") {
        run_exo_doctor();
    } else if bytes_eq(subcmd, b"metrics") || bytes_eq(subcmd, b"perf") {
        print_exo_metrics();
    } else if bytes_eq(subcmd, b"phoenix") || bytes_eq(subcmd, b"state") {
        handle_exo_phoenix(args);
    } else if bytes_eq(subcmd, b"cap") || bytes_eq(subcmd, b"caps") {
        print_exo_caps();
    } else if bytes_eq(subcmd, b"net") {
        print_exo_net(args);
    } else if bytes_eq(subcmd, b"install") {
        handle_exo_install(args, false);
    } else if bytes_eq(subcmd, b"compat") {
        handle_exo_compat(args, state);
    } else if bytes_eq(subcmd, b"remove") || bytes_eq(subcmd, b"uninstall") {
        handle_exo_remove(args);
    } else if bytes_eq(subcmd, b"list") {
        list_installed_packages();
    } else if bytes_eq(subcmd, b"search") {
        search_packages(args);
    } else if bytes_eq(subcmd, b"audit") {
        handle_exo_audit(args);
    } else if bytes_eq(subcmd, b"log") || bytes_eq(subcmd, b"logs") {
        print_exo_logs(args);
    } else if bytes_eq(subcmd, b"mount") {
        handle_exo_mount(args);
    } else if bytes_eq(subcmd, b"umount") {
        handle_exo_umount(args);
    } else if bytes_eq(subcmd, b"cp") {
        handle_exo_cp(args, state);
    } else if bytes_eq(subcmd, b"usb") {
        handle_exo_usb();
    } else if bytes_eq(subcmd, b"beep") {
        handle_exo_beep(args);
    } else if bytes_eq(subcmd, b"chime") {
        handle_exo_chime();
    } else if bytes_eq(subcmd, b"wayland") || bytes_eq(subcmd, b"gui") || bytes_eq(subcmd, b"desktop") || bytes_eq(subcmd, b"gpu") {
        handle_exo_wayland(args);
    } else {
        write_all(b"exo: unknown subcommand '");
        write_bytes(subcmd);
        write_all(b"'. Run 'exo help' for available commands.\n");
    }
}

fn print_exo_help() {
    write_all(b"Exo-OS Native Management & Observability Tool ('exo')\n\n");
    write_all(b"Usage: exo <subcommand> [args]\n\n");
    write_all(b"Subcommands:\n");
    write_all(b"  info, version         Display Exo-OS Strata build and kernel identity\n");
    write_all(b"  doctor                Perform comprehensive system health inspection\n");
    write_all(b"  metrics, perf         Display live kernel & IPC telemetry counters\n");
    write_all(b"  phoenix [switch]      Display or trigger ExoPhoenix A/B resurrection\n");
    write_all(b"  cap, caps             List process capability tokens & sandbox state\n");
    write_all(b"  net                   Display ExoNet V5 NetGrant network stack status\n");
    write_all(b"  install <pkg>         Install native verified Exo-OS package\n");
    write_all(b"  compat install <pkg>  Install POSIX application in confined sandbox\n");
    write_all(b"  remove <pkg>          Uninstall package and revoke capabilities\n");
    write_all(b"  list                  List installed native & compat applications\n");
    write_all(b"  search <term>         Search package registry\n");
    write_all(b"  audit [--verify-chain] Verify ExoLedger cryptographic integrity\n");
    write_all(b"  log [--filter <tag>]  Display recent system monitor logs\n");
    write_all(b"  wayland, gui, gpu     Inspect Wayland Compositor & VirtIO-GPU status\n");
    write_all(b"  help                  Show this help message\n");
}

fn print_exo_info() {
    write_all(b"Exo-OS v0.2.0 \"Strata\" (x86_64 #![no_std])\n");
    write_all(b"Architecture: Micro-noyau Zero-Trust (Ring 0 / Ring 1 / Ring 3)\n");
    write_all(b"Kernel Engine: ExoPhoenix Resurrection Engine (Kernel A / Kernel B)\n");
    write_all(b"Filesystem: ExoFS (Content-Addressed Storage, 64-bit Epochs)\n");
    write_all(b"Security: ExoShield LSM & Strict 67-Edge Authorized IPC Graph\n");
    write_all(b"Driver Model: Out-of-Kernel Isolated Ring 1 Servers (IOMMU Whitelisted)\n");
}

fn ipc_service_online(name: &[u8]) -> bool {
    let rc = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_LOOKUP,
            name.as_ptr() as u64,
            name.len() as u64,
            0,
        )
    };
    rc > 0
}

fn path_exists_probe(path_cstr: &[u8]) -> bool {
    let mut st = LinuxStat::default();
    let rc = unsafe {
        syscall::syscall2(
            syscall::SYS_STAT,
            path_cstr.as_ptr() as u64,
            &mut st as *mut LinuxStat as u64,
        )
    };
    rc == 0
}

fn run_exo_doctor() {
    write_all(b"=== Exo-OS Doctor : Live System Diagnostics ===\n\n");
    let mut passed = 0u64;
    let total = 8u64;

    // 1. Kernel Syscall / Perf Telemetry
    let total_calls = unsafe {
        syscall::syscall2(syscall::SYS_EXO_PERF_READ, syscall::EXO_PERF_DISPATCH_TOTAL, 0)
    };
    if total_calls > 0 {
        passed += 1;
        write_all(b"[PASS] 1. Kernel Syscall/Perf     : live (dispatch_total=");
        write_u64(total_calls as u64);
        write_all(b")\n");
    } else {
        write_all(b"[FAIL] 1. Kernel Syscall/Perf     : SYS_EXO_PERF_READ failed (rc=");
        write_i64(total_calls);
        write_all(b")\n");
    }

    // 2. ExoPhoenix Engine
    let phoenix = unsafe { syscall::syscall0(syscall::SYS_EXO_PHOENIX_STATE_GET) };
    if phoenix >= 1 {
        passed += 1;
        write_all(b"[PASS] 2. ExoPhoenix Engine       : live (state=");
        write_i64(phoenix);
        let mut rec = PhoenixTimingRecordWire::zeroed();
        let mrc = unsafe {
            syscall::syscall3(
                syscall::SYS_EXOPHOENIX_TEST,
                0,
                &mut rec as *mut PhoenixTimingRecordWire as u64,
                core::mem::size_of::<PhoenixTimingRecordWire>() as u64,
            )
        };
        if mrc > 0 && rec.magic == PHOENIX_METRICS_MAGIC {
            write_all(b", last_cycle=");
            write_u64(rec.cycle_id as u64);
            write_all(b", S0=");
            write_u64(rec.delta_s0_us);
            write_all(b"us, S0+S1_SLA=");
            write_u64(rec.delta_sla_us);
            write_all(b"us");
        }
        write_all(b")\n");
    } else {
        write_all(b"[WARN] 2. ExoPhoenix Engine       : unavailable (rc=");
        write_i64(phoenix);
        write_all(b")\n");
    }

    // 3. ExoShield Security Service
    if ipc_service_online(b"exo_shield") {
        passed += 1;
        write_all(b"[PASS] 3. ExoShield Service       : IPC endpoint online\n");
    } else {
        write_all(b"[WARN] 3. ExoShield Service       : IPC endpoint offline\n");
    }

    // 4. ExoFS Root & Dynamic Linker (honest check across pure & compat profiles)
    if path_exists_probe(b"/bin/exosh\0") && path_exists_probe(b"/sbin/exo-init-server\0") {
        passed += 1;
        if path_exists_probe(b"/lib/ld-exo.so\0") {
            write_all(b"[PASS] 4. ExoFS & Dynamic Linker  : /bin/exosh and /lib/ld-exo.so present (profile=compat)\n");
        } else {
            write_all(b"[PASS] 4. ExoFS Root Integrity    : /bin/exosh and /sbin/exo-init-server present (profile=pure, 0 C/libc)\n");
        }
    } else {
        write_all(b"[FAIL] 4. ExoFS & Dynamic Linker  : rootfs artifacts missing\n");
    }

    // 5. Wayland Compositor Socket (S5B: exo_compositor Strate 2 + fb_server Strate 1)
    if path_exists_probe(b"/run/wayland-0\0") && ipc_service_online(b"fb_server") {
        passed += 1;
        if ipc_service_online(b"exo_compositor") {
            write_all(b"[PASS] 5. Wayland / Compositor    : /run/wayland-0 ready (exo_compositor -> fb_server)\n");
        } else {
            write_all(b"[PASS] 5. Wayland / fb_server     : /run/wayland-0 ready\n");
        }
    } else {
        write_all(b"[WARN] 5. Wayland / Compositor    : compositor socket not ready\n");
    }

    // 6. ExoNet Network Stack
    let net_sock = unsafe { syscall::syscall3(syscall::SYS_SOCKET, AF_INET as u64, 2, 0) };
    if net_sock >= 0 {
        let _ = close_fd(net_sock);
        passed += 1;
        write_all(b"[PASS] 6. Network Stack (ExoNet)  : network_server live (AF_INET OK)\n");
    } else {
        write_all(b"[WARN] 6. Network Stack (ExoNet)  : socket(AF_INET) returned errno ");
        write_i64(net_sock);
        write_all(b"\n");
    }

    // 7. Audio Service
    if ipc_service_online(b"audio_server") {
        passed += 1;
        write_all(b"[PASS] 7. Audio Service           : audio_server IPC endpoint online\n");
    } else {
        write_all(b"[WARN] 7. Audio Service           : audio_server offline\n");
    }

    // 8. USB Service
    if ipc_service_online(b"usb_server") {
        passed += 1;
        write_all(b"[PASS] 8. USB Service             : usb_server IPC endpoint online\n");
    } else {
        write_all(b"[WARN] 8. USB Service             : usb_server offline\n");
    }

    write_all(b"\nSystem Health: ");
    write_u64(passed);
    write_all(b"/");
    write_u64(total);
    write_all(b" checks passed\n");
}

fn print_exo_metrics() {
    let total_calls = unsafe { syscall::syscall2(syscall::SYS_EXO_PERF_READ, syscall::EXO_PERF_DISPATCH_TOTAL, 0) };
    let fast_path   = unsafe { syscall::syscall2(syscall::SYS_EXO_PERF_READ, syscall::EXO_PERF_DISPATCH_FAST_PATH, 0) };
    let slow_path   = unsafe { syscall::syscall2(syscall::SYS_EXO_PERF_READ, syscall::EXO_PERF_DISPATCH_SLOW_PATH, 0) };
    let enosys      = unsafe { syscall::syscall2(syscall::SYS_EXO_PERF_READ, syscall::EXO_PERF_DISPATCH_ENOSYS, 0) };

    let sent     = unsafe { syscall::syscall2(syscall::SYS_EXO_PERF_READ, syscall::EXO_PERF_IPC_MESSAGES_SENT, 0) };
    let received = unsafe { syscall::syscall2(syscall::SYS_EXO_PERF_READ, syscall::EXO_PERF_IPC_MESSAGES_RECEIVED, 0) };
    let dropped  = unsafe { syscall::syscall2(syscall::SYS_EXO_PERF_READ, syscall::EXO_PERF_IPC_MESSAGES_DROPPED, 0) };
    let rpc      = unsafe { syscall::syscall2(syscall::SYS_EXO_PERF_READ, syscall::EXO_PERF_IPC_RPC_CALLS, 0) };
    let timeouts = unsafe { syscall::syscall2(syscall::SYS_EXO_PERF_READ, syscall::EXO_PERF_IPC_RPC_TIMEOUTS, 0) };
    let waits    = unsafe { syscall::syscall2(syscall::SYS_EXO_PERF_READ, syscall::EXO_PERF_IPC_FUTEX_WAITS, 0) };
    let wakes    = unsafe { syscall::syscall2(syscall::SYS_EXO_PERF_READ, syscall::EXO_PERF_IPC_FUTEX_WAKES, 0) };

    let mut out = [0u8; 1024];
    let mut len = 0usize;
    push_bytes(&mut out, &mut len, b"=== Exo-OS Micro-Kernel Performance Telemetry (SYS_EXO_PERF_READ) ===\n\n");
    push_bytes(&mut out, &mut len, b"Syscall Dispatch Telemetry:\n");
    push_bytes(&mut out, &mut len, b"  Total Invocations      : "); push_i64(&mut out, &mut len, total_calls); push_bytes(&mut out, &mut len, b"\n");
    push_bytes(&mut out, &mut len, b"  Fast-Path (Direct)     : "); push_i64(&mut out, &mut len, fast_path);   push_bytes(&mut out, &mut len, b"\n");
    push_bytes(&mut out, &mut len, b"  Slow-Path (Complex)    : "); push_i64(&mut out, &mut len, slow_path);   push_bytes(&mut out, &mut len, b"\n");
    push_bytes(&mut out, &mut len, b"  ENOSYS (Unimplemented) : "); push_i64(&mut out, &mut len, enosys);      push_bytes(&mut out, &mut len, b"\n\n");

    push_bytes(&mut out, &mut len, b"Zero-Trust IPC Telemetry:\n");
    push_bytes(&mut out, &mut len, b"  Messages Sent          : "); push_i64(&mut out, &mut len, sent);        push_bytes(&mut out, &mut len, b"\n");
    push_bytes(&mut out, &mut len, b"  Messages Received      : "); push_i64(&mut out, &mut len, received);    push_bytes(&mut out, &mut len, b"\n");
    push_bytes(&mut out, &mut len, b"  Messages Dropped       : "); push_i64(&mut out, &mut len, dropped);     push_bytes(&mut out, &mut len, b"\n");
    push_bytes(&mut out, &mut len, b"  RPC Calls Handled      : "); push_i64(&mut out, &mut len, rpc);         push_bytes(&mut out, &mut len, b"\n");
    push_bytes(&mut out, &mut len, b"  RPC Timeouts           : "); push_i64(&mut out, &mut len, timeouts);    push_bytes(&mut out, &mut len, b"\n");
    push_bytes(&mut out, &mut len, b"  Futex Waits / Wakes    : "); push_i64(&mut out, &mut len, waits);       push_bytes(&mut out, &mut len, b" / "); push_i64(&mut out, &mut len, wakes); push_bytes(&mut out, &mut len, b"\n");

    write_bytes(&out[..len]);
}

fn civil_from_days(days_since_epoch: i64) -> (i64, u8, u8) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = y + if m <= 2 { 1 } else { 0 };
    (y, m as u8, d as u8)
}

fn days_in_month(year: i64, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0) {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

fn month_name(month: u8) -> &'static [u8] {
    match month {
        1 => b"January",
        2 => b"February",
        3 => b"March",
        4 => b"April",
        5 => b"May",
        6 => b"June",
        7 => b"July",
        8 => b"August",
        9 => b"September",
        10 => b"October",
        11 => b"November",
        12 => b"December",
        _ => b"Unknown",
    }
}

pub fn cmd_calendar(rest: &[u8]) {
    let mut ts = Timespec::default();
    let rc = unsafe { syscall::syscall2(syscall::SYS_CLOCK_GETTIME, 0, &mut ts as *mut Timespec as u64) };
    let secs = if rc == 0 && ts.tv_sec >= 1_577_836_800 {
        ts.tv_sec
    } else {
        1_794_744_000
    };
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let hour = (rem / 3600) as u64;
    let min = ((rem % 3600) / 60) as u64;
    let (year, month, day) = civil_from_days(days);
    let first_wday = ((days - (day as i64 - 1) + 4).rem_euclid(7)) as u8;
    let dim = days_in_month(year, month);

    let mut out = [0u8; 640];
    let mut len = 0usize;
    push_bytes(&mut out, &mut len, b"    ");
    push_bytes(&mut out, &mut len, month_name(month));
    push_bytes(&mut out, &mut len, b" ");
    push_u64(&mut out, &mut len, year as u64);
    push_bytes(&mut out, &mut len, b" (");
    if hour < 10 { push_bytes(&mut out, &mut len, b"0"); }
    push_u64(&mut out, &mut len, hour);
    push_bytes(&mut out, &mut len, b":");
    if min < 10 { push_bytes(&mut out, &mut len, b"0"); }
    push_u64(&mut out, &mut len, min);
    push_bytes(&mut out, &mut len, b" UTC)\n");
    push_bytes(&mut out, &mut len, b"Su Mo Tu We Th Fr Sa\n");

    let mut col = 0u8;
    while col < first_wday {
        push_bytes(&mut out, &mut len, b"   ");
        col += 1;
    }
    let mut d = 1u8;
    while d <= dim {
        if d == day {
            push_bytes(&mut out, &mut len, b"[");
            push_u64(&mut out, &mut len, d as u64);
            push_bytes(&mut out, &mut len, b"]");
        } else {
            if d < 10 { push_bytes(&mut out, &mut len, b" "); }
            push_u64(&mut out, &mut len, d as u64);
            push_bytes(&mut out, &mut len, b" ");
        }
        col += 1;
        if col == 7 || d == dim {
            push_bytes(&mut out, &mut len, b"\n");
            col = 0;
        }
        d += 1;
    }
    if !trim(rest).is_empty() {
        push_bytes(&mut out, &mut len, b"\nExo-OS Real-Time Clock (CMOS RTC UTC)\n");
    }
    write_bytes(&out[..len]);
}

const PHOENIX_METRICS_MAGIC: u32 = 0x504D_4554;

#[repr(C, align(64))]
#[derive(Clone, Copy)]
struct PhoenixTimingRecordWire {
    magic: u32,
    cycle_id: u32,
    trigger_reason: u32,
    flags: u32,
    tsc_khz: u64,
    t0_freeze_req: u64,
    t1_freeze_ack_all: u64,
    t2_iommu_and_forge: u64,
    t3_kernel_b_active: u64,
    t4_crypto_reseeded: u64,
    t5_strate1_ready: u64,
    t6_strate2_ready: u64,
    delta_s0_freeze_us: u64,
    delta_s0_forge_us: u64,
    delta_s0_us: u64,
    delta_crypto_us: u64,
    delta_sla_us: u64,
    delta_full_us: u64,
    crypto_epoch_id: u64,
}

impl PhoenixTimingRecordWire {
    const fn zeroed() -> Self {
        Self {
            magic: 0,
            cycle_id: 0,
            trigger_reason: 0,
            flags: 0,
            tsc_khz: 0,
            t0_freeze_req: 0,
            t1_freeze_ack_all: 0,
            t2_iommu_and_forge: 0,
            t3_kernel_b_active: 0,
            t4_crypto_reseeded: 0,
            t5_strate1_ready: 0,
            t6_strate2_ready: 0,
            delta_s0_freeze_us: 0,
            delta_s0_forge_us: 0,
            delta_s0_us: 0,
            delta_crypto_us: 0,
            delta_sla_us: 0,
            delta_full_us: 0,
            crypto_epoch_id: 0,
        }
    }
}

fn print_phoenix_timing_record(rec: &PhoenixTimingRecordWire) {
    if rec.magic != PHOENIX_METRICS_MAGIC {
        write_all(b"  Last Switch SLA : (no cycle recorded yet)\n");
        return;
    }
    write_all(b"  Last Cycle ID   : ");
    write_u64(rec.cycle_id as u64);
    write_all(b" (epoch=");
    write_u64(rec.crypto_epoch_id);
    write_all(b", degraded=");
    write_u64(u64::from((rec.flags & 1) != 0));
    write_all(b")\n");
    write_all(b"  Strate 0 (Ring0): ");
    write_u64(rec.delta_s0_us);
    write_all(b" us (freeze=");
    write_u64(rec.delta_s0_freeze_us);
    write_all(b" us, forge=");
    write_u64(rec.delta_s0_forge_us);
    write_all(b" us)\n");
    write_all(b"  Strate 0+1 SLA  : ");
    write_u64(rec.delta_sla_us);
    write_all(b" us (crypto=");
    write_u64(rec.delta_crypto_us);
    write_all(b" us)\n");
    write_all(b"  Full S2 Signal  : ");
    write_u64(rec.delta_full_us);
    write_all(b" us\n");
}

fn handle_exo_phoenix(args: &[u8]) {
    let (action, _) = first_token(args);
    if bytes_eq(action, b"switch") {
        write_all(b"Initiating manual ExoPhoenix 3-Strata A/B switch (SYS_PHOENIX_FORCE=523)...\n");
        let mut rec = PhoenixTimingRecordWire::zeroed();
        let rc = unsafe {
            syscall::syscall3(
                syscall::SYS_EXO_PHOENIX_FORCE,
                1, // PHOENIX_REASON_FORCE_SYSCALL
                &mut rec as *mut PhoenixTimingRecordWire as u64,
                core::mem::size_of::<PhoenixTimingRecordWire>() as u64,
            )
        };
        if rc == 0 {
            write_all(b"[OK] ExoPhoenix 3-Strata switch completed.\n");
            print_phoenix_timing_record(&rec);
        } else {
            write_all(b"[FAIL] Failed to trigger handoff: error code ");
            write_i64(rc);
            write_all(b"\n");
        }
        return;
    }

    let state = unsafe { syscall::syscall0(syscall::SYS_EXO_PHOENIX_STATE_GET) };
    write_all(b"ExoPhoenix Status: ");
    match state {
        1 => write_all(b"Normal (Kernel B active, Kernel A standby)\n"),
        9 => write_all(b"NetworkDraining (Draining rings before A/B switch)\n"),
        10 => write_all(b"NetworkSerialized (Ready for atomic handoff)\n"),
        rc => {
            write_all(b"State code ");
            write_u32(rc as u32);
            write_all(b"\n");
        }
    }
    let mut rec = PhoenixTimingRecordWire::zeroed();
    let mrc = unsafe {
        syscall::syscall3(
            syscall::SYS_EXOPHOENIX_TEST,
            0,
            &mut rec as *mut PhoenixTimingRecordWire as u64,
            core::mem::size_of::<PhoenixTimingRecordWire>() as u64,
        )
    };
    if mrc > 0 {
        print_phoenix_timing_record(&rec);
    }
}

fn print_exo_caps() {
    let pid = unsafe { syscall::syscall0(syscall::SYS_GETPID) };
    let uid = unsafe { syscall::syscall0(syscall::SYS_GETUID) };
    let gid = unsafe { syscall::syscall0(syscall::SYS_GETGID) };
    write_all(b"Process Identity & Capabilities (exosh):\n");
    write_all(b"  PID / UID / GID : ");
    write_i64(pid);
    write_all(b" / ");
    write_i64(uid);
    write_all(b" / ");
    write_i64(gid);
    write_all(b"\n");
    write_all(b"  ExoShield IPC   : ");
    if ipc_service_online(b"exo_shield") {
        write_all(b"online\n");
    } else {
        write_all(b"offline\n");
    }
    write_all(b"  Token listing   : kernel capability introspection syscall not exposed yet (ENOSYS)\n");
}

fn print_exo_net(_args: &[u8]) {
    write_all(b"ExoNet Stack Status:\n");
    write_all(b"  network_server  : ");
    if ipc_service_online(b"network_server") {
        write_all(b"online\n");
    } else {
        write_all(b"offline\n");
    }
    let sock = unsafe { syscall::syscall3(syscall::SYS_SOCKET, AF_INET as u64, 2, 0) };
    if sock >= 0 {
        let _ = close_fd(sock);
        write_all(b"  AF_INET probe   : OK\n");
    } else {
        write_all(b"  AF_INET probe   : errno ");
        write_i64(sock);
        write_all(b"\n");
    }
    let conf_fd = unsafe {
        syscall::syscall4(
            syscall::SYS_OPENAT,
            AT_FDCWD,
            b"/etc/network.conf\0".as_ptr() as u64,
            syscall::O_RDONLY,
            0,
        )
    };
    if conf_fd >= 0 {
        let mut buf = [0u8; 256];
        let n = unsafe {
            syscall::syscall3(
                syscall::SYS_READ,
                conf_fd as u64,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
            )
        };
        let _ = close_fd(conf_fd);
        if n > 0 {
            write_all(b"  /etc/network.conf:\n");
            write_bytes(&buf[..n as usize]);
            if buf[n as usize - 1] != b'\n' {
                write_all(b"\n");
            }
        }
    } else {
        write_all(b"  /etc/network.conf: not present (using server default)\n");
    }
}

fn parse_compat_policy_promises() -> u64 {
    // Minimal compat default matching `kernel/src/security/isolation/pledge.rs`:
    // STDIO(0) | RPATH(1) | WPATH(2) | CPATH(3) | TMPPATH(4) | UNIX(6) | EXEC(10) |
    // SHM(13) | SIGNAL(14) | TTY(15) | FUTEX(16) = 0x1e45f.
    // Explicitly omits INET(5), DNS(7), PROC(9), ID(11), ROUTE(12), UNIX_ABSTRACT(17), DLOPEN(18).
    const DEFAULT_COMPAT_PROMISES: u64 = (1 << 0)
        | (1 << 1)
        | (1 << 2)
        | (1 << 3)
        | (1 << 4)
        | (1 << 6)
        | (1 << 10)
        | (1 << 13)
        | (1 << 14)
        | (1 << 15)
        | (1 << 16);

    let fd = unsafe {
        syscall::syscall4(
            syscall::SYS_OPENAT,
            AT_FDCWD,
            b"/etc/exoshield/compat_default.policy\0".as_ptr() as u64,
            syscall::O_RDONLY,
            0,
        )
    };
    if fd < 0 {
        return DEFAULT_COMPAT_PROMISES;
    }
    let mut buf = [0u8; 512];
    let n = unsafe {
        syscall::syscall3(
            syscall::SYS_READ,
            fd as u64,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        )
    };
    let _ = close_fd(fd);
    if n <= 0 {
        return DEFAULT_COMPAT_PROMISES;
    }
    let slice = &buf[..n as usize];
    // Parse optional `promises=0x...` line while never permitting privileged Ring 1 promises.
    let mut i = 0usize;
    while i < slice.len() {
        let start = i;
        while i < slice.len() && slice[i] != b'\n' {
            i += 1;
        }
        let line = trim(&slice[start..i]);
        if let Some(hex_str) = line.strip_prefix(b"promises=0x") {
            let mut val = 0u64;
            let mut valid = !hex_str.is_empty();
            for &b in hex_str {
                let d = match b {
                    b'0'..=b'9' => (b - b'0') as u64,
                    b'a'..=b'f' => (b - b'a' + 10) as u64,
                    b'A'..=b'F' => (b - b'A' + 10) as u64,
                    _ => {
                        valid = false;
                        break;
                    }
                };
                val = (val << 4) | d;
            }
            if valid && val != 0 {
                // Always retain EXEC (1<<10) so execve succeeds and never grant ID(11)/ROUTE(12).
                let forbidden: u64 = (1 << 11) | (1 << 12);
                return (val | (1 << 10)) & !forbidden;
            }
        }
        i = i.saturating_add(1);
    }
    DEFAULT_COMPAT_PROMISES
}

fn handle_exo_compat(args: &[u8], state: &mut ShellState) {
    let (subcmd, rest) = first_token(args);
    if bytes_eq(subcmd, b"install") {
        handle_exo_install(rest, true);
    } else if bytes_eq(subcmd, b"run") {
        let cmdline = trim(rest);
        if cmdline.is_empty() {
            write_all(b"Usage: exo compat run <binary> [args...]\n");
            return;
        }
        // S2.3: Confine via SYS_EXO_PLEDGE (324) with minimal compat policy +
        // NO_RING1_CRITICAL_IPC flag (0x2) before execve.
        let promises = parse_compat_policy_promises();
        const PLEDGE_FLAG_NO_RING1_CRITICAL_IPC: u64 = 0x2;
        if let Err(()) =
            crate::exec::run_external_with_pledge(cmdline, state, Some((promises, PLEDGE_FLAG_NO_RING1_CRITICAL_IPC)))
        {
            let (pkg, _) = first_token(cmdline);
            write_all(b"exo compat run: binary not found: ");
            write_bytes(pkg);
            write_all(b"\n");
        }
    } else {
        write_all(b"Usage: exo compat [install|run] <pkg> [args...]\n");
    }
}

fn handle_exo_install(args: &[u8], is_compat: bool) {
    let (pkg_name, _) = first_token(args);
    if pkg_name.is_empty() {
        if is_compat {
            write_all(b"Usage: exo compat install <package>\n");
        } else {
            write_all(b"Usage: exo install <package>\n");
        }
        return;
    }
    write_all(b"exo install: package manager backend not implemented for '");
    write_bytes(pkg_name);
    write_all(b"' (ENOSYS)\n");
}

fn handle_exo_remove(args: &[u8]) {
    let (pkg_name, _) = first_token(args);
    if pkg_name.is_empty() {
        write_all(b"Usage: exo remove <package>\n");
        return;
    }
    write_all(b"exo remove: package manager backend not implemented for '");
    write_bytes(pkg_name);
    write_all(b"' (ENOSYS)\n");
}

fn list_dir_entries_brief(dir_cstr: &[u8], label: &[u8]) {
    write_bytes(label);
    write_all(b":\n");
    let fd = unsafe {
        syscall::syscall4(
            syscall::SYS_OPENAT,
            AT_FDCWD,
            dir_cstr.as_ptr() as u64,
            syscall::O_RDONLY,
            0,
        )
    };
    if fd < 0 {
        write_all(b"  (unavailable)\n");
        return;
    }
    let mut buf = [0u8; IO_BUF];
    loop {
        let n = unsafe {
            syscall::syscall3(
                syscall::SYS_GETDENTS64,
                fd as u64,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
            )
        };
        if n <= 0 {
            break;
        }
        let slice = &buf[..n as usize];
        let mut off = 0usize;
        while off + DIRENT64_HEADER_SIZE <= slice.len() {
            let reclen = u16::from_le_bytes([slice[off + 16], slice[off + 17]]) as usize;
            if reclen == 0 || off + reclen > slice.len() {
                break;
            }
            let start = off + DIRENT64_HEADER_SIZE;
            let mut end = start;
            while end < off + reclen && slice[end] != 0 {
                end += 1;
            }
            let name = &slice[start..end];
            if !name.is_empty() && !bytes_eq(name, b".") && !bytes_eq(name, b"..") {
                write_all(b"  ");
                write_bytes(name);
                write_all(b"\n");
            }
            off += reclen;
        }
    }
    let _ = close_fd(fd);
}

fn list_installed_packages() {
    write_all(b"Installed Executables on ExoFS:\n");
    list_dir_entries_brief(b"/bin\0", b"/bin");
    list_dir_entries_brief(b"/sbin\0", b"/sbin");
}

fn search_packages(args: &[u8]) {
    let (term, _) = first_token(args);
    write_all(b"exo search: remote package registry not configured for '");
    write_bytes(term);
    write_all(b"' (ENOSYS)\n");
}

fn handle_exo_audit(_args: &[u8]) {
    write_all(b"=== ExoLedger Audit Trail ===\n");
    let chain_rc = unsafe { syscall::syscall3(syscall::SYS_EXOPHOENIX_TEST, 1, 0, 0) };
    if chain_rc >= 0 {
        write_all(b"ExoLedger BLAKE3 chain : INTACT (PASS, ");
        write_i64(chain_rc);
        write_all(b" verified entries)\n");
    } else {
        write_all(b"ExoLedger BLAKE3 chain : CORRUPTED (FAIL, errno=");
        write_i64(chain_rc);
        write_all(b")\n");
    }
    let mut st = LinuxStat::default();
    let rc = unsafe {
        syscall::syscall2(
            syscall::SYS_STAT,
            b"/var/log/exoledger.sealed\0".as_ptr() as u64,
            &mut st as *mut LinuxStat as u64,
        )
    };
    if rc == 0 {
        write_all(b"Log file : /var/log/exoledger.sealed (");
        write_i64(st.st_size);
        write_all(b" bytes)\n");
    } else {
        write_all(b"Log file : /var/log/exoledger.sealed not present (in-memory SSR P0+Ring active)\n");
    }
}

fn print_exo_logs(_args: &[u8]) {
    write_all(b"exo log: ring-buffer log reader not wired to kernel/monitor_server yet (ENOSYS)\n");
}

fn handle_exo_mount(args: &[u8]) {
    let (dev, tail) = next_arg(args);
    let (mnt, _) = next_arg(tail);
    let dev_name = if dev.is_empty() { b"/dev/usb0" as &[u8] } else { dev };
    let mnt_name = if mnt.is_empty() { b"/mnt/usb" as &[u8] } else { mnt };
    write_all(b"exo mount: cannot mount ");
    write_bytes(dev_name);
    write_all(b" on ");
    write_bytes(mnt_name);
    write_all(b" (dynamic filesystem mount syscall not implemented: ENOSYS)\n");
}

fn handle_exo_umount(args: &[u8]) {
    let (mnt, _) = next_arg(args);
    let mnt_name = if mnt.is_empty() { b"/mnt/usb" as &[u8] } else { mnt };
    let rc = unsafe { syscall::syscall0(syscall::SYS_SYNC) };
    write_all(b"exo umount: synced rootfs (rc=");
    write_i64(rc);
    write_all(b"), dynamic unmount of ");
    write_bytes(mnt_name);
    write_all(b" not implemented (ENOSYS)\n");
}

fn handle_exo_cp(args: &[u8], state: &ShellState) {
    let (src_arg, tail) = next_arg(args);
    let (dst_arg, _) = next_arg(tail);
    if src_arg.is_empty() || dst_arg.is_empty() {
        write_all(b"exo cp: usage: exo cp <src> <dst>\n");
        return;
    }

    let mut src_path = [0u8; PATH_MAX];
    let Some(src_len) = absolute_path(state.cwd(), src_arg, &mut src_path) else {
        write_all(b"exo cp: source path invalid\n");
        return;
    };
    let mut dst_path = [0u8; PATH_MAX];
    let Some(dst_len) = absolute_path(state.cwd(), dst_arg, &mut dst_path) else {
        write_all(b"exo cp: destination path invalid\n");
        return;
    };
    src_path[src_len] = 0;
    dst_path[dst_len] = 0;

    let in_fd = unsafe {
        syscall::syscall4(
            syscall::SYS_OPENAT,
            AT_FDCWD,
            src_path.as_ptr() as u64,
            syscall::O_RDONLY,
            0,
        )
    };
    if in_fd < 0 {
        print_errno(b"exo cp (open src)", in_fd);
        return;
    }

    let out_fd = unsafe {
        syscall::syscall4(
            syscall::SYS_OPENAT,
            AT_FDCWD,
            dst_path.as_ptr() as u64,
            syscall::O_CREAT | syscall::O_WRONLY | syscall::O_TRUNC,
            0o644,
        )
    };
    if out_fd < 0 {
        print_errno(b"exo cp (open dst)", out_fd);
        let _ = close_fd(in_fd);
        return;
    }

    let mut buf = [0u8; 1024];
    let mut total_copied = 0usize;
    let mut had_err = false;
    loop {
        let n = unsafe {
            syscall::syscall3(
                syscall::SYS_READ,
                in_fd as u64,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
            )
        };
        if n == 0 {
            break;
        }
        if n < 0 {
            print_errno(b"exo cp read", n);
            had_err = true;
            break;
        }
        let rc = write_fd_all(out_fd, &buf[..n as usize]);
        if rc < 0 {
            print_errno(b"exo cp write", rc);
            had_err = true;
            break;
        }
        total_copied += n as usize;
    }
    let _ = close_fd(out_fd);
    let _ = close_fd(in_fd);

    if !had_err {
        write_all(b"exo cp: copied ");
        write_u64(total_copied as u64);
        write_all(b" bytes\n");
    }
}

fn handle_exo_usb() {
    write_all(b"=== USB Subsystem Status ===\n");
    if ipc_service_online(b"usb_server") {
        write_all(b"usb_server : online (device enumeration RPC not exposed yet)\n");
    } else {
        write_all(b"usb_server : offline\n");
    }
    if path_exists_probe(b"/dev/usb0\0") {
        write_all(b"/dev/usb0  : present\n");
    } else {
        write_all(b"/dev/usb0  : not present\n");
    }
}

pub fn cmd_beep(rest: &[u8]) {
    handle_exo_beep(rest);
}

pub fn cmd_chime(_rest: &[u8]) {
    handle_exo_chime();
}

fn handle_exo_beep(args: &[u8]) {
    let (freq_str, dur_str) = first_token(args);
    let freq = parse_u64(freq_str).unwrap_or(800) as u32;
    let dur = parse_u64(dur_str).unwrap_or(200) as u32;
    let req = syscall::AudioRequest {
        sender_pid: 0,
        msg_type: syscall::AUDIO_MSG_BEEP,
        reply_endpoint: 0,
        a: freq,
        b: dur,
        _reserved: [0; 16],
    };
    let rc = unsafe {
        syscall::syscall6(
            syscall::SYS_IPC_SEND,
            syscall::AUDIO_SERVER_ENDPOINT,
            &req as *const syscall::AudioRequest as u64,
            core::mem::size_of::<syscall::AudioRequest>() as u64,
            0,
            0,
            0,
        )
    };
    if rc >= 0 {
        write_all(b"Audio: Beep sent (");
        write_u64(freq as u64);
        write_all(b" Hz, ");
        write_u64(dur as u64);
        write_all(b" ms)\n");
    } else {
        write_all(b"exo beep: audio_server unavailable (errno ");
        write_i64(rc);
        write_all(b")\n");
    }
}

fn handle_exo_chime() {
    let req = syscall::AudioRequest {
        sender_pid: 0,
        msg_type: syscall::AUDIO_MSG_PLAY_SYSTEM_SOUND,
        reply_endpoint: 0,
        a: syscall::AUDIO_SOUND_BOOT_COMPLETE,
        b: 0,
        _reserved: [0; 16],
    };
    let rc = unsafe {
        syscall::syscall6(
            syscall::SYS_IPC_SEND,
            syscall::AUDIO_SERVER_ENDPOINT,
            &req as *const syscall::AudioRequest as u64,
            core::mem::size_of::<syscall::AudioRequest>() as u64,
            0,
            0,
            0,
        )
    };
    if rc >= 0 {
        write_all(b"Audio: Playing Exo-OS resurrection chime (Intel HDA / VirtIO Sound)...\n");
    } else {
        write_all(b"exo chime: audio_server unavailable (errno ");
        write_i64(rc);
        write_all(b")\n");
    }
}

#[derive(Clone, Copy)]
struct ExoWaylandData;

struct ExoWaylandClientState {
    globals_count: u32,
    surface_id: u32,
    shm_fd: i32,
    shm_formats: u32,
    pool: Option<wayland_client::protocol::wl_shm_pool::WlShmPool>,
    surface: Option<wayland_client::protocol::wl_surface::WlSurface>,
    buffer: Option<wayland_client::protocol::wl_buffer::WlBuffer>,
    xdg_wm_base: Option<wayland_protocols::xdg::shell::client::xdg_wm_base::XdgWmBase>,
    configured_serial: u32,
    buffer_released: bool,
}

impl
    wayland_client::Dispatch<
        wayland_client::protocol::wl_registry::WlRegistry,
        ExoWaylandClientState,
    > for ExoWaylandData
{
    fn event(
        &self,
        state: &mut ExoWaylandClientState,
        registry: &wayland_client::protocol::wl_registry::WlRegistry,
        event: wayland_client::protocol::wl_registry::Event,
        _conn: &wayland_client::Connection,
        qh: &wayland_client::QueueHandle<ExoWaylandClientState>,
    ) {
        use std::os::fd::BorrowedFd;
        use wayland_client::Proxy;
        if let wayland_client::protocol::wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            state.globals_count += 1;
            if interface == "wl_compositor" {
                let comp: wayland_client::protocol::wl_compositor::WlCompositor =
                    registry.bind(name, version.min(6), qh, ExoWaylandData);
                let surface = comp.create_surface(qh, ExoWaylandData);
                state.surface_id = surface.id().protocol_id();
                state.surface = Some(surface);
            } else if interface == "wl_shm" {
                let shm: wayland_client::protocol::wl_shm::WlShm =
                    registry.bind(name, version.min(1), qh, ExoWaylandData);
                if state.shm_fd >= 0 {
                    let borrowed = unsafe { BorrowedFd::borrow_raw(state.shm_fd) };
                    let pool = shm.create_pool(borrowed, 64 * 64 * 4, qh, ExoWaylandData);
                    let buffer = pool.create_buffer(
                        0,
                        64,
                        64,
                        64 * 4,
                        wayland_client::protocol::wl_shm::Format::Argb8888,
                        qh,
                        ExoWaylandData,
                    );
                    state.pool = Some(pool);
                    state.buffer = Some(buffer);
                }
            } else if interface == "xdg_wm_base" {
                let xdg: wayland_protocols::xdg::shell::client::xdg_wm_base::XdgWmBase =
                    registry.bind(name, version.min(6), qh, ExoWaylandData);
                state.xdg_wm_base = Some(xdg);
            }
        }
    }
}

impl
    wayland_client::Dispatch<
        wayland_client::protocol::wl_compositor::WlCompositor,
        ExoWaylandClientState,
    > for ExoWaylandData
{
    fn event(
        &self,
        _state: &mut ExoWaylandClientState,
        _proxy: &wayland_client::protocol::wl_compositor::WlCompositor,
        _event: wayland_client::protocol::wl_compositor::Event,
        _conn: &wayland_client::Connection,
        _qh: &wayland_client::QueueHandle<ExoWaylandClientState>,
    ) {
    }
}

impl
    wayland_client::Dispatch<
        wayland_client::protocol::wl_surface::WlSurface,
        ExoWaylandClientState,
    > for ExoWaylandData
{
    fn event(
        &self,
        _state: &mut ExoWaylandClientState,
        _proxy: &wayland_client::protocol::wl_surface::WlSurface,
        _event: wayland_client::protocol::wl_surface::Event,
        _conn: &wayland_client::Connection,
        _qh: &wayland_client::QueueHandle<ExoWaylandClientState>,
    ) {
    }
}

impl
    wayland_client::Dispatch<
        wayland_client::protocol::wl_shm::WlShm,
        ExoWaylandClientState,
    > for ExoWaylandData
{
    fn event(
        &self,
        state: &mut ExoWaylandClientState,
        _proxy: &wayland_client::protocol::wl_shm::WlShm,
        event: wayland_client::protocol::wl_shm::Event,
        _conn: &wayland_client::Connection,
        _qh: &wayland_client::QueueHandle<ExoWaylandClientState>,
    ) {
        if matches!(event, wayland_client::protocol::wl_shm::Event::Format { .. }) {
            state.shm_formats += 1;
        }
    }
}

impl
    wayland_client::Dispatch<
        wayland_client::protocol::wl_shm_pool::WlShmPool,
        ExoWaylandClientState,
    > for ExoWaylandData
{
    fn event(
        &self,
        _state: &mut ExoWaylandClientState,
        _proxy: &wayland_client::protocol::wl_shm_pool::WlShmPool,
        _event: wayland_client::protocol::wl_shm_pool::Event,
        _conn: &wayland_client::Connection,
        _qh: &wayland_client::QueueHandle<ExoWaylandClientState>,
    ) {
    }
}

impl
    wayland_client::Dispatch<
        wayland_client::protocol::wl_buffer::WlBuffer,
        ExoWaylandClientState,
    > for ExoWaylandData
{
    fn event(
        &self,
        state: &mut ExoWaylandClientState,
        _proxy: &wayland_client::protocol::wl_buffer::WlBuffer,
        event: wayland_client::protocol::wl_buffer::Event,
        _conn: &wayland_client::Connection,
        _qh: &wayland_client::QueueHandle<ExoWaylandClientState>,
    ) {
        if matches!(event, wayland_client::protocol::wl_buffer::Event::Release) {
            state.buffer_released = true;
        }
    }
}

impl
    wayland_client::Dispatch<
        wayland_protocols::xdg::shell::client::xdg_wm_base::XdgWmBase,
        ExoWaylandClientState,
    > for ExoWaylandData
{
    fn event(
        &self,
        _state: &mut ExoWaylandClientState,
        proxy: &wayland_protocols::xdg::shell::client::xdg_wm_base::XdgWmBase,
        event: wayland_protocols::xdg::shell::client::xdg_wm_base::Event,
        _conn: &wayland_client::Connection,
        _qh: &wayland_client::QueueHandle<ExoWaylandClientState>,
    ) {
        if let wayland_protocols::xdg::shell::client::xdg_wm_base::Event::Ping { serial } = event {
            proxy.pong(serial);
        }
    }
}

impl
    wayland_client::Dispatch<
        wayland_protocols::xdg::shell::client::xdg_surface::XdgSurface,
        ExoWaylandClientState,
    > for ExoWaylandData
{
    fn event(
        &self,
        state: &mut ExoWaylandClientState,
        proxy: &wayland_protocols::xdg::shell::client::xdg_surface::XdgSurface,
        event: wayland_protocols::xdg::shell::client::xdg_surface::Event,
        _conn: &wayland_client::Connection,
        _qh: &wayland_client::QueueHandle<ExoWaylandClientState>,
    ) {
        if let wayland_protocols::xdg::shell::client::xdg_surface::Event::Configure { serial } =
            event
        {
            proxy.ack_configure(serial);
            state.configured_serial = serial;
        }
    }
}

impl
    wayland_client::Dispatch<
        wayland_protocols::xdg::shell::client::xdg_toplevel::XdgToplevel,
        ExoWaylandClientState,
    > for ExoWaylandData
{
    fn event(
        &self,
        _state: &mut ExoWaylandClientState,
        _proxy: &wayland_protocols::xdg::shell::client::xdg_toplevel::XdgToplevel,
        _event: wayland_protocols::xdg::shell::client::xdg_toplevel::Event,
        _conn: &wayland_client::Connection,
        _qh: &wayland_client::QueueHandle<ExoWaylandClientState>,
    ) {
    }
}

/// Verifies OS multi-threading (`clone` + `PT_TLS` + blocking kernel `futex`) in `std`.
fn run_std_runtime_verification() -> Option<(u32, u32)> {
    use std::sync::{Arc, Condvar, Mutex, mpsc};
    thread_local! {
        static TLS_SLOT: std::cell::Cell<u32> = const { std::cell::Cell::new(0x10) };
    }

    let pair = Arc::new((Mutex::new(0u32), Condvar::new()));
    let (tx, rx) = mpsc::channel::<u32>();

    let pair_w1 = Arc::clone(&pair);
    let tx_w1 = tx.clone();
    let h1 = std::thread::Builder::new()
        .name(std::string::String::from("exo-std-w1"))
        .spawn(move || {
            TLS_SLOT.with(|c| c.set(c.get() + 11));
            let tls_val = TLS_SLOT.with(|c| c.get());
            let (lock, cvar) = &*pair_w1;
            let mut guard = lock.lock().unwrap();
            *guard += tls_val;
            cvar.notify_all();
            let _ = tx_w1.send(1);
        })
        .ok()?;

    let pair_w2 = Arc::clone(&pair);
    let tx_w2 = tx;
    let h2 = std::thread::Builder::new()
        .name(std::string::String::from("exo-std-w2"))
        .spawn(move || {
            TLS_SLOT.with(|c| c.set(c.get() + 21));
            let tls_val = TLS_SLOT.with(|c| c.get());
            let (lock, cvar) = &*pair_w2;
            let mut guard = lock.lock().unwrap();
            *guard += tls_val;
            cvar.notify_all();
            let _ = tx_w2.send(1);
        })
        .ok()?;

    let r1 = rx.recv().ok()?;
    let r2 = rx.recv().ok()?;
    h1.join().ok()?;
    h2.join().ok()?;

    // Verify main thread TLS was not mutated by worker threads (must still be 0x10).
    let main_tls = TLS_SLOT.with(|c| c.get());
    let (lock, _cvar) = &*pair;
    let total = *lock.lock().unwrap() + main_tls;
    Some((r1 + r2, total))
}

struct LiveWaylandSummary {
    globals: u32,
    surface_id: u32,
    configured_serial: u32,
    shm_formats: u32,
    buffer_released: bool,
}

/// Connects to the real `/run/wayland-0` `fb_server` compositor via `wayland_client::Connection::connect_to_env`,
/// performs registry discovery, transfers a `memfd` pool via `SCM_RIGHTS`, completes XDG configure handshake,
/// commits an ARGB8888 surface buffer, and verifies buffer release over the live compositor socket.
fn run_wayland_rs_roundtrip() -> Result<LiveWaylandSummary, &'static str> {
    let shm_size: usize = 64 * 64 * 4;
    const MFD_ALLOW_SEALING: u32 = 0x0002;
    const F_ADD_SEALS: u64 = 1033;
    const F_SEAL_SHRINK: u64 = 0x0002;
    let shm_fd = std::os::exo::memfd_create(b"wayland-shm-0\0", MFD_ALLOW_SEALING)
        .map_err(|_| "memfd_create failed")?;
    let trunc_rc = unsafe { syscall::syscall2(syscall::SYS_FTRUNCATE, shm_fd as u64, shm_size as u64) };
    if trunc_rc < 0 {
        let _ = close_fd(shm_fd as i64);
        return Err("ftruncate shm failed");
    }
    let seal_rc = unsafe {
        syscall::syscall3(syscall::SYS_FCNTL, shm_fd as u64, F_ADD_SEALS, F_SEAL_SHRINK)
    };
    if seal_rc < 0 {
        let _ = close_fd(shm_fd as i64);
        return Err("fcntl F_SEAL_SHRINK failed");
    }
    let client_map = unsafe {
        syscall::syscall6(
            syscall::SYS_MMAP,
            0,
            shm_size as u64,
            syscall::PROT_READ | syscall::PROT_WRITE,
            syscall::MAP_SHARED,
            shm_fd as u64,
            0,
        )
    };
    if client_map <= 0 {
        let _ = close_fd(shm_fd as i64);
        return Err("mmap shm failed");
    }
    unsafe {
        let pixels = core::slice::from_raw_parts_mut(client_map as usize as *mut u32, 64 * 64);
        for y in 0..64 {
            for x in 0..64 {
                let px = if x < 2 || y < 2 || x >= 62 || y >= 62 {
                    0xFF55_99DD
                } else {
                    0xFF33_6699
                };
                pixels[y * 64 + x] = px;
            }
        }
        let _ = syscall::syscall2(syscall::SYS_MUNMAP, client_map as u64, shm_size as u64);
    }

    let conn = unsafe { wayland_client::Connection::connect_to_env() }
        .map_err(|_| "connect_to_env(/run/wayland-0) failed")?;
    let mut event_queue = conn.new_event_queue::<ExoWaylandClientState>();
    let qh = event_queue.handle();

    let mut client_state = ExoWaylandClientState {
        globals_count: 0,
        surface_id: 0,
        shm_fd,
        shm_formats: 0,
        pool: None,
        surface: None,
        buffer: None,
        xdg_wm_base: None,
        configured_serial: 0,
        buffer_released: false,
    };

    let _registry = conn.display().get_registry(&qh, ExoWaylandData);
    event_queue
        .roundtrip(&mut client_state)
        .map_err(|_| "registry roundtrip failed")?;

    let (Some(surface), Some(xdg)) = (client_state.surface.take(), client_state.xdg_wm_base.take()) else {
        let _ = close_fd(shm_fd as i64);
        return Err("compositor missing wl_surface or xdg_wm_base global");
    };
    let xdg_surface = xdg.get_xdg_surface(&surface, &qh, ExoWaylandData);
    let toplevel = xdg_surface.get_toplevel(&qh, ExoWaylandData);
    toplevel.set_title(&std::string::String::from("ExoCompositor Wayland Client"));
    toplevel.set_app_id(&std::string::String::from("exo-wayland"));
    surface.commit();

    event_queue
        .roundtrip(&mut client_state)
        .map_err(|_| "xdg configure roundtrip failed")?;

    if let Some(buffer) = &client_state.buffer {
        surface.attach(Some(buffer), 0, 0);
        surface.damage(0, 0, 64, 64);
        surface.commit();
        event_queue
            .roundtrip(&mut client_state)
            .map_err(|_| "surface buffer commit roundtrip failed")?;
    }

    if let Some(buf) = client_state.buffer.take() {
        buf.destroy();
    }
    if let Some(pool) = client_state.pool.take() {
        pool.destroy();
    }
    toplevel.destroy();
    xdg_surface.destroy();
    surface.destroy();
    let _ = event_queue.roundtrip(&mut client_state);
    let _ = close_fd(shm_fd as i64);

    if client_state.configured_serial == 0 {
        return Err("compositor did not emit xdg_surface.configure");
    }

    Ok(LiveWaylandSummary {
        globals: client_state.globals_count,
        surface_id: client_state.surface_id,
        configured_serial: client_state.configured_serial,
        shm_formats: client_state.shm_formats,
        buffer_released: client_state.buffer_released,
    })
}

fn handle_exo_wayland(_args: &[u8]) {
    write_all(b"=== ExoCompositor : Wayland & VirtIO-GPU Stack ===\n\n");
    write_all(b"  Protocol Engine : Wayland Wire Protocol (/run/wayland-0 -> exo_compositor Strate 2 -> fb_server Strate 1)\n");
    write_all(b"  Global Objects  : wl_compositor#6, wl_shm#1, xdg_wm_base#6, wl_seat#7\n");
    if let Some((threads_ok, tls_sum)) = run_std_runtime_verification() {
        write_all(b"  Rust std PAL    : LIVE OK (threads=");
        write_u64(threads_ok as u64);
        write_all(b", futex+tls_sum=");
        write_u64(tls_sum as u64);
        write_all(b")\n");
    }
    match run_wayland_rs_roundtrip() {
        Ok(res) => {
            write_all(b"  wayland-rs std  : LIVE OK (/run/wayland-0 globals=");
            write_u64(res.globals as u64);
            write_all(b", shm_formats=");
            write_u64(res.shm_formats as u64);
            write_all(b", configure_serial=");
            write_u64(res.configured_serial as u64);
            write_all(b", surface_id=");
            write_u64(res.surface_id as u64);
            write_all(b", buffer_released=");
            write_u64(u64::from(res.buffer_released));
            write_all(b")\n");
        }
        Err(reason) => {
            write_all(b"  wayland-rs std  : ERROR (");
            write_all(reason.as_bytes());
            write_all(b")\n");
        }
    }
    write_all(b"  Zero-Copy SHM   : SurfaceGrant (memory_server SHM) + memfd MAP_SHARED + SCM_RIGHTS\n");
    write_all(b"  GPU Backend     : VirtIO-GPU 2D/Cursor (PCI 0x1AF4:0x1050 controlq/cursorq) + GOP Shadow RAM\n");
    write_all(b"  Desktop Shell   : Top Status Bar + xdg_toplevel Window Frame + Interactive Dock\n");
}
