use super::syscall;
use core::sync::atomic::{AtomicBool, Ordering};

static CONSOLE_QUIET: AtomicBool = AtomicBool::new(false);

pub fn set_console_quiet(quiet: bool) {
    CONSOLE_QUIET.store(quiet, Ordering::Release);
}

#[inline]
pub fn write_all(bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    if CONSOLE_QUIET.load(Ordering::Acquire) {
        return;
    }
    unsafe {
        let _ = syscall::syscall3(
            syscall::SYS_WRITE,
            1,
            bytes.as_ptr() as u64,
            bytes.len() as u64,
        );
    }
}

fn push_bytes(out: &mut [u8], len: &mut usize, bytes: &[u8]) {
    let room = out.len().saturating_sub(*len);
    let copy_len = bytes.len().min(room);
    out[*len..*len + copy_len].copy_from_slice(&bytes[..copy_len]);
    *len += copy_len;
}

fn push_u64(out: &mut [u8], len: &mut usize, mut value: u64) {
    let mut digits = [0u8; 20];
    let mut digit_len = 0usize;
    if value == 0 {
        push_bytes(out, len, b"0");
        return;
    }
    while value != 0 {
        digits[digit_len] = b'0' + (value % 10) as u8;
        digit_len += 1;
        value /= 10;
    }
    while digit_len != 0 {
        digit_len -= 1;
        push_bytes(out, len, &digits[digit_len..digit_len + 1]);
    }
}

fn push_i64(out: &mut [u8], len: &mut usize, value: i64) {
    if value < 0 {
        push_bytes(out, len, b"-");
        push_u64(out, len, value.unsigned_abs());
    } else {
        push_u64(out, len, value as u64);
    }
}

#[inline]
pub fn line(message: &[u8]) {
    let mut buf = [0u8; 128];
    let mut len = 0usize;
    push_bytes(&mut buf, &mut len, message);
    push_bytes(&mut buf, &mut len, b"\n");
    write_all(&buf[..len]);
}

pub fn service_status(prefix: &[u8], name: &str, suffix: &[u8]) {
    let mut buf = [0u8; 128];
    let mut len = 0usize;
    push_bytes(&mut buf, &mut len, prefix);
    push_bytes(&mut buf, &mut len, name.as_bytes());
    push_bytes(&mut buf, &mut len, suffix);
    write_all(&buf[..len]);
}

pub fn service_pid(prefix: &[u8], name: &str, pid: u32) {
    let mut buf = [0u8; 128];
    let mut len = 0usize;
    push_bytes(&mut buf, &mut len, prefix);
    push_bytes(&mut buf, &mut len, name.as_bytes());
    push_bytes(&mut buf, &mut len, b" pid=");
    push_u64(&mut buf, &mut len, pid as u64);
    push_bytes(&mut buf, &mut len, b"\n");
    write_all(&buf[..len]);
}

pub fn service_error(prefix: &[u8], name: &str, code: i64) {
    let mut buf = [0u8; 128];
    let mut len = 0usize;
    push_bytes(&mut buf, &mut len, prefix);
    push_bytes(&mut buf, &mut len, name.as_bytes());
    push_bytes(&mut buf, &mut len, b" rc=");
    push_i64(&mut buf, &mut len, code);
    push_bytes(&mut buf, &mut len, b"\n");
    write_all(&buf[..len]);
}
