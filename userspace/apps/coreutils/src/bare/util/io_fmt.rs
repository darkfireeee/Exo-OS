#![allow(unused_imports, dead_code)]
use super::path_fs::{is_dir_mode, is_regular_mode};
use crate::bare::types::*;
use exo_syscall_abi as syscall;

    pub fn write_all(fd: u64, bytes: &[u8]) {
        let mut done = 0usize;
        while done < bytes.len() {
            let n = unsafe {
                syscall::syscall3(
                    syscall::SYS_WRITE,
                    fd,
                    bytes[done..].as_ptr() as u64,
                    (bytes.len() - done) as u64,
                )
            };
            if n <= 0 {
                return;
            }
            done += n as usize;
        }
    }

    pub fn write_byte(fd: u64, byte: u8) {
        write_all(fd, &[byte]);
    }

    pub fn write_u64(fd: u64, mut value: u64) {
        let mut buf = [0u8; 20];
        let mut pos = buf.len();
        if value == 0 {
            write_byte(fd, b'0');
            return;
        }
        while value != 0 {
            pos -= 1;
            buf[pos] = b'0' + (value % 10) as u8;
            value /= 10;
        }
        write_all(fd, &buf[pos..]);
    }

    pub fn write_i64(fd: u64, value: i64) {
        if value < 0 {
            write_byte(fd, b'-');
            write_u64(fd, value.unsigned_abs());
        } else {
            write_u64(fd, value as u64);
        }
    }

    pub fn print_errno(cmd: &[u8], rc: i64) -> i32 {
        write_all(STDERR, cmd);
        write_all(STDERR, b": errno ");
        write_i64(STDERR, rc);
        write_byte(STDERR, b'\n');
        1
    }

    pub fn eq(a: &[u8], b: &[u8]) -> bool {
        a == b
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

    pub fn parse_i32(input: &[u8]) -> Option<i32> {
        parse_u64(input).and_then(|v| i32::try_from(v).ok())
    }

    pub fn strip_prefix<'a>(input: &'a [u8], prefix: &[u8]) -> Option<&'a [u8]> {
        if input.len() < prefix.len() {
            return None;
        }
        (&input[..prefix.len()] == prefix).then_some(&input[prefix.len()..])
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
        } else if eq(suffix, b"K") || eq(suffix, b"k") {
            1024
        } else if eq(suffix, b"M") || eq(suffix, b"m") {
            1024 * 1024
        } else if eq(suffix, b"G") || eq(suffix, b"g") {
            1024 * 1024 * 1024
        } else {
            return None;
        };
        base.checked_mul(multiplier)
    }


    pub fn write_mode(mode: u32) {
        let mut out = [b'-'; 10];
        out[0] = if is_dir_mode(mode) { b'd' } else { b'-' };
        let bits = [
            (0o400, b'r'),
            (0o200, b'w'),
            (0o100, b'x'),
            (0o040, b'r'),
            (0o020, b'w'),
            (0o010, b'x'),
            (0o004, b'r'),
            (0o002, b'w'),
            (0o001, b'x'),
        ];
        let mut i = 0usize;
        while i < bits.len() {
            if mode & bits[i].0 != 0 {
                out[i + 1] = bits[i].1;
            }
            i += 1;
        }
        write_all(STDOUT, &out);
    }

    pub fn write_human_size(size: i64) {
        if size < 0 {
            write_i64(STDOUT, size);
            return;
        }
        let mut value = size as u64;
        let units = [b'B', b'K', b'M', b'G', b'T'];
        let mut unit = 0usize;
        while value >= 1024 && unit + 1 < units.len() {
            value = value.saturating_add(512) / 1024;
            unit += 1;
        }
        write_u64(STDOUT, value);
        write_byte(STDOUT, units[unit]);
    }

    pub fn write_fixed_2(fd: u64, centi_units: u64) {
        let whole = centi_units / 100;
        let frac = centi_units % 100;
        write_u64(fd, whole);
        write_byte(fd, b'.');
        write_byte(fd, b'0' + (frac / 10) as u8);
        write_byte(fd, b'0' + (frac % 10) as u8);
    }

    pub fn write_duration_ms(fd: u64, ns: u64) {
        write_u64(fd, ns / 1_000_000);
        write_all(fd, b"ms");
    }

    pub fn write_mib_per_sec(fd: u64, bytes: u64, ns: u64) {
        if ns == 0 {
            write_all(fd, b"0.00");
            return;
        }
        let centi_mib_per_sec = (bytes as u128)
            .saturating_mul(100)
            .saturating_mul(1_000_000_000)
            .checked_div(1024u128 * 1024u128)
            .unwrap_or(0)
            .checked_div(ns as u128)
            .unwrap_or(0)
            .min(u64::MAX as u128) as u64;
        write_fixed_2(fd, centi_mib_per_sec);
    }

    pub fn write_colored_name(name: &[u8], mode: u32, append_slash: bool) {
        if is_dir_mode(mode) {
            write_all(STDOUT, ANSI_DIR);
            write_all(STDOUT, name);
            if append_slash {
                write_byte(STDOUT, b'/');
            }
            write_all(STDOUT, ANSI_RESET);
        } else if mode & 0o111 != 0 {
            write_all(STDOUT, ANSI_EXEC);
            write_all(STDOUT, name);
            write_all(STDOUT, ANSI_RESET);
        } else {
            write_all(STDOUT, name);
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

    pub fn push_mode(out: &mut [u8], len: &mut usize, mode: u32) {
        let kind = if is_dir_mode(mode) {
            b'd'
        } else if is_regular_mode(mode) {
            b'-'
        } else {
            b'?'
        };
        let mut buf = [b'-'; 10];
        buf[0] = kind;
        let bits = [
            (0o400, b'r'),
            (0o200, b'w'),
            (0o100, b'x'),
            (0o040, b'r'),
            (0o020, b'w'),
            (0o010, b'x'),
            (0o004, b'r'),
            (0o002, b'w'),
            (0o001, b'x'),
        ];
        let mut i = 0usize;
        while i < bits.len() {
            if mode & bits[i].0 != 0 {
                buf[i + 1] = bits[i].1;
            }
            i += 1;
        }
        push_bytes(out, len, &buf);
    }

    pub fn push_human_size(out: &mut [u8], len: &mut usize, size: i64) {
        if size < 0 {
            push_i64(out, len, size);
            return;
        }
        let mut value = size as u64;
        let units = [b'B', b'K', b'M', b'G', b'T'];
        let mut unit = 0usize;
        while value >= 1024 && unit + 1 < units.len() {
            value = value.saturating_add(512) / 1024;
            unit += 1;
        }
        push_u64(out, len, value);
        push_bytes(out, len, &[units[unit]]);
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

    const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

    pub fn push_hex_u16(out: &mut [u8], len: &mut usize, val: u16) {
        let bytes = [
            HEX_DIGITS[((val >> 12) & 0xF) as usize],
            HEX_DIGITS[((val >> 8) & 0xF) as usize],
            HEX_DIGITS[((val >> 4) & 0xF) as usize],
            HEX_DIGITS[(val & 0xF) as usize],
        ];
        push_bytes(out, len, &bytes);
    }

    pub fn push_hex_bytes(out: &mut [u8], len: &mut usize, bytes: &[u8]) {
        for &b in bytes {
            let pair = [
                HEX_DIGITS[(b >> 4) as usize],
                HEX_DIGITS[(b & 0xF) as usize],
            ];
            push_bytes(out, len, &pair);
        }
    }

    pub fn push_u64_padded4(out: &mut [u8], len: &mut usize, val: u64) {
        let v = (val % 10_000) as u32;
        let digits = [
            b'0' + ((v / 1000) % 10) as u8,
            b'0' + ((v / 100) % 10) as u8,
            b'0' + ((v / 10) % 10) as u8,
            b'0' + (v % 10) as u8,
        ];
        push_bytes(out, len, &digits);
    }

