#![allow(unused_imports, dead_code)]
use super::types::*;
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

    pub fn copy_component(out: &mut [u8; PATH_MAX], len: &mut usize, comp: &[u8]) -> bool {
        if *len > 1 {
            if *len + 1 >= PATH_MAX {
                return false;
            }
            out[*len] = b'/';
            *len += 1;
        }
        if *len + comp.len() >= PATH_MAX {
            return false;
        }
        out[*len..*len + comp.len()].copy_from_slice(comp);
        *len += comp.len();
        true
    }

    pub fn pop_component(out: &mut [u8; PATH_MAX], len: &mut usize) {
        if *len <= 1 {
            *len = 1;
            return;
        }
        while *len > 1 && out[*len - 1] != b'/' {
            *len -= 1;
        }
        if *len > 1 {
            *len -= 1;
        }
    }

    pub fn normalize_into(src: &[u8], out: &mut [u8; PATH_MAX]) -> Option<usize> {
        let mut len = 1usize;
        out[0] = b'/';
        let mut i = 0usize;
        while i <= src.len() {
            while i < src.len() && src[i] == b'/' {
                i += 1;
            }
            let start = i;
            while i < src.len() && src[i] != b'/' {
                i += 1;
            }
            if start == i {
                break;
            }
            let comp = &src[start..i];
            if eq(comp, b".") {
                continue;
            }
            if eq(comp, b"..") {
                pop_component(out, &mut len);
                continue;
            }
            if !copy_component(out, &mut len, comp) {
                return None;
            }
        }
        if len >= PATH_MAX {
            return None;
        }
        out[len] = 0;
        Some(len)
    }

    pub fn path_arg(args: &Args, input: &[u8], out: &mut [u8; PATH_MAX]) -> Option<usize> {
        let input = if input.is_empty() { b"." } else { input };
        if input.starts_with(b"/") {
            return normalize_into(input, out);
        }
        let cwd = args.pwd();
        let mut joined = [0u8; PATH_MAX * 2];
        let mut len = 0usize;
        let cwd_len = cwd.len().min(joined.len());
        joined[..cwd_len].copy_from_slice(&cwd[..cwd_len]);
        len += cwd_len;
        if len == 0 || joined[len - 1] != b'/' {
            if len >= joined.len() {
                return None;
            }
            joined[len] = b'/';
            len += 1;
        }
        if len + input.len() > joined.len() {
            return None;
        }
        joined[len..len + input.len()].copy_from_slice(input);
        len += input.len();
        normalize_into(&joined[..len], out)
    }

    pub fn basename(path: &[u8]) -> &[u8] {
        let mut end = path.len();
        while end > 1 && path[end - 1] == b'/' {
            end -= 1;
        }
        let mut start = end;
        while start > 0 && path[start - 1] != b'/' {
            start -= 1;
        }
        &path[start..end]
    }

    pub fn dirname(path: &[u8], out: &mut [u8; PATH_MAX]) -> usize {
        let mut end = path.len();
        while end > 1 && path[end - 1] == b'/' {
            end -= 1;
        }
        let mut slash = end;
        while slash > 0 && path[slash - 1] != b'/' {
            slash -= 1;
        }
        let len = if slash == 0 {
            1
        } else {
            slash.saturating_sub(1).max(1)
        };
        if slash == 0 {
            out[0] = b'.';
            out[1] = 0;
            1
        } else {
            out[..len].copy_from_slice(&path[..len]);
            out[len] = 0;
            len
        }
    }

    pub fn open_path(path: &[u8; PATH_MAX], flags: u64, mode: u64) -> i64 {
        unsafe {
            syscall::syscall4(
                syscall::SYS_OPENAT,
                AT_FDCWD,
                path.as_ptr() as u64,
                flags,
                mode,
            )
        }
    }

    pub fn close(fd: i64) {
        if fd >= 0 {
            let _ = unsafe { syscall::syscall1(syscall::SYS_CLOSE, fd as u64) };
        }
    }

    pub fn write_fd_all(fd: u64, bytes: &[u8]) -> i64 {
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
                return if n == 0 { -5 } else { n };
            }
            done += n as usize;
        }
        bytes.len() as i64
    }

    pub fn ftruncate_fd(fd: i64, len: u64) -> i64 {
        unsafe { syscall::syscall2(syscall::SYS_FTRUNCATE, fd as u64, len) }
    }

    pub fn monotonic_ns() -> Option<u64> {
        let mut ts = LinuxTimespec::default();
        let rc = unsafe {
            syscall::syscall2(
                syscall::SYS_CLOCK_GETTIME,
                CLOCK_MONOTONIC,
                &mut ts as *mut LinuxTimespec as u64,
            )
        };
        if rc < 0 || ts.tv_sec < 0 || ts.tv_nsec < 0 {
            return None;
        }
        Some((ts.tv_sec as u64).saturating_mul(1_000_000_000) + ts.tv_nsec as u64)
    }

    pub fn stat_path(path: &[u8; PATH_MAX]) -> Option<LinuxStat> {
        let mut st = LinuxStat::default();
        let rc = unsafe {
            syscall::syscall2(
                syscall::SYS_STAT,
                path.as_ptr() as u64,
                &mut st as *mut LinuxStat as u64,
            )
        };
        if rc == 0 {
            Some(st)
        } else {
            None
        }
    }

    pub fn is_dir_mode(mode: u32) -> bool {
        mode & S_IFMT == S_IFDIR
    }

    pub fn path_len(path: &[u8; PATH_MAX]) -> usize {
        let mut len = 0usize;
        while len < PATH_MAX && path[len] != 0 {
            len += 1;
        }
        len
    }

    pub fn append_path_component(
        parent: &[u8; PATH_MAX],
        name: &[u8],
        out: &mut [u8; PATH_MAX],
    ) -> Option<usize> {
        if name.is_empty() || name.contains(&b'/') {
            return None;
        }
        let mut len = path_len(parent);
        out[..len].copy_from_slice(&parent[..len]);
        if len == 0 {
            out[0] = b'/';
            len = 1;
        }
        if len > 1 && out[len - 1] != b'/' {
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

    pub fn remove_path(path: &[u8; PATH_MAX], recursive: bool, force: bool, depth: usize) -> i32 {
        let Some(st) = stat_path(path) else {
            return if force { 0 } else { print_errno(b"rm", -2) };
        };

        if !is_dir_mode(st.st_mode) {
            let rc = unsafe { syscall::syscall1(syscall::SYS_UNLINK, path.as_ptr() as u64) };
            if rc < 0 && !force {
                return print_errno(b"rm", rc);
            }
            return 0;
        }

        if !recursive {
            return if force { 0 } else { print_errno(b"rm", EISDIR) };
        }
        if depth >= RM_MAX_DEPTH {
            return if force { 0 } else { print_errno(b"rm", -36) };
        }

        let fd = open_path(path, syscall::O_RDONLY, 0);
        if fd < 0 {
            return if force { 0 } else { print_errno(b"rm", fd) };
        }

        let mut rc = 0i32;
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
            if n == 0 {
                break;
            }
            if n < 0 {
                close(fd);
                return if force { 0 } else { print_errno(b"rm", n) };
            }

            let mut off = 0usize;
            while off + DIRENT64_HEADER_SIZE <= n as usize {
                let reclen = u16::from_le_bytes([buf[off + 16], buf[off + 17]]) as usize;
                if reclen == 0 || off + reclen > n as usize {
                    break;
                }
                let start = off + DIRENT64_HEADER_SIZE;
                let mut end = start;
                while end < off + reclen && buf[end] != 0 {
                    end += 1;
                }
                let name = &buf[start..end];
                if !name.is_empty() && !eq(name, b".") && !eq(name, b"..") {
                    let mut child = [0u8; PATH_MAX];
                    if append_path_component(path, name, &mut child).is_none() {
                        if !force {
                            rc = print_errno(b"rm", -36);
                        }
                    } else {
                        let child_rc = remove_path(&child, recursive, force, depth + 1);
                        if child_rc != 0 && rc == 0 {
                            rc = child_rc;
                        }
                    }
                }
                off += reclen;
            }
        }
        close(fd);

        let rmdir_rc = unsafe { syscall::syscall1(syscall::SYS_RMDIR, path.as_ptr() as u64) };
        if rmdir_rc < 0 && !force {
            return print_errno(b"rm", rmdir_rc);
        }
        rc
    }

    pub fn remove_star(args: &Args, arg: &[u8], recursive: bool, force: bool) -> i32 {
        let parent_arg = if eq(arg, b"*") {
            b"." as &[u8]
        } else if arg.len() > 2 && arg[arg.len() - 2] == b'/' && arg[arg.len() - 1] == b'*' {
            &arg[..arg.len() - 2]
        } else {
            return print_errno(b"rm", -22);
        };

        let mut parent = [0u8; PATH_MAX];
        if path_arg(args, parent_arg, &mut parent).is_none() {
            return if force { 0 } else { print_errno(b"rm", -36) };
        }
        let fd = open_path(&parent, syscall::O_RDONLY, 0);
        if fd < 0 {
            return if force { 0 } else { print_errno(b"rm", fd) };
        }

        let mut rc = 0i32;
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
            if n == 0 {
                break;
            }
            if n < 0 {
                close(fd);
                return if force { 0 } else { print_errno(b"rm", n) };
            }

            let mut off = 0usize;
            while off + DIRENT64_HEADER_SIZE <= n as usize {
                let reclen = u16::from_le_bytes([buf[off + 16], buf[off + 17]]) as usize;
                if reclen == 0 || off + reclen > n as usize {
                    break;
                }
                let start = off + DIRENT64_HEADER_SIZE;
                let mut end = start;
                while end < off + reclen && buf[end] != 0 {
                    end += 1;
                }
                let name = &buf[start..end];
                if !name.is_empty() && name[0] != b'.' && !eq(name, b".") && !eq(name, b"..") {
                    let mut child = [0u8; PATH_MAX];
                    if append_path_component(&parent, name, &mut child).is_none() {
                        if !force {
                            rc = print_errno(b"rm", -36);
                        }
                    } else {
                        let child_rc = remove_path(&child, recursive, force, 1);
                        if child_rc != 0 && rc == 0 {
                            rc = child_rc;
                        }
                    }
                }
                off += reclen;
            }
        }
        close(fd);
        rc
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

    pub fn print_long_entry(
        path: &[u8; PATH_MAX],
        display_name: &[u8],
        human: bool,
        append_slash: bool,
    ) {
        let mut line = [0u8; PATH_MAX + 128];
        let mut len = 0usize;
        let Some(stat) = stat_path(path) else {
            push_bytes(&mut line, &mut len, b"?????????? ? ? ? ");
            push_bytes(&mut line, &mut len, display_name);
            push_bytes(&mut line, &mut len, b"\n");
            write_all(STDOUT, &line[..len]);
            return;
        };
        push_mode(&mut line, &mut len, stat.st_mode);
        push_bytes(&mut line, &mut len, b" ");
        push_u64(&mut line, &mut len, stat.st_nlink);
        push_bytes(&mut line, &mut len, b" ");
        push_u64(&mut line, &mut len, stat.st_uid as u64);
        push_bytes(&mut line, &mut len, b" ");
        push_u64(&mut line, &mut len, stat.st_gid as u64);
        push_bytes(&mut line, &mut len, b" ");
        if human {
            push_human_size(&mut line, &mut len, stat.st_size);
        } else {
            push_i64(&mut line, &mut len, stat.st_size);
        }
        push_bytes(&mut line, &mut len, b" ");
        push_colored_name(&mut line, &mut len, display_name, stat.st_mode, append_slash);
        push_bytes(&mut line, &mut len, b"\n");
        write_all(STDOUT, &line[..len]);
    }

    pub fn is_dev_zero(path: &[u8]) -> bool {
        eq(path, b"/dev/zero")
    }

    pub fn is_dev_null(path: &[u8]) -> bool {
        eq(path, b"/dev/null")
    }

    pub fn is_dev_urandom(path: &[u8]) -> bool {
        eq(path, b"/dev/urandom") || eq(path, b"/dev/random")
    }

    pub fn read_file_to_fd(path: &[u8; PATH_MAX], out_fd: u64) -> i32 {
        let fd = open_path(path, syscall::O_RDONLY, 0);
        if fd < 0 {
            return print_errno(b"cat", fd);
        }
        let mut buf = [0u8; IO_BUF];
        loop {
            let n = unsafe {
                syscall::syscall3(
                    syscall::SYS_READ,
                    fd as u64,
                    buf.as_mut_ptr() as u64,
                    buf.len() as u64,
                )
            };
            if n == 0 {
                break;
            }
            if n < 0 {
                close(fd);
                return print_errno(b"read", n);
            }
            write_all(out_fd, &buf[..n as usize]);
        }
        close(fd);
        0
    }
