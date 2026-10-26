//! # servers/exosh
#![allow(dead_code, unused_imports)]
use core::cell::UnsafeCell;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use exo_syscall_abi as syscall;

use crate::types::*;
use crate::terminal::*;
use crate::builtins::*;
use crate::exec::*;

pub const STDOUT: u64 = 1;

pub const STDERR: u64 = 2;

pub const AT_FDCWD: u64 = (-100i64) as u64;

pub const PATH_MAX: usize = 256;

pub const LINE_MAX: usize = 256;

pub const IO_BUF: usize = 2048;

pub const DD_IO_BUF: usize = 1024 * 1024;

pub const SHELL_WRITE_CHUNK: usize = 64 * 1024;

pub const FS_BENCH_CHUNK: usize = 64 * 1024;

pub const DIRENT64_HEADER_SIZE: usize = 24;

pub const DT_DIR: u8 = 4;

pub const TREE_MAX_DEPTH: usize = 4;

pub const RM_MAX_DEPTH: usize = 8;

pub const HISTORY_MAX: usize = 16;

pub const EXEC_MAX_ARGS: usize = 32;

pub const EXEC_ARG_MAX: usize = 160;

pub const CLOCK_MONOTONIC: u64 = 1;

pub const DD_DEFAULT_BS: u64 = 512;

pub const S_IFMT: u32 = 0o170000;

pub const S_IFDIR: u32 = 0o040000;

pub const S_IFREG: u32 = 0o100000;

pub const ANSI_RESET: &[u8] = b"\x1b[0m";

pub const ANSI_DIR: &[u8] = b"\x1b[1;34m";

pub const ANSI_EXEC: &[u8] = b"\x1b[1;32m";

pub const ANSI_REVERSE: &[u8] = b"\x1b[7m";

pub const SIGTERM: u64 = 15;

pub const SIGKILL: u64 = 9;

pub const AF_INET: u16 = 2;

pub const SOCK_STREAM: u64 = 1;

pub const SOCK_RAW: u64 = 3;

pub const IPPROTO_ICMP: u64 = 1;

pub const EAGAIN: i64 = -11;

pub const EXDEV: i64 = -18;

pub const EISDIR: i64 = -21;

pub const PING_IDENT: u16 = 0x4558;

pub const ICMP_ECHO_HEADER_LEN: usize = 8;

pub const PING_PAYLOAD: &[u8] = b"exoos-ping";

pub const PING_PACKET_LEN: usize = ICMP_ECHO_HEADER_LEN + PING_PAYLOAD.len();

pub const PING_RECV_ATTEMPTS: usize = 1000;

pub const INPUT_REPLY_DRAIN_LIMIT: usize = 32;

pub const INPUT_REPLY_DRAIN_TIMEOUT_MS: u64 = 1;

pub const INPUT_REPLY_WAIT_TIMEOUT_MS: u64 = 10;

pub const HID_KEY_END: u16 = 0x004d;

pub const HID_KEY_HOME: u16 = 0x004a;

pub const HID_KEY_DELETE: u16 = 0x004c;

pub const HID_KEY_LEFT: u16 = 0x0050;

pub const HID_KEY_RIGHT: u16 = 0x004f;

pub const HID_KEY_UP: u16 = 0x0052;

pub const HID_KEY_DOWN: u16 = 0x0051;

pub const EXOSH_INPUT_REPLY_CHANNEL: u64 = 0x494E_5001;

pub const EXOSH_TTY_REPLY_CHANNEL: u64 = 0x5454_5901;

pub struct DdIoBuffer(pub UnsafeCell<[u8; DD_IO_BUF]>);

unsafe impl Sync for DdIoBuffer {}

pub static DD_IO_BUFFER: DdIoBuffer = DdIoBuffer(UnsafeCell::new([0; DD_IO_BUF]));

pub static INPUT_ATTACHED_ENDPOINT: AtomicU64 = AtomicU64::new(0);

pub static INPUT_REPLY_ENDPOINT: AtomicU64 = AtomicU64::new(0);

pub static TTY_REPLY_ENDPOINT: AtomicU64 = AtomicU64::new(0);

pub static STDIN_READ_ERROR_LOGGED: AtomicBool = AtomicBool::new(false);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Timespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

pub fn u64_to_str(mut value: u64, out: &mut [u8]) -> usize {
    if value == 0 {
        if !out.is_empty() {
            out[0] = b'0';
            return 1;
        }
        return 0;
    }
    let mut buf = [0u8; 20];
    let mut pos = buf.len();
    while value != 0 && pos > 0 {
        pos -= 1;
        buf[pos] = b'0' + (value % 10) as u8;
        value /= 10;
    }
    let len = buf.len() - pos;
    let n = len.min(out.len());
    out[..n].copy_from_slice(&buf[pos..pos + n]);
    n
}

pub fn prompt(state: &ShellState) {
    let mut out = [0u8; PATH_MAX + 8];
    let mut len = 0usize;
    push_bytes(&mut out, &mut len, b"exosh:");
    push_bytes(&mut out, &mut len, state.cwd());
    push_bytes(&mut out, &mut len, b"$ ");
    write_bytes(&out[..len]);
}

pub fn append_path_component(
    parent: &[u8; PATH_MAX],
    parent_len: usize,
    name: &[u8],
    out: &mut [u8; PATH_MAX],
) -> Option<usize> {
    if name.is_empty() || name.contains(&b'/') {
        return None;
    }

    let mut len = parent_len.min(PATH_MAX - 1);
    out[..len].copy_from_slice(&parent[..len]);
    if len == 0 {
        out[0] = b'/';
        len = 1;
    }
    if len > 1 {
        if len + 1 >= PATH_MAX {
            return None;
        }
        out[len] = b'/';
        len += 1;
    }
    if len + name.len() >= PATH_MAX {
        return None;
    }
    out[len..len + name.len()].copy_from_slice(name);
    len += name.len();
    out[len] = 0;
    Some(len)
}

pub fn basename(path: &[u8]) -> &[u8] {
    let path = trim_end_slashes(path);
    if path.is_empty() || path == b"/" {
        return b"/";
    }
    let mut i = path.len();
    while i > 0 {
        if path[i - 1] == b'/' {
            return &path[i..];
        }
        i -= 1;
    }
    path
}

pub fn trim_end_slashes(path: &[u8]) -> &[u8] {
    let mut end = path.len();
    while end > 1 && path[end - 1] == b'/' {
        end -= 1;
    }
    &path[..end]
}

pub fn stat_path(path: &[u8; PATH_MAX]) -> Option<LinuxStat> {
    let mut stat = LinuxStat::default();
    let rc = unsafe {
        syscall::syscall2(
            syscall::SYS_STAT,
            path.as_ptr() as u64,
            &mut stat as *mut LinuxStat as u64,
        )
    };
    if rc == 0 {
        Some(stat)
    } else {
        None
    }
}

pub fn is_dir_path(path: &[u8; PATH_MAX]) -> bool {
    stat_path(path)
        .map(|stat| is_dir_mode(stat.st_mode))
        .unwrap_or(false)
}

pub fn is_dir_mode(mode: u32) -> bool {
    mode & S_IFMT == S_IFDIR
}

pub fn is_regular_mode(mode: u32) -> bool {
    mode & S_IFMT == S_IFREG
}

pub fn absolute_path(cwd: &[u8], input: &[u8], out: &mut [u8; PATH_MAX]) -> Option<usize> {
    let input = trim(input);
    let mut len;
    if input.is_empty() || input[0] == b'/' {
        out[0] = b'/';
        len = 1;
    } else {
        len = cwd.len().min(PATH_MAX - 1);
        out[..len].copy_from_slice(&cwd[..len]);
        if len == 0 {
            out[0] = b'/';
            len = 1;
        }
    }

    let mut i = 0usize;
    while i <= input.len() {
        let start = i;
        while i < input.len() && input[i] != b'/' {
            i += 1;
        }
        let comp = &input[start..i];
        if comp.is_empty() || bytes_eq(comp, b".") {
        } else if bytes_eq(comp, b"..") {
            pop_component(out, &mut len);
        } else {
            if len > 1 {
                if len + 1 >= PATH_MAX {
                    return None;
                }
                out[len] = b'/';
                len += 1;
            }
            if len + comp.len() >= PATH_MAX {
                return None;
            }
            out[len..len + comp.len()].copy_from_slice(comp);
            len += comp.len();
        }
        i += 1;
    }

    if len == 0 {
        out[0] = b'/';
        len = 1;
    }
    if len >= PATH_MAX {
        return None;
    }
    out[len] = 0;
    Some(len)
}

pub fn pop_component(out: &mut [u8; PATH_MAX], len: &mut usize) {
    if *len <= 1 {
        *len = 1;
        out[0] = b'/';
        return;
    }
    let mut i = *len - 1;
    while i > 0 && out[i] != b'/' {
        i -= 1;
    }
    *len = if i == 0 { 1 } else { i };
}

pub fn first_token(line: &[u8]) -> (&[u8], &[u8]) {
    next_arg(line)
}

pub fn next_arg(input: &[u8]) -> (&[u8], &[u8]) {
    let input = trim_start(input);
    if input.is_empty() {
        return (&[], &[]);
    }
    if input[0] == b'"' || input[0] == b'\'' {
        let quote = input[0];
        let mut i = 1usize;
        while i < input.len() && input[i] != quote {
            i += 1;
        }
        if i < input.len() {
            let arg = &input[1..i];
            let rest = if i + 1 < input.len() { &input[i + 1..] } else { &[] };
            return (arg, rest);
        } else {
            return (&input[1..], &[]);
        }
    }
    let mut end = 0usize;
    while end < input.len() && !is_space(input[end]) {
        end += 1;
    }
    (&input[..end], &input[end..])
}

pub fn trim(input: &[u8]) -> &[u8] {
    trim_end(trim_start(input))
}

pub fn trim_start(input: &[u8]) -> &[u8] {
    let mut start = 0usize;
    while start < input.len() && is_space(input[start]) {
        start += 1;
    }
    &input[start..]
}

pub fn trim_end(input: &[u8]) -> &[u8] {
    let mut end = input.len();
    while end > 0 && is_space(input[end - 1]) {
        end -= 1;
    }
    &input[..end]
}

pub fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r')
}

pub fn find_byte(input: &[u8], needle: u8) -> Option<usize> {
    let mut i = 0usize;
    while i < input.len() {
        if input[i] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

pub fn bytes_eq(a: &[u8], b: &[u8]) -> bool {
    a == b
}

pub fn strip_prefix<'a>(input: &'a [u8], prefix: &[u8]) -> Option<&'a [u8]> {
    if input.len() < prefix.len() {
        return None;
    }
    if &input[..prefix.len()] == prefix {
        Some(&input[prefix.len()..])
    } else {
        None
    }
}

pub fn parse_u32(input: &[u8]) -> Option<u32> {
    if input.is_empty() {
        return None;
    }
    let mut value = 0u32;
    let mut i = 0usize;
    while i < input.len() {
        let b = input[i];
        if !b.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?;
        value = value.checked_add((b - b'0') as u32)?;
        i += 1;
    }
    Some(value)
}

pub fn parse_u64(input: &[u8]) -> Option<u64> {
    if input.is_empty() {
        return None;
    }
    let mut value = 0u64;
    let mut i = 0usize;
    while i < input.len() {
        let b = input[i];
        if !b.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?;
        value = value.checked_add((b - b'0') as u64)?;
        i += 1;
    }
    Some(value)
}

pub fn parse_ipv4(input: &[u8]) -> Option<[u8; 4]> {
    let mut out = [0u8; 4];
    let mut part = 0usize;
    let mut start = 0usize;
    let mut i = 0usize;
    while i <= input.len() {
        if i == input.len() || input[i] == b'.' {
            if part >= out.len() || start == i {
                return None;
            }
            let value = parse_u32(&input[start..i])?;
            if value > 255 {
                return None;
            }
            out[part] = value as u8;
            part += 1;
            start = i.saturating_add(1);
        }
        i += 1;
    }
    if part == 4 {
        Some(out)
    } else {
        None
    }
}

pub fn sockaddr_in(addr: [u8; 4], port: u16) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[0..2].copy_from_slice(&AF_INET.to_ne_bytes());
    out[2..4].copy_from_slice(&port.to_be_bytes());
    out[4..8].copy_from_slice(&addr);
    out
}

pub fn parse_size(input: &[u8]) -> Option<u64> {
    if input.is_empty() {
        return None;
    }
    let mut number_end = 0usize;
    while number_end < input.len() && input[number_end].is_ascii_digit() {
        number_end += 1;
    }
    let base = parse_u64(&input[..number_end])?;
    let suffix = &input[number_end..];
    let multiplier = if suffix.is_empty() {
        1
    } else if bytes_eq(suffix, b"K") || bytes_eq(suffix, b"k") {
        1024
    } else if bytes_eq(suffix, b"M") || bytes_eq(suffix, b"m") {
        1024 * 1024
    } else if bytes_eq(suffix, b"G") || bytes_eq(suffix, b"g") {
        1024 * 1024 * 1024
    } else {
        return None;
    };
    base.checked_mul(multiplier)
}

pub fn is_dev_zero(path: &[u8]) -> bool {
    bytes_eq(path, b"/dev/zero")
}

pub fn is_dev_null(path: &[u8]) -> bool {
    bytes_eq(path, b"/dev/null")
}

pub fn is_dev_urandom(path: &[u8]) -> bool {
    bytes_eq(path, b"/dev/urandom") || bytes_eq(path, b"/dev/random")
}

pub fn service_pid(name: &[u8]) -> Option<u32> {
    let mut entries = [syscall::ExoProcessInfo::zeroed(); 64];
    let count = process_list_snapshot(&mut entries);
    if count > 0 {
        let mut idx = 0usize;
        while idx < count as usize && idx < entries.len() {
            let entry = entries[idx];
            if bytes_eq(name, process_entry_name(&entry)) {
                return Some(entry.pid);
            }
            idx += 1;
        }
        return None;
    }

    None
}

pub fn write_padded(bytes: &[u8], width: usize) {
    write_bytes(bytes);
    let mut n = bytes.len();
    while n < width {
        write_all(b" ");
        n += 1;
    }
}

pub fn write_u32(mut value: u32) {
    let mut buf = [0u8; 10];
    let mut pos = buf.len();
    if value == 0 {
        write_all(b"0");
        return;
    }
    while value != 0 {
        pos -= 1;
        buf[pos] = b'0' + (value % 10) as u8;
        value /= 10;
    }
    write_bytes(&buf[pos..]);
}

pub fn write_ipv4(addr: [u8; 4]) {
    write_u32(addr[0] as u32);
    write_all(b".");
    write_u32(addr[1] as u32);
    write_all(b".");
    write_u32(addr[2] as u32);
    write_all(b".");
    write_u32(addr[3] as u32);
}

pub fn push_u32(out: &mut [u8], len: &mut usize, mut value: u32) {
    let mut buf = [0u8; 10];
    let mut pos = buf.len();
    if value == 0 {
        push_bytes(out, len, b"0");
        return;
    }
    while value != 0 {
        pos -= 1;
        buf[pos] = b'0' + (value % 10) as u8;
        value /= 10;
    }
    push_bytes(out, len, &buf[pos..]);
}

pub fn push_u64(out: &mut [u8], len: &mut usize, mut value: u64) {
    let mut buf = [0u8; 20];
    let mut pos = buf.len();
    if value == 0 {
        push_bytes(out, len, b"0");
        return;
    }
    while value != 0 {
        pos -= 1;
        buf[pos] = b'0' + (value % 10) as u8;
        value /= 10;
    }
    push_bytes(out, len, &buf[pos..]);
}

pub fn push_i64(out: &mut [u8], len: &mut usize, value: i64) {
    if value < 0 {
        push_bytes(out, len, b"-");
        push_u64(out, len, value.unsigned_abs());
    } else {
        push_u64(out, len, value as u64);
    }
}

pub fn push_bytes(out: &mut [u8], len: &mut usize, bytes: &[u8]) {
    let available = out.len().saturating_sub(*len);
    let n = bytes.len().min(available);
    if n == 0 {
        return;
    }
    out[*len..*len + n].copy_from_slice(&bytes[..n]);
    *len += n;
}

pub fn write_mode(mode: u32) {
    let mut buf = [0u8; 16];
    let mut len = 0usize;
    push_mode(&mut buf, &mut len, mode);
    write_bytes(&buf[..len]);
}

pub fn push_mode(out: &mut [u8], len: &mut usize, mode: u32) {
    if is_dir_mode(mode) {
        push_bytes(out, len, b"[dir:cap]");
    } else if (mode & 0o170000) == 0o060000 || (mode & 0o170000) == 0o020000 {
        push_bytes(out, len, b"[device ]");
    } else {
        let r = if mode & 0o400 != 0 { b'r' } else { b'-' };
        let w = if mode & 0o200 != 0 { b'w' } else { b'-' };
        let x = if mode & 0o100 != 0 { b'x' } else { b'-' };
        let cap_buf = [b'[', b'c', b'a', b'p', b':', r, w, x, b']'];
        push_bytes(out, len, &cap_buf);
    }
}

pub fn push_colored_name(
    out: &mut [u8],
    len: &mut usize,
    name: &[u8],
    mode: u32,
    append_slash: bool,
) {
    if is_dir_mode(mode) {
        push_bytes(out, len, ANSI_DIR);
        push_bytes(out, len, name);
        if append_slash {
            push_bytes(out, len, b"/");
        }
        push_bytes(out, len, ANSI_RESET);
    } else if mode & 0o111 != 0 {
        push_bytes(out, len, ANSI_EXEC);
        push_bytes(out, len, name);
        push_bytes(out, len, ANSI_RESET);
    } else {
        push_bytes(out, len, name);
    }
}

pub fn write_octal_mode(mode: u32) {
    let mut shift = 9i32;
    while shift >= 0 {
        let digit = ((mode >> shift) & 0x7) as u8;
        write_byte(b'0' + digit);
        shift -= 3;
    }
}

pub fn push_human_size(out: &mut [u8], len: &mut usize, size: i64) {
    if size < 0 {
        push_i64(out, len, size);
        return;
    }
    let mut value = size as u64;
    let units = [b'B', b'K', b'M', b'G'];
    let mut unit = 0usize;
    while value >= 1024 && unit + 1 < units.len() {
        value = value.saturating_add(512) / 1024;
        unit += 1;
    }
    push_u64(out, len, value);
    push_bytes(out, len, &[units[unit]]);
}

pub fn write_human_size(size: i64) {
    if size < 0 {
        write_i64(size);
        return;
    }
    let mut value = size as u64;
    let units = [b'B', b'K', b'M', b'G'];
    let mut unit = 0usize;
    while value >= 1024 && unit + 1 < units.len() {
        value = value.saturating_add(512) / 1024;
        unit += 1;
    }
    write_u64(value);
    write_bytes(&[units[unit]]);
}

pub fn write_human_u64(mut value: u64) {
    let units = [b'B', b'K', b'M', b'G', b'T'];
    let mut unit = 0usize;
    while value >= 1024 && unit + 1 < units.len() {
        value = value.saturating_add(512) / 1024;
        unit += 1;
    }
    write_u64(value);
    write_bytes(&[units[unit]]);
}

pub fn print_errno(prefix: &[u8], rc: i64) {
    write_bytes(prefix);
    write_all(b": errno ");
    write_i64(rc);
    write_all(b"\n");
}

pub fn write_i64(value: i64) {
    if value < 0 {
        write_all(b"-");
        write_u64(value.unsigned_abs());
    } else {
        write_u64(value as u64);
    }
}

pub fn write_u64(mut value: u64) {
    let mut buf = [0u8; 20];
    let mut pos = buf.len();
    if value == 0 {
        write_all(b"0");
        return;
    }
    while value != 0 {
        pos -= 1;
        buf[pos] = b'0' + (value % 10) as u8;
        value /= 10;
    }
    write_bytes(&buf[pos..]);
}

pub fn write_duration_ms(ns: u64) {
    write_u64(ns / 1_000_000);
    write_all(b"ms");
}

pub fn write_mib_per_sec(bytes: u64, ns: u64) {
    if ns == 0 {
        write_all(b"0.00");
        return;
    }
    let centi_mib_per_sec = (bytes as u128)
        .saturating_mul(100)
        .saturating_mul(1_000_000_000)
        .checked_div(1024u128 * 1024u128)
        .unwrap_or(0)
        .checked_div(ns as u128)
        .unwrap_or(0);
    write_fixed_2(centi_mib_per_sec.min(u64::MAX as u128) as u64);
}

pub fn write_fixed_2(centi_units: u64) {
    let whole = centi_units / 100;
    let frac = centi_units % 100;
    write_u64(whole);
    write_all(b".");
    write_byte(b'0' + (frac / 10) as u8);
    write_byte(b'0' + (frac % 10) as u8);
}

pub fn write_all(bytes: &[u8]) {
    write_bytes(bytes);
}

pub fn write_byte(byte: u8) {
    write_bytes(&[byte]);
}

pub fn write_bytes(bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    let _ = write_fd_all(STDOUT as i64, bytes);
}

pub fn write_fd_all(fd: i64, bytes: &[u8]) -> i64 {
    let mut written = 0usize;
    while written < bytes.len() {
        let remaining = bytes.len() - written;
        let chunk = remaining.min(SHELL_WRITE_CHUNK);
        let rc = unsafe {
            syscall::syscall3(
                syscall::SYS_WRITE,
                fd as u64,
                bytes[written..].as_ptr() as u64,
                chunk as u64,
            )
        };
        if rc <= 0 {
            return if rc == 0 { -5 } else { rc };
        }
        written += rc as usize;
    }
    bytes.len() as i64
}

pub fn close_fd(fd: i64) -> i64 {
    unsafe { syscall::syscall1(syscall::SYS_CLOSE, fd as u64) }
}

pub fn ftruncate_fd(fd: i64, len: u64) -> i64 {
    unsafe { syscall::syscall2(syscall::SYS_FTRUNCATE, fd as u64, len) }
}

pub fn fdatasync_fd(fd: i64) -> i64 {
    unsafe { syscall::syscall1(syscall::SYS_FDATASYNC, fd as u64) }
}

pub fn sleep_ms(ms: u64) {
    let ts = Timespec {
        tv_sec: (ms / 1000) as i64,
        tv_nsec: ((ms % 1000) * 1_000_000) as i64,
    };
    let _ = unsafe { syscall::syscall2(syscall::SYS_NANOSLEEP, &ts as *const _ as u64, 0) };
}

pub fn monotonic_ns() -> Option<u64> {
    let mut ts = Timespec::default();
    let rc = unsafe {
        syscall::syscall2(
            syscall::SYS_CLOCK_GETTIME,
            CLOCK_MONOTONIC,
            &mut ts as *mut Timespec as u64,
        )
    };
    if rc != 0 || ts.tv_sec < 0 || ts.tv_nsec < 0 {
        return None;
    }
    Some((ts.tv_sec as u64).saturating_mul(1_000_000_000) + ts.tv_nsec as u64)
}

pub const CRYPTO_SERVER_PID: u32 = 5;

pub const CRYPTO_PROTOCOL_VERSION: u8 = 3;

pub const CRYPTO_REQUEST_PAYLOAD_SIZE: usize = 200;

pub const CRYPTO_REPLY_DATA_SIZE: usize = 224;

pub const CRYPTO_RANDOM: u32 = 1;

pub const CRYPTO_OK: u32 = 0;

pub const IPC_RECV_TIMEOUT_MS: u64 = 5_000;

#[repr(C)]
pub struct CryptoRequestBench {
    pub sender_pid: u32,
    pub msg_type: u32,
    pub reply_endpoint: u64,
    pub payload_len: u16,
    pub version: u8,
    pub flags: u8,
    pub cap_token: syscall::ExoCapTokenWire,
    pub payload: [u8; CRYPTO_REQUEST_PAYLOAD_SIZE],
}

#[repr(C)]
pub struct CryptoReplyBench {
    pub sender_pid: u32,
    pub status: u32,
    pub key_handle: u32,
    pub data_len: u16,
    pub version: u8,
    pub flags: u8,
    pub data: [u8; CRYPTO_REPLY_DATA_SIZE],
}

