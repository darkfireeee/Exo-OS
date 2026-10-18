//! # servers/exosh
#![allow(dead_code, unused_imports)]
use core::cell::UnsafeCell;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use exo_syscall_abi as syscall;

use super::*;
use crate::types::*;
use crate::util::*;
use crate::terminal::*;
use crate::exec::*;

pub fn cmd_ping(rest: &[u8]) {
    let (host, tail) = next_arg(rest);
    if host.is_empty() {
        write_all(b"ping: usage: ping <ipv4> [count]\n");
        return;
    }
    let Some(addr) = parse_ipv4(host) else {
        write_all(b"ping: invalid IPv4 address\n");
        return;
    };
    let (count_arg, _) = next_arg(tail);
    let count = if count_arg.is_empty() {
        4
    } else {
        parse_u64(count_arg).unwrap_or(4).min(32)
    };

    let mut fd = -1i64;
    let mut attempts = 0usize;
    while attempts < 3 {
        fd = unsafe {
            syscall::syscall3(syscall::SYS_SOCKET, AF_INET as u64, SOCK_RAW, IPPROTO_ICMP)
        };
        if fd >= 0 {
            break;
        }
        sleep_ms(50);
        attempts += 1;
    }
    if fd < 0 {
        print_errno(b"ping socket", fd);
        return;
    }

    let bind_addr = sockaddr_in([0, 0, 0, 0], PING_IDENT);
    let bind_rc = unsafe {
        syscall::syscall3(
            syscall::SYS_BIND,
            fd as u64,
            bind_addr.as_ptr() as u64,
            bind_addr.len() as u64,
        )
    };
    if bind_rc < 0 {
        print_errno(b"ping bind", bind_rc);
        let _ = close_fd(fd);
        return;
    }

    let sockaddr = sockaddr_in(addr, 0);
    let rc = unsafe {
        syscall::syscall3(
            syscall::SYS_CONNECT,
            fd as u64,
            sockaddr.as_ptr() as u64,
            sockaddr.len() as u64,
        )
    };
    if rc < 0 {
        print_errno(b"ping connect", rc);
        let _ = close_fd(fd);
        return;
    }

    write_all(b"PING ");
    write_bytes(host);
    write_all(b" via ExoNet V4\n");

    let mut seq = 0u64;
    while seq < count {
        let Some(start) = monotonic_ns() else {
            write_all(b"ping: clock unavailable\n");
            break;
        };

        let packet = icmp_echo_request(seq as u16);
        let send_rc = unsafe {
            syscall::syscall6(
                syscall::SYS_SENDTO,
                fd as u64,
                packet.as_ptr() as u64,
                packet.len() as u64,
                0,
                0,
                0,
            )
        };
        if send_rc < 0 {
            print_errno(b"ping sendto", send_rc);
            break;
        }

        let mut recv_buf = [0u8; 64];
        let mut from = [0u8; 16];
        let mut recv_rc = EAGAIN;
        let mut attempts = 0usize;
        while attempts < PING_RECV_ATTEMPTS {
            let mut from_len = from.len() as u32;
            recv_rc = unsafe {
                syscall::syscall6(
                    syscall::SYS_RECVFROM,
                    fd as u64,
                    recv_buf.as_mut_ptr() as u64,
                    recv_buf.len() as u64,
                    0,
                    from.as_mut_ptr() as u64,
                    &mut from_len as *mut u32 as u64,
                )
            };
            if recv_rc != EAGAIN {
                break;
            }
            sleep_ms(10);
            attempts += 1;
        }
        let elapsed = monotonic_ns().unwrap_or(start).saturating_sub(start);
        if recv_rc == EAGAIN {
            write_all(b"ping: timeout waiting for ICMP echo reply\n");
            break;
        }
        if recv_rc < 0 {
            print_errno(b"ping recvfrom", recv_rc);
            break;
        }
        if !icmp_echo_reply_matches(&recv_buf[..recv_rc as usize], seq as u16) {
            write_all(b"ping: unexpected ICMP packet\n");
            break;
        }

        write_u64(recv_rc as u64);
        write_all(b" bytes from ");
        write_ipv4(addr);
        write_all(b": seq=");
        write_u64(seq);
        write_all(b" time=");
        write_u64(elapsed / 1_000);
        write_all(b"us\n");

        seq = seq.saturating_add(1);
        if seq < count {
            sleep_ms(250);
        }
    }

    let _ = close_fd(fd);
}

pub fn icmp_echo_request(seq: u16) -> [u8; PING_PACKET_LEN] {
    let mut packet = [0u8; PING_PACKET_LEN];
    packet[0] = 8;
    packet[4..6].copy_from_slice(&PING_IDENT.to_be_bytes());
    packet[6..8].copy_from_slice(&seq.to_be_bytes());
    packet[ICMP_ECHO_HEADER_LEN..].copy_from_slice(PING_PAYLOAD);
    let checksum = internet_checksum(&packet);
    packet[2..4].copy_from_slice(&checksum.to_be_bytes());
    packet
}

pub fn icmp_echo_reply_matches(packet: &[u8], seq: u16) -> bool {
    packet.len() >= ICMP_ECHO_HEADER_LEN
        && packet[0] == 0
        && packet[1] == 0
        && u16::from_be_bytes([packet[4], packet[5]]) == PING_IDENT
        && u16::from_be_bytes([packet[6], packet[7]]) == seq
        && internet_checksum(packet) == 0
}

pub fn internet_checksum(bytes: &[u8]) -> u16 {
    let mut sum = 0u32;
    let mut idx = 0usize;
    while idx + 1 < bytes.len() {
        sum = sum.wrapping_add(u16::from_be_bytes([bytes[idx], bytes[idx + 1]]) as u32);
        idx += 2;
    }
    if idx < bytes.len() {
        sum = sum.wrapping_add((bytes[idx] as u32) << 8);
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

pub fn cmd_tcping(rest: &[u8]) {
    let (host, tail) = next_arg(rest);
    if host.is_empty() {
        write_all(b"tcping: usage: tcping <ipv4> [port]\n");
        return;
    }
    let Some(addr) = parse_ipv4(host) else {
        write_all(b"tcping: invalid IPv4 address\n");
        return;
    };
    let (port_arg, _) = next_arg(tail);
    let port = if port_arg.is_empty() {
        80
    } else {
        let Some(parsed) = parse_u64(port_arg) else {
            write_all(b"tcping: invalid port\n");
            return;
        };
        if parsed == 0 || parsed > u16::MAX as u64 {
            write_all(b"tcping: invalid port\n");
            return;
        }
        parsed as u16
    };

    let mut fd = -1i64;
    let mut attempts = 0usize;
    while attempts < 3 {
        fd = unsafe { syscall::syscall3(syscall::SYS_SOCKET, AF_INET as u64, SOCK_STREAM, 0) };
        if fd >= 0 {
            break;
        }
        sleep_ms(50);
        attempts += 1;
    }
    if fd < 0 {
        print_errno(b"tcping socket", fd);
        return;
    }

    let sockaddr = sockaddr_in(addr, port);
    let Some(start) = monotonic_ns() else {
        write_all(b"tcping: clock unavailable\n");
        let _ = close_fd(fd);
        return;
    };
    let rc = unsafe {
        syscall::syscall3(
            syscall::SYS_CONNECT,
            fd as u64,
            sockaddr.as_ptr() as u64,
            sockaddr.len() as u64,
        )
    };
    let elapsed = monotonic_ns().unwrap_or(start).saturating_sub(start);
    if rc < 0 {
        write_all(b"tcping ");
        write_ipv4(addr);
        write_all(b":");
        write_u64(port as u64);
        write_all(b" failed after ");
        write_u64(elapsed / 1_000);
        write_all(b"us");
        print_errno(b" connect", rc);
        let _ = close_fd(fd);
        return;
    }

    write_all(b"tcping ");
    write_ipv4(addr);
    write_all(b":");
    write_u64(port as u64);
    write_all(b" connected time=");
    write_u64(elapsed / 1_000);
    write_all(b"us\n");

    let _ = close_fd(fd);
}

