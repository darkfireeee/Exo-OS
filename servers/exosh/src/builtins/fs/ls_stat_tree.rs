//! Directory listing, metadata inspection, and tree traversal (`ls`, `stat`, `tree`).
#![allow(dead_code, unused_imports)]
use exo_syscall_abi as syscall;
use crate::builtins::*;
use crate::types::*;
use crate::util::*;
use crate::terminal::*;
use crate::exec::*;

pub fn cmd_ls(rest: &[u8], state: &ShellState) {
    let (opts, target_arg) = parse_ls_args(rest);
    let target = if target_arg.is_empty() {
        state.cwd()
    } else {
        target_arg
    };
    let mut path = [0u8; PATH_MAX];
    let Some(path_len) = absolute_path(state.cwd(), target, &mut path) else {
        write_all(b"ls: path too long\n");
        return;
    };

    let fd = unsafe {
        syscall::syscall4(
            syscall::SYS_OPENAT,
            AT_FDCWD,
            path.as_ptr() as u64,
            syscall::O_RDONLY,
            0,
        )
    };
    if fd < 0 {
        if opts.long {
            print_long_entry(&path, path_len, basename(&path[..path_len]), opts);
        } else if let Some(stat) = stat_path(&path) {
            let mut line = [0u8; PATH_MAX + 32];
            let mut len = 0usize;
            push_colored_name(
                &mut line,
                &mut len,
                basename(&path[..path_len]),
                stat.st_mode,
                is_dir_mode(stat.st_mode) && path_len > 1,
            );
            push_bytes(&mut line, &mut len, b"\n");
            write_bytes(&line[..len]);
        } else {
            print_errno(b"ls", fd);
        }
        return;
    }

    let mut buf = [0u8; IO_BUF];
    let mut out_buf = [0u8; IO_BUF];
    let mut out_len = 0usize;
    let mut printed = false;
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
            if out_len > 0 {
                write_bytes(&out_buf[..out_len]);
                out_len = 0;
            }
            print_errno(b"ls", n);
            break;
        }
        parse_ls_dirents(
            &buf[..n as usize],
            &path,
            path_len,
            opts,
            &mut printed,
            &mut out_buf,
            &mut out_len,
        );
    }
    if out_len > 0 {
        write_bytes(&out_buf[..out_len]);
    }
    if printed && !opts.long {
        write_all(b"\n");
    }
    let _ = close_fd(fd);
}


pub fn cmd_tree(rest: &[u8], state: &ShellState) {
    let (arg, _) = next_arg(rest);
    let target = if arg.is_empty() { state.cwd() } else { arg };
    let mut path = [0u8; PATH_MAX];
    let Some(len) = absolute_path(state.cwd(), target, &mut path) else {
        write_all(b"tree: path too long\n");
        return;
    };

    write_bytes(&path[..len]);
    write_all(b"\n");
    tree_walk(&path, len, 0);
}

pub fn tree_walk(path: &[u8; PATH_MAX], path_len: usize, depth: usize) {
    if depth >= TREE_MAX_DEPTH {
        return;
    }

    let fd = unsafe {
        syscall::syscall4(
            syscall::SYS_OPENAT,
            AT_FDCWD,
            path.as_ptr() as u64,
            syscall::O_RDONLY,
            0,
        )
    };
    if fd < 0 {
        if depth == 0 {
            print_errno(b"tree", fd);
        }
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
        if n == 0 {
            break;
        }
        if n < 0 {
            if depth == 0 {
                print_errno(b"tree", n);
            }
            break;
        }
        parse_tree_dirents(&buf[..n as usize], path, path_len, depth);
    }

    let _ = close_fd(fd);
}

pub fn cmd_stat(rest: &[u8], state: &ShellState) {
    let (arg, _) = next_arg(rest);
    if arg.is_empty() {
        write_all(b"stat: usage: stat <path>\n");
        return;
    }
    let mut path = [0u8; PATH_MAX];
    let Some(path_len) = absolute_path(state.cwd(), arg, &mut path) else {
        write_all(b"stat: path too long\n");
        return;
    };
    let resolved_opt = resolve_exofs_path(&path, path_len);
    let stat_opt = stat_path(&path);
    if resolved_opt.is_none() && stat_opt.is_none() {
        print_errno(b"stat", syscall::ENOENT);
        return;
    }

    let is_dir = stat_opt.map(|s| is_dir_mode(s.st_mode)).unwrap_or(false);
    let obj_kind = resolved_opt.map(|r| r.object_kind).unwrap_or(if is_dir { 1 } else { 0 });

    write_all(b"Object   : ");
    write_bytes(&path[..path_len]);
    write_all(b"\nType     : ");
    match obj_kind {
        1 => write_all(b"Directory (d)\n"),
        2 => write_all(b"Executable Code (x)\n"),
        3 => write_all(b"Relation (r)\n"),
        4 => write_all(b"Secret (s)\n"),
        5 => write_all(b"Configuration (c)\n"),
        _ => if is_dir {
            write_all(b"Directory (d)\n")
        } else {
            write_all(b"Immutable Blob (b)\n")
        },
    }

    write_all(b"Rights   : ");
    if is_dir {
        write_all(b"rw-lk-d (READ | WRITE | LIST | LINK | DERIVE)\n");
    } else if path.starts_with(b"/mnt/usb") {
        write_all(b"rw----d (READ | WRITE | DERIVE, MS_NOEXEC enforced)\n");
    } else {
        write_all(b"rw---sd (READ | WRITE | SEAL | DERIVE)\n");
    }

    if let Some(res) = resolved_opt {
        let mut hex_buf = [0u8; 64];
        let mut hlen = 0usize;
        push_hex_bytes(&mut hex_buf, &mut hlen, &res.object_id[..8]);
        write_all(b"CapToken : @");
        write_bytes(&hex_buf[..hlen]);
        write_all(b"\nEpoch    : ");
        write_u64(res.epoch_id.max(1));
        write_all(b"\nSize     : ");
        write_u64(res.size_bytes);
        write_all(b" bytes\nBLAKE3   : ");
        if obj_kind == 4 {
            write_all(b"[redacted - SecretObject S-09]\n");
        } else {
            let mut b3_buf = [0u8; 64];
            let mut blen = 0usize;
            push_hex_bytes(&mut b3_buf, &mut blen, &res.blob_id);
            write_bytes(&b3_buf[..blen]);
            write_all(b"\n");
        }
    } else if let Some(st) = stat_opt {
        write_all(b"Size     : ");
        write_i64(st.st_size);
        write_all(b" bytes\nEpoch    : ---- (VFS pseudo/compat)\n");
    }
}


pub fn parse_ls_args(mut rest: &[u8]) -> (LsOptions, &[u8]) {
    let mut opts = LsOptions::default();
    loop {
        let (arg, tail) = next_arg(rest);
        if arg.len() < 2 || arg[0] != b'-' {
            return (opts, arg);
        }
        let mut i = 1usize;
        while i < arg.len() {
            match arg[i] {
                b'l' => opts.long = true,
                b'a' => opts.all = true,
                b'h' => opts.human = true,
                _ => {}
            }
            i += 1;
        }
        rest = tail;
    }
}

pub fn parse_ls_dirents(
    buf: &[u8],
    parent_path: &[u8; PATH_MAX],
    parent_len: usize,
    opts: LsOptions,
    printed: &mut bool,
    out_buf: &mut [u8; IO_BUF],
    out_len: &mut usize,
) {
    let mut off = 0usize;
    while off + DIRENT64_HEADER_SIZE <= buf.len() {
        let reclen = u16::from_le_bytes([buf[off + 16], buf[off + 17]]) as usize;
        if reclen == 0 || off + reclen > buf.len() {
            break;
        }
        let dtype = buf[off + 18];
        let name_start = off + DIRENT64_HEADER_SIZE;
        let mut name_end = name_start;
        while name_end < off + reclen && buf[name_end] != 0 {
            name_end += 1;
        }
        let name = &buf[name_start..name_end];
        if !name.is_empty() && (opts.all || name[0] != b'.') {
            if opts.long {
                if *out_len > 0 {
                    write_bytes(&out_buf[..*out_len]);
                    *out_len = 0;
                }
                let mut child = [0u8; PATH_MAX];
                if let Some(child_len) =
                    append_path_component(parent_path, parent_len, name, &mut child)
                {
                    print_long_entry(&child, child_len, name, opts);
                }
            } else {
                if *printed {
                    if *out_len + 2 >= out_buf.len() {
                        write_bytes(&out_buf[..*out_len]);
                        *out_len = 0;
                    }
                    push_bytes(out_buf, out_len, b"  ");
                }
                if *out_len + name.len() + 32 >= out_buf.len() {
                    write_bytes(&out_buf[..*out_len]);
                    *out_len = 0;
                }
                if dtype == DT_DIR {
                    push_colored_name(out_buf, out_len, name, S_IFDIR | 0o755, true);
                } else {
                    push_colored_name(out_buf, out_len, name, S_IFREG | 0o644, false);
                }
            }
            *printed = true;
        }
        off += reclen;
    }
}

pub fn resolve_exofs_path(path: &[u8; PATH_MAX], path_len: usize) -> Option<syscall::ExofsPathResolveResult> {
    if path_len == 0 {
        return None;
    }
    let mut resolved = syscall::ExofsPathResolveResult::default();
    let rc = unsafe {
        syscall::exofs_path_resolve_raw(
            path.as_ptr() as u64,
            path_len as u64,
            0,
            &mut resolved,
            syscall::EXOFS_RIGHT_READ_ONLY as u64,
        )
    };
    if rc >= 0 {
        Some(resolved)
    } else {
        None
    }
}

pub fn push_hex_bytes(out: &mut [u8], len: &mut usize, bytes: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for &b in bytes {
        let pair = [
            HEX[(b >> 4) as usize],
            HEX[(b & 0xF) as usize],
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

fn push_hex16(buf: &mut [u8], len: &mut usize, val: u16) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let b0 = HEX[((val >> 12) & 0xF) as usize];
    let b1 = HEX[((val >> 8) & 0xF) as usize];
    let b2 = HEX[((val >> 4) & 0xF) as usize];
    let b3 = HEX[(val & 0xF) as usize];
    push_bytes(buf, len, &[b0, b1, b2, b3]);
}

pub fn print_long_entry(path: &[u8; PATH_MAX], path_len: usize, name: &[u8], opts: LsOptions) {
    let mut line = [0u8; PATH_MAX + 160];
    let mut len = 0usize;

    let resolved_opt = resolve_exofs_path(path, path_len);
    let stat_opt = stat_path(path);

    if resolved_opt.is_none() && stat_opt.is_none() {
        push_bytes(&mut line, &mut len, b"?  -------  .           @0000  ep:----  0         --------  ");
        push_bytes(&mut line, &mut len, name);
        push_bytes(&mut line, &mut len, b"\n");
        write_bytes(&line[..len]);
        return;
    }

    let is_usb = path.starts_with(b"/mnt/usb");
    let is_dir = stat_opt.map(|s| is_dir_mode(s.st_mode)).unwrap_or(false);
    let is_exec = stat_opt.map(|s| (s.st_mode & 0o111) != 0).unwrap_or(false) && !is_usb;

    // 1. Type
    let obj_type = if is_dir {
        b'd'
    } else if let Some(ref res) = resolved_opt {
        match res.object_kind {
            1 => b'd',
            2 => b'x',
            3 => b'r',
            4 => b's',
            5 => b'c',
            _ => if is_exec { b'x' } else { b'b' },
        }
    } else if is_exec {
        b'x'
    } else {
        b'b'
    };
    push_bytes(&mut line, &mut len, &[obj_type, b' ', b' ']);

    // 2. Rights (7 caractères)
    let rights: [u8; 7] = if is_dir {
        *b"rw-lk-d"
    } else if is_usb {
        *b"rw----d"
    } else if obj_type == b'x' {
        *b"r-x---d"
    } else if obj_type == b's' {
        *b"r----sd"
    } else {
        *b"rw---sd"
    };
    push_bytes(&mut line, &mut len, &rights);
    push_bytes(&mut line, &mut len, b"  ");

    // 3. Flags (10 caractères)
    let flags_str: &[u8; 10] = if is_usb {
        b"[usb]     "
    } else if let Some(ref res) = resolved_opt {
        if (res.flags & 0x01) != 0 {
            b"[sealed]  "
        } else if obj_type == b'x' {
            b"[sig ok]  "
        } else if obj_type == b's' {
            b"[enc]     "
        } else {
            b".         "
        }
    } else {
        b".         "
    };
    push_bytes(&mut line, &mut len, flags_str);
    push_bytes(&mut line, &mut len, b"  @");

    // 4. CapToken (@<token:04x>)
    let token_u16 = if let Some(ref res) = resolved_opt {
        u16::from_le_bytes([res.object_id[0], res.object_id[1]])
    } else if let Some(ref st) = stat_opt {
        (st.st_ino & 0xFFFF) as u16
    } else {
        0
    };
    push_hex16(&mut line, &mut len, token_u16);

    // 5. Epoch (ep:<epoch:04>)
    push_bytes(&mut line, &mut len, b"  ep:");
    if let Some(ref res) = resolved_opt {
        if !is_usb {
            push_u64_padded4(&mut line, &mut len, res.epoch_id.max(1));
        } else {
            push_bytes(&mut line, &mut len, b"----");
        }
    } else {
        push_bytes(&mut line, &mut len, b"----");
    }
    push_bytes(&mut line, &mut len, b"  ");

    // 6. Size (8 colonnes min)
    let size_val = resolved_opt
        .map(|r| r.size_bytes as i64)
        .or_else(|| stat_opt.map(|s| s.st_size))
        .unwrap_or(0);
    let size_start = len;
    if opts.human {
        push_human_size(&mut line, &mut len, size_val);
    } else {
        push_i64(&mut line, &mut len, size_val);
    }
    let size_written = len - size_start;
    let mut pad = 0usize;
    while size_written + pad < 8 {
        push_bytes(&mut line, &mut len, b" ");
        pad += 1;
    }
    push_bytes(&mut line, &mut len, b"  ");

    // 7. BLAKE3 truncated to 8 hex chars (-------- for dirs and secrets S-09)
    if obj_type == b'd' || obj_type == b's' {
        push_bytes(&mut line, &mut len, b"--------");
    } else if let Some(ref res) = resolved_opt {
        if res.blob_id != [0u8; 32] {
            push_hex_bytes(&mut line, &mut len, &res.blob_id[..4]);
        } else {
            push_bytes(&mut line, &mut len, b"00000000");
        }
    } else {
        push_bytes(&mut line, &mut len, b"--------");
    }
    push_bytes(&mut line, &mut len, b"  ");

    // 8. Name
    push_colored_name(
        &mut line,
        &mut len,
        name,
        stat_opt.map(|s| s.st_mode).unwrap_or(if is_dir { S_IFDIR | 0o755 } else { S_IFREG | 0o644 }),
        is_dir && path_len > 1,
    );
    push_bytes(&mut line, &mut len, b"\n");
    write_bytes(&line[..len]);
}

pub fn write_colored_name(name: &[u8], mode: u32, append_slash: bool) {
    let mut buf = [0u8; PATH_MAX + 32];
    let mut len = 0usize;
    push_colored_name(&mut buf, &mut len, name, mode, append_slash);
    write_bytes(&buf[..len]);
}

pub fn parse_tree_dirents(buf: &[u8], parent_path: &[u8; PATH_MAX], parent_len: usize, depth: usize) {
    let mut off = 0usize;
    while off + DIRENT64_HEADER_SIZE <= buf.len() {
        let reclen = u16::from_le_bytes([buf[off + 16], buf[off + 17]]) as usize;
        if reclen == 0 || off + reclen > buf.len() {
            break;
        }
        let dtype = buf[off + 18];
        let name_start = off + DIRENT64_HEADER_SIZE;
        let mut name_end = name_start;
        while name_end < off + reclen && buf[name_end] != 0 {
            name_end += 1;
        }
        let name = &buf[name_start..name_end];
        if !name.is_empty() && !bytes_eq(name, b".") && !bytes_eq(name, b"..") {
            write_tree_indent(depth);
            write_bytes(name);
            if dtype == DT_DIR {
                write_all(b"/");
            }
            write_all(b"\n");

            if dtype == DT_DIR && depth + 1 < TREE_MAX_DEPTH {
                let mut child = [0u8; PATH_MAX];
                if let Some(child_len) =
                    append_path_component(parent_path, parent_len, name, &mut child)
                {
                    tree_walk(&child, child_len, depth + 1);
                }
            }
        }
        off += reclen;
    }
}

pub fn write_tree_indent(depth: usize) {
    let mut i = 0usize;
    while i <= depth {
        write_all(b"  ");
        i += 1;
    }
}

