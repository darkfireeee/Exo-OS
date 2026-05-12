use super::syscall;

#[inline]
pub fn write_all(bytes: &[u8]) {
    if bytes.is_empty() {
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

#[inline]
pub fn line(message: &[u8]) {
    write_all(message);
    write_all(b"\n");
}

pub fn service_status(prefix: &[u8], name: &str, suffix: &[u8]) {
    write_all(prefix);
    write_all(name.as_bytes());
    write_all(suffix);
}

pub fn service_pid(prefix: &[u8], name: &str, pid: u32) {
    write_all(prefix);
    write_all(name.as_bytes());
    write_all(b" pid=");
    write_u32(pid);
    write_all(b"\n");
}

pub fn service_error(prefix: &[u8], name: &str, code: i64) {
    write_all(prefix);
    write_all(name.as_bytes());
    write_all(b" rc=");
    write_i64(code);
    write_all(b"\n");
}

pub fn write_u32(mut value: u32) {
    let mut buf = [0u8; 10];
    let mut len = 0usize;
    if value == 0 {
        write_all(b"0");
        return;
    }
    while value != 0 {
        buf[len] = b'0' + (value % 10) as u8;
        len += 1;
        value /= 10;
    }
    while len != 0 {
        len -= 1;
        write_all(&buf[len..len + 1]);
    }
}

pub fn write_i64(value: i64) {
    if value < 0 {
        write_all(b"-");
        write_u64(value.unsigned_abs());
    } else {
        write_u64(value as u64);
    }
}

fn write_u64(mut value: u64) {
    let mut buf = [0u8; 20];
    let mut len = 0usize;
    if value == 0 {
        write_all(b"0");
        return;
    }
    while value != 0 {
        buf[len] = b'0' + (value % 10) as u8;
        len += 1;
        value /= 10;
    }
    while len != 0 {
        len -= 1;
        write_all(&buf[len..len + 1]);
    }
}
