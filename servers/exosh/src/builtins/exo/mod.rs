//! Native `exo` shell command dispatcher (modularized).
#![allow(dead_code, unused_imports)]

mod calendar;
mod doctor_metrics;
mod media_usb;
mod phoenix_audit;
mod pkg_compat;
mod wayland_probe;

pub use calendar::cmd_calendar;
pub use media_usb::{cmd_beep, cmd_chime};

use doctor_metrics::{print_exo_metrics, run_exo_doctor};
use media_usb::{handle_exo_beep, handle_exo_chime, handle_exo_usb};
use phoenix_audit::{
    handle_exo_audit, handle_exo_cp, handle_exo_mount, handle_exo_phoenix, handle_exo_umount,
    print_exo_caps, print_exo_logs, print_exo_net,
};
use pkg_compat::{
    handle_exo_compat, handle_exo_install, handle_exo_remove, list_installed_packages,
    search_packages,
};
use wayland_probe::handle_exo_wayland;
use crate::builtins::*;
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

