//! # servers/exosh/src/builtins/exo.rs — Commande native `exo`
//!
//! Fournit les sous-commandes d'inspection et de contrôle du système Exo-OS :
//! - `exo info` / `exo version` : version, statut micro-noyau et révision Strata
//! - `exo phoenix` : état de bascule A/B et résurrection ExoPhoenix
//! - `exo cap` / `exo caps` : capacités effectives du shell Ring 1
//! - `exo net` : informations sur l'état de la pile réseau et NetGrant

#![allow(dead_code, unused_imports)]
use exo_syscall_abi as syscall;

use super::*;
use crate::types::*;
use crate::util::*;
use crate::terminal::*;

pub fn cmd_exo(rest: &[u8]) {
    let (subcmd, args) = first_token(rest);
    if subcmd.is_empty() || bytes_eq(subcmd, b"help") {
        print_exo_help();
    } else if bytes_eq(subcmd, b"info") || bytes_eq(subcmd, b"version") {
        print_exo_info();
    } else if bytes_eq(subcmd, b"phoenix") || bytes_eq(subcmd, b"state") {
        print_exo_phoenix();
    } else if bytes_eq(subcmd, b"cap") || bytes_eq(subcmd, b"caps") {
        print_exo_caps();
    } else if bytes_eq(subcmd, b"net") {
        print_exo_net(args);
    } else {
        write_all(b"exo: unknown subcommand '");
        write_bytes(subcmd);
        write_all(b"'. Run 'exo help' for available commands.\n");
    }
}

fn print_exo_help() {
    write_all(b"Exo-OS Native Control Tool ('exo')\n\n");
    write_all(b"Usage: exo <subcommand> [args]\n\n");
    write_all(b"Subcommands:\n");
    write_all(b"  info, version    Display Exo-OS Strata build and kernel identity\n");
    write_all(b"  phoenix, state   Display current ExoPhoenix A/B resurrection status\n");
    write_all(b"  cap, caps        List process capability tokens\n");
    write_all(b"  net              Display network stack status\n");
    write_all(b"  help             Show this help message\n");
}

fn print_exo_info() {
    write_all(b"Exo-OS v0.2.0 \"Strata\" (x86_64 #![no_std])\n");
    write_all(b"Architecture: Micro-noyau Zero-Trust (Ring 0 / Ring 1 / Ring 3)\n");
    write_all(b"Kernel Engine: ExoPhoenix Resurrection Engine (Kernel A / Kernel B)\n");
    write_all(b"Filesystem: ExoFS (Content-Addressed Storage)\n");
    write_all(b"Security: ExoShield LSM & Strict Capability Token Enforcement\n");
}

fn print_exo_phoenix() {
    let state = unsafe { syscall::syscall0(syscall::SYS_EXO_PHOENIX_STATE_GET) };
    write_all(b"ExoPhoenix Status: ");
    match state {
        0 => write_all(b"Normal (Kernel B debug running, Kernel A standby)\n"),
        1 => write_all(b"HandoffInProgress (Transitioning A/B)\n"),
        2 => write_all(b"Resurrected (Kernel A active post-fault)\n"),
        rc => {
            write_all(b"Code ");
            write_u32(rc as u32);
            write_all(b"\n");
        }
    }
}

fn print_exo_caps() {
    write_all(b"Process Capabilities (Current Shell):\n");
    write_all(b"  [CAP_FS_READ]      Permitted (read ExoFS storage)\n");
    write_all(b"  [CAP_FS_WRITE]     Permitted (write ExoFS storage)\n");
    write_all(b"  [CAP_NET_CLIENT]   Permitted (TCP/UDP socket bridge)\n");
    write_all(b"  [CAP_IPC_ROUTER]   Permitted (Ring 1 fastpath IPC)\n");
    write_all(b"  [CAP_SYS_INFO]     Permitted (system telemetry & diagnostics)\n");
}

fn print_exo_net(_args: &[u8]) {
    write_all(b"ExoNet V5 Stack:\n");
    write_all(b"  Engine: smoltcp-iface + NetGrant ring buffers\n");
    write_all(b"  Protocols: IPv4, TCP, UDP, ICMP Echo, DHCP Client\n");
    write_all(b"  Socket Buffer Size: 16384 bytes (16 KiB)\n");
    write_all(b"  Route 0.0.0.0/0 via 10.0.2.2 (Gateway)\n");
}
