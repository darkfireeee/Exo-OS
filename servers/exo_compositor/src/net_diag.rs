//! Real ExoNet network diagnostics: UDP DNS resolver, raw ICMP ping, and external TCP reachability probe.

use crate::bench::now_ns;

#[cfg(target_os = "exo")]
use exo_syscall_abi as syscall;

#[derive(Clone, Debug, Default)]
pub struct NetDiagReport {
    pub local_ip: String,
    pub gateway_ip: String,
    pub dns_ip: String,
    pub loopback_ok: bool,
    pub loopback_ms: String,
    pub gateway_ok: bool,
    pub gateway_ms: String,
    pub dns_ok: bool,
    pub dns_detail: String,
    pub external_ok: bool,
    pub external_detail: String,
    pub summary: String,
}

/// Parse an IPv4 dotted-quad string (`"1.1.1.1"`).
pub fn parse_ipv4(s: &str) -> Option<[u8; 4]> {
    let mut parts = s.trim().split('.');
    let a: u8 = parts.next()?.parse().ok()?;
    let b: u8 = parts.next()?.parse().ok()?;
    let c: u8 = parts.next()?.parse().ok()?;
    let d: u8 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some([a, b, c, d])
}

pub fn fmt_ipv4(ip: [u8; 4]) -> String {
    format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
}

pub fn fmt_ms(us: u64) -> String {
    format!("{}.{} ms", us / 1000, (us % 1000) / 100)
}

/// Read configured DNS server from `/etc/resolv.conf` (fallback: QEMU Slirp `10.0.2.3`).
pub fn read_dns_server() -> [u8; 4] {
    if let Ok(txt) = std::fs::read_to_string("/etc/resolv.conf") {
        for line in txt.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("nameserver") {
                if let Some(ip) = parse_ipv4(rest.trim()) {
                    return ip;
                }
            }
        }
    }
    [10, 0, 2, 3]
}

fn icmp_checksum(data: &[u8]) -> u16 {
    let mut sum = 0u32;
    let mut i = 0usize;
    while i + 1 < data.len() {
        sum = sum.wrapping_add(u16::from_be_bytes([data[i], data[i + 1]]) as u32);
        i += 2;
    }
    if i < data.len() {
        sum = sum.wrapping_add((data[i] as u32) << 8);
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

fn build_dns_query(host: &str, txid: u16) -> Vec<u8> {
    let mut pkt = Vec::with_capacity(64);
    pkt.extend_from_slice(&txid.to_be_bytes());
    pkt.extend_from_slice(&0x0100u16.to_be_bytes()); // Standard recursive query
    pkt.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT = 1
    pkt.extend_from_slice(&0u16.to_be_bytes()); // ANCOUNT
    pkt.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
    pkt.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT
    for label in host.trim_end_matches('.').split('.') {
        let b = label.as_bytes();
        if b.is_empty() || b.len() > 63 {
            continue;
        }
        pkt.push(b.len() as u8);
        pkt.extend_from_slice(b);
    }
    pkt.push(0);
    pkt.extend_from_slice(&1u16.to_be_bytes()); // QTYPE = A
    pkt.extend_from_slice(&1u16.to_be_bytes()); // QCLASS = IN
    pkt
}

fn parse_dns_a_response(resp: &[u8], expected_txid: u16) -> Option<[u8; 4]> {
    if resp.len() < 12 {
        return None;
    }
    if u16::from_be_bytes([resp[0], resp[1]]) != expected_txid {
        return None;
    }
    let ancount = u16::from_be_bytes([resp[6], resp[7]]);
    if ancount == 0 {
        return None;
    }
    let mut pos = 12usize;
    // Skip question section
    while pos < resp.len() {
        let len = resp[pos];
        if len == 0 {
            pos += 1;
            break;
        }
        if (len & 0xC0) == 0xC0 {
            pos += 2;
            break;
        }
        pos += 1 + len as usize;
    }
    pos += 4; // QTYPE + QCLASS
    // Walk answer records
    for _ in 0..ancount {
        if pos >= resp.len() {
            break;
        }
        if (resp[pos] & 0xC0) == 0xC0 {
            pos += 2;
        } else {
            while pos < resp.len() && resp[pos] != 0 {
                pos += 1 + resp[pos] as usize;
            }
            pos += 1;
        }
        if pos + 10 > resp.len() {
            break;
        }
        let rtype = u16::from_be_bytes([resp[pos], resp[pos + 1]]);
        let rdlen = u16::from_be_bytes([resp[pos + 8], resp[pos + 9]]) as usize;
        pos += 10;
        if rtype == 1 && rdlen == 4 && pos + 4 <= resp.len() {
            return Some([resp[pos], resp[pos + 1], resp[pos + 2], resp[pos + 3]]);
        }
        pos += rdlen;
    }
    None
}

#[cfg(target_os = "exo")]
const MSG_DONTWAIT: u64 = 0x40;

#[cfg(target_os = "exo")]
fn make_sockaddr_in(ip: [u8; 4], port: u16) -> [u8; 16] {
    let mut raw = [0u8; 16];
    raw[0..2].copy_from_slice(&2u16.to_ne_bytes()); // AF_INET = 2
    raw[2..4].copy_from_slice(&port.to_be_bytes());
    raw[4..8].copy_from_slice(&ip);
    raw
}

#[cfg(target_os = "exo")]
pub fn resolve_dns_a(host: &str) -> Result<([u8; 4], [u8; 4], u64), String> {
    if let Some(ip) = parse_ipv4(host) {
        return Ok((ip, ip, 0));
    }
    if host.eq_ignore_ascii_case("localhost") {
        return Ok(([127, 0, 0, 1], [127, 0, 0, 1], 0));
    }
    let primary_dns = read_dns_server();
    let servers = [primary_dns, [1, 1, 1, 1]];
    let txid = ((now_ns() >> 8) as u16) ^ 0x4558;
    let query = build_dns_query(host, txid);

    for dns_ip in servers {
        // AF_INET=2, SOCK_DGRAM=2 | O_NONBLOCK=0x800
        let fd = unsafe { syscall::syscall3(syscall::SYS_SOCKET, 2, 2 | 0x800, 0) };
        if fd < 0 {
            continue;
        }
        let addr = make_sockaddr_in(dns_ip, 53);
        let t0 = now_ns();
        let sent = unsafe {
            syscall::syscall6(
                syscall::SYS_SENDTO,
                fd as u64,
                query.as_ptr() as u64,
                query.len() as u64,
                MSG_DONTWAIT,
                addr.as_ptr() as u64,
                16,
            )
        };
        if sent < 0 {
            unsafe { let _ = syscall::syscall1(syscall::SYS_CLOSE, fd as u64); }
            continue;
        }
        let mut resp = [0u8; 512];
        let deadline = t0 + 600_000_000;
        let mut found = None;
        while now_ns() < deadline {
            let n = unsafe {
                syscall::syscall6(
                    syscall::SYS_RECVFROM,
                    fd as u64,
                    resp.as_mut_ptr() as u64,
                    resp.len() as u64,
                    MSG_DONTWAIT,
                    0,
                    0,
                )
            };
            if n >= 12 {
                if let Some(ip) = parse_dns_a_response(&resp[..n as usize], txid) {
                    found = Some((ip, dns_ip, now_ns().saturating_sub(t0) / 1000));
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(8));
        }
        unsafe { let _ = syscall::syscall1(syscall::SYS_CLOSE, fd as u64); }
        if let Some(res) = found {
            return Ok(res);
        }
    }
    Err(format!("DNS : impossible de résoudre « {} »", host))
}

#[cfg(not(target_os = "exo"))]
pub fn resolve_dns_a(host: &str) -> Result<([u8; 4], [u8; 4], u64), String> {
    if let Some(ip) = parse_ipv4(host) {
        return Ok((ip, ip, 0));
    }
    if host.eq_ignore_ascii_case("localhost") {
        return Ok(([127, 0, 0, 1], [127, 0, 0, 1], 0));
    }
    let _ = (build_dns_query, parse_dns_a_response);
    Err(format!("DNS hôte simulé : {}", host))
}

/// Send one real ICMP Echo Request via `AF_INET / SOCK_RAW / IPPROTO_ICMP`.
#[cfg(target_os = "exo")]
pub fn icmp_echo_once(ip: [u8; 4], ident: u16, seq: u16, timeout_ms: u64) -> Result<(u64, u8), i64> {
    // AF_INET=2, SOCK_RAW=3 | O_NONBLOCK=0x800, IPPROTO_ICMP=1
    let fd = unsafe { syscall::syscall3(syscall::SYS_SOCKET, 2, 3 | 0x800, 1) };
    if fd < 0 {
        return Err(fd);
    }
    let mut pkt = [0u8; 64];
    pkt[0] = 8; // ICMP Echo Request
    pkt[1] = 0;
    pkt[4..6].copy_from_slice(&ident.to_be_bytes());
    pkt[6..8].copy_from_slice(&seq.to_be_bytes());
    let tag = b"ExoOS-ExoNet-ICMP-Echo-64B";
    pkt[8..8 + tag.len()].copy_from_slice(tag);
    let csum = icmp_checksum(&pkt);
    pkt[2..4].copy_from_slice(&csum.to_be_bytes());

    let addr = make_sockaddr_in(ip, 0);
    let t0 = now_ns();
    let sent = unsafe {
        syscall::syscall6(
            syscall::SYS_SENDTO,
            fd as u64,
            pkt.as_ptr() as u64,
            pkt.len() as u64,
            MSG_DONTWAIT,
            addr.as_ptr() as u64,
            16,
        )
    };
    if sent < 0 {
        unsafe { let _ = syscall::syscall1(syscall::SYS_CLOSE, fd as u64); }
        return Err(sent);
    }

    let mut rx = [0u8; 256];
    let deadline = t0 + timeout_ms.saturating_mul(1_000_000);
    while now_ns() < deadline {
        let n = unsafe {
            syscall::syscall6(
                syscall::SYS_RECVFROM,
                fd as u64,
                rx.as_mut_ptr() as u64,
                rx.len() as u64,
                MSG_DONTWAIT,
                0,
                0,
            )
        };
        if n >= 8 {
            let len = n as usize;
            let (icmp_off, ttl) = if rx[0] == 0 {
                (0usize, 64u8)
            } else if (rx[0] >> 4) == 4 && len >= 28 {
                let ihl = ((rx[0] & 0x0F) as usize) * 4;
                (ihl, rx[8])
            } else {
                (len, 64)
            };
            if icmp_off + 8 <= len && rx[icmp_off] == 0 {
                let r_id = u16::from_be_bytes([rx[icmp_off + 4], rx[icmp_off + 5]]);
                let r_seq = u16::from_be_bytes([rx[icmp_off + 6], rx[icmp_off + 7]]);
                if r_id == ident && r_seq == seq {
                    let rtt = now_ns().saturating_sub(t0) / 1000;
                    unsafe { let _ = syscall::syscall1(syscall::SYS_CLOSE, fd as u64); }
                    return Ok((rtt.max(1), ttl));
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(6));
    }
    unsafe { let _ = syscall::syscall1(syscall::SYS_CLOSE, fd as u64); }
    Err(syscall::ETIMEDOUT)
}

#[cfg(not(target_os = "exo"))]
pub fn icmp_echo_once(ip: [u8; 4], _ident: u16, _seq: u16, _timeout_ms: u64) -> Result<(u64, u8), i64> {
    let _ = icmp_checksum(&[8, 0, 0, 0]);
    if ip[0] == 127 {
        Ok((250, 64))
    } else {
        Err(-110)
    }
}

/// Real external TCP reachability probe (`SYS_SOCKET` + `SYS_CONNECT`) used when QEMU Slirp
/// drops unprivileged host ICMP (`ping_group_range = 1 0`) while TCP/UDP Internet works.
#[cfg(target_os = "exo")]
pub fn tcp_probe_once(ip: [u8; 4], port: u16, timeout_ms: u64) -> Result<u64, i64> {
    let fd = unsafe { syscall::syscall3(syscall::SYS_SOCKET, 2, 1 | 0x800, 0) };
    if fd < 0 {
        return Err(fd);
    }
    // Ensure O_NONBLOCK (0x800) via F_SETFL (4) so SYS_CONNECT never blocks 10 s inside kernel
    unsafe {
        let _ = syscall::syscall3(syscall::SYS_FCNTL, fd as u64, 4, 0x800);
    }
    let addr = make_sockaddr_in(ip, port);
    let t0 = now_ns();
    let deadline = t0 + timeout_ms.saturating_mul(1_000_000);
    let mut rc = unsafe { syscall::syscall3(syscall::SYS_CONNECT, fd as u64, addr.as_ptr() as u64, 16) };
    // EINPROGRESS = -115, EAGAIN = -11, EALREADY = -114, EISCONN = -106
    while (rc == -115 || rc == -11 || rc == -114) && now_ns() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(8));
        rc = unsafe { syscall::syscall3(syscall::SYS_CONNECT, fd as u64, addr.as_ptr() as u64, 16) };
        if rc == 0 || rc == -106 {
            let rtt = now_ns().saturating_sub(t0) / 1000;
            unsafe { let _ = syscall::syscall1(syscall::SYS_CLOSE, fd as u64); }
            return Ok(rtt.max(1));
        }
    }
    unsafe { let _ = syscall::syscall1(syscall::SYS_CLOSE, fd as u64); }
    if rc == 0 || rc == -106 {
        Ok((now_ns().saturating_sub(t0) / 1000).max(1))
    } else if rc == -115 || rc == -11 || rc == -114 {
        Err(syscall::ETIMEDOUT)
    } else {
        Err(rc)
    }
}

#[cfg(not(target_os = "exo"))]
pub fn tcp_probe_once(_ip: [u8; 4], _port: u16, _timeout_ms: u64) -> Result<u64, i64> {
    Err(-110)
}

fn probe_ports_for(ip: [u8; 4]) -> &'static [u16] {
    match ip {
        [1, 1, 1, 1] | [1, 0, 0, 1] | [8, 8, 8, 8] | [8, 8, 4, 4] | [9, 9, 9, 9] | [10, 0, 2, 3] => &[53, 443, 80],
        _ => &[443, 80, 53],
    }
}

/// Execute `ping <target> [count]` and return `(line, color_kind)` where
/// `0 = main, 1 = ok, 2 = warn, 3 = err, 4 = accent`.
pub fn run_ping_command(target: &str, count: usize) -> Vec<(String, u8)> {
    let mut out = Vec::new();
    let count = count.clamp(1, 6);
    let ip = match resolve_dns_a(target) {
        Ok((ip, dns_srv, dns_us)) => {
            if parse_ipv4(target).is_none() && target != "localhost" {
                out.push((
                    format!("DNS ({}) : {} -> {} ({})", fmt_ipv4(dns_srv), target, fmt_ipv4(ip), fmt_ms(dns_us)),
                    4,
                ));
            }
            ip
        }
        Err(e) => {
            out.push((e, 3));
            return out;
        }
    };

    let ip_s = fmt_ipv4(ip);
    out.push((format!("PING {} ({}) : 56 octets de données", target, ip_s), 0));

    let is_local = ip[0] == 127 || (ip[0] == 10 && ip[1] == 0 && ip[2] == 2);
    let ident = ((now_ns() >> 4) as u16).max(1);
    let mut received = 0usize;
    let mut min_us = u64::MAX;
    let mut max_us = 0u64;
    let mut sum_us = 0u64;
    let mut tcp_fallback_port: Option<u16> = None;

    for seq in 1..=(count as u16) {
        if let Some(port) = tcp_fallback_port {
            match tcp_probe_once(ip, port, 1200) {
                Ok(rtt) => {
                    received += 1;
                    min_us = min_us.min(rtt);
                    max_us = max_us.max(rtt);
                    sum_us += rtt;
                    out.push((
                        format!("64 octets de {}:{} (TCP/ExoNet): seq={} temps={}", ip_s, port, seq, fmt_ms(rtt)),
                        1,
                    ));
                }
                Err(e) => out.push((format!("seq={}: délai expiré (TCP errno {})", seq, e), 3)),
            }
            continue;
        }

        match icmp_echo_once(ip, ident, seq, if is_local { 500 } else { 700 }) {
            Ok((rtt, ttl)) => {
                received += 1;
                min_us = min_us.min(rtt);
                max_us = max_us.max(rtt);
                sum_us += rtt;
                out.push((
                    format!("64 octets de {}: icmp_seq={} ttl={} temps={}", ip_s, seq, ttl, fmt_ms(rtt)),
                    1,
                ));
            }
            Err(_) if !is_local && seq == 1 => {
                // External ICMP blocked by host/Slirp (`ping_group_range = 1 0`) — probe real TCP!
                let mut hit = None;
                for &p in probe_ports_for(ip) {
                    if let Ok(rtt) = tcp_probe_once(ip, p, 1200) {
                        hit = Some((p, rtt));
                        break;
                    }
                }
                if let Some((port, rtt)) = hit {
                    tcp_fallback_port = Some(port);
                    received += 1;
                    min_us = min_us.min(rtt);
                    max_us = max_us.max(rtt);
                    sum_us += rtt;
                    out.push((
                        String::from("Note : ICMP externe filtré par l'hôte (QEMU Slirp) -> bascule sonde TCP directe"),
                        2,
                    ));
                    out.push((
                        format!("64 octets de {}:{} (TCP/ExoNet): seq={} temps={}", ip_s, port, seq, fmt_ms(rtt)),
                        1,
                    ));
                } else {
                    out.push((format!("icmp_seq={}: hôte injoignable (ICMP + TCP expirés)", seq), 3));
                }
            }
            Err(e) => {
                out.push((format!("icmp_seq={}: délai expiré (errno {})", seq, e), 3));
            }
        }
    }

    let loss = ((count - received) * 100) / count;
    out.push((format!("--- Statistiques ping {} ---", target), 4));
    out.push((
        format!("{} transmis, {} reçus, {}% perte", count, received, loss),
        if received > 0 { 1 } else { 3 },
    ));
    if received > 0 {
        let avg = sum_us / (received as u64);
        out.push((
            format!("rtt min/moy/max = {} / {} / {}", fmt_ms(min_us), fmt_ms(avg), fmt_ms(max_us)),
            0,
        ));
    }
    out
}

/// Complete live ExoNet diagnostic used by Settings ("Tester ExoNet") and Terminal (`nettest`).
pub fn run_full_net_diagnostic() -> NetDiagReport {
    let dns = read_dns_server();
    let ident = ((now_ns() >> 3) as u16).max(1);

    let (loopback_ok, loopback_ms) = match icmp_echo_once([127, 0, 0, 1], ident, 1, 250) {
        Ok((us, _)) => (true, fmt_ms(us)),
        Err(e) => (false, format!("err {}", e)),
    };

    let (gateway_ok, gateway_ms) = match icmp_echo_once([10, 0, 2, 2], ident.wrapping_add(1), 1, 400) {
        Ok((us, _)) => (true, fmt_ms(us)),
        Err(e) => (false, format!("err {}", e)),
    };

    let (dns_ok, dns_detail) = match resolve_dns_a("cloudflare.com") {
        Ok((ip, srv, us)) => (true, format!("cloudflare.com -> {} via {} ({})", fmt_ipv4(ip), fmt_ipv4(srv), fmt_ms(us))),
        Err(e) => (false, e),
    };

    let (external_ok, external_detail) = match icmp_echo_once([1, 1, 1, 1], ident.wrapping_add(2), 1, 450) {
        Ok((us, ttl)) => (true, format!("ICMP 1.1.1.1 OK (ttl={}, {})", ttl, fmt_ms(us))),
        Err(_) => match tcp_probe_once([1, 1, 1, 1], 53, 850) {
            Ok(us) => (true, format!("Internet 1.1.1.1:53 OK en {} (TCP, ICMP filtré Slirp)", fmt_ms(us))),
            Err(_) => match tcp_probe_once([8, 8, 8, 8], 53, 850) {
                Ok(us) => (true, format!("Internet 8.8.8.8:53 OK en {} (TCP)", fmt_ms(us))),
                Err(e) => (false, format!("Externe injoignable (errno {})", e)),
            },
        },
    };

    let summary = if external_ok && dns_ok {
        format!("Internet OK • Passerelle {} • {}", gateway_ms, external_detail)
    } else if gateway_ok {
        format!("Passerelle 10.0.2.2 OK ({}) • Externe : {}", gateway_ms, external_detail)
    } else if loopback_ok {
        format!("Loopback OK ({}) • Aucune passerelle matérielle joignable", loopback_ms)
    } else {
        String::from("Pile ExoNet hors ligne")
    };

    NetDiagReport {
        local_ip: String::from("10.0.2.15/24"),
        gateway_ip: String::from("10.0.2.2"),
        dns_ip: fmt_ipv4(dns),
        loopback_ok,
        loopback_ms,
        gateway_ok,
        gateway_ms,
        dns_ok,
        dns_detail,
        external_ok,
        external_detail,
        summary,
    }
}
