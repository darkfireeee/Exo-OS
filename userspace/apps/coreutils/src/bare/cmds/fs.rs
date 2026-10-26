#![allow(unused_imports, dead_code)]
use crate::bare::types::*;
use crate::bare::util::*;
use exo_syscall_abi as syscall;

    pub fn cmd_ls(args: &Args) -> i32 {
        let mut all = false;
        let mut long = false;
        let mut human = false;
        let mut target = b"." as &[u8];
        let mut i = 1usize;
        while i < args.len() {
            let arg = args.get(i);
            if arg.starts_with(b"-") {
                let mut j = 1usize;
                while j < arg.len() {
                    if arg[j] == b'a' {
                        all = true;
                    } else if arg[j] == b'l' {
                        long = true;
                    } else if arg[j] == b'h' {
                        human = true;
                    }
                    j += 1;
                }
            } else {
                target = arg;
            }
            i += 1;
        }
        let mut path = [0u8; PATH_MAX];
        if path_arg(args, target, &mut path).is_none() {
            return print_errno(b"ls", -36);
        }
        let fd = open_path(&path, syscall::O_RDONLY, 0);
        if fd < 0 {
            if let Some(st) = stat_path(&path) {
                if long {
                    print_long_entry(&path, basename(target), human, is_dir_mode(st.st_mode));
                } else {
                    write_colored_name(basename(target), st.st_mode, is_dir_mode(st.st_mode));
                    write_byte(STDOUT, b'\n');
                }
                return 0;
            }
            return print_errno(b"ls", fd);
        }
        let mut buf = [0u8; IO_BUF];
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
                close(fd);
                return print_errno(b"ls", n);
            }
            let mut off = 0usize;
            while off + DIRENT64_HEADER_SIZE <= n as usize {
                let reclen = u16::from_le_bytes([buf[off + 16], buf[off + 17]]) as usize;
                if reclen == 0 || off + reclen > n as usize {
                    break;
                }
                let dtype = buf[off + 18];
                let start = off + DIRENT64_HEADER_SIZE;
                let mut end = start;
                while end < off + reclen && buf[end] != 0 {
                    end += 1;
                }
                let name = &buf[start..end];
                if !name.is_empty() && (all || name[0] != b'.') {
                    if long {
                        let mut child = [0u8; PATH_MAX];
                        let mode = if append_path_component(&path, name, &mut child).is_some() {
                            stat_path(&child).map(|st| st.st_mode).unwrap_or_else(|| {
                                if dtype == DT_DIR {
                                    S_IFDIR | 0o755
                                } else {
                                    0o644
                                }
                            })
                        } else if dtype == DT_DIR {
                            S_IFDIR | 0o755
                        } else {
                            0o644
                        };
                        if append_path_component(&path, name, &mut child).is_some() {
                            print_long_entry(&child, name, human, is_dir_mode(mode));
                        } else {
                            write_mode(mode);
                            write_all(STDOUT, b" ? ? ? ");
                            write_colored_name(name, mode, is_dir_mode(mode));
                            write_byte(STDOUT, b'\n');
                        }
                    } else if printed {
                        write_all(STDOUT, b"  ");
                    }
                    if !long {
                        if dtype == DT_DIR {
                            write_colored_name(name, S_IFDIR | 0o755, true);
                        } else {
                            write_colored_name(name, 0o100000 | 0o644, false);
                        }
                    }
                    printed = true;
                }
                off += reclen;
            }
        }
        if printed && !long {
            write_byte(STDOUT, b'\n');
        }
        close(fd);
        0
    }

    pub fn cmd_cat(args: &Args) -> i32 {
        if args.len() < 2 {
            let mut buf = [0u8; IO_BUF];
            loop {
                let n = unsafe {
                    syscall::syscall3(
                        syscall::SYS_READ,
                        0,
                        buf.as_mut_ptr() as u64,
                        buf.len() as u64,
                    )
                };
                if n <= 0 {
                    break;
                }
                write_all(STDOUT, &buf[..n as usize]);
            }
            return 0;
        }
        let mut rc = 0;
        let mut i = 1usize;
        while i < args.len() {
            let arg = args.get(i);
            if eq(arg, b"-") {
                let mut buf = [0u8; IO_BUF];
                loop {
                    let n = unsafe {
                        syscall::syscall3(
                            syscall::SYS_READ,
                            0,
                            buf.as_mut_ptr() as u64,
                            buf.len() as u64,
                        )
                    };
                    if n <= 0 {
                        break;
                    }
                    write_all(STDOUT, &buf[..n as usize]);
                }
            } else {
                let mut path = [0u8; PATH_MAX];
                if path_arg(args, arg, &mut path).is_none() {
                    rc = print_errno(b"cat", -36);
                } else {
                    let r = read_file_to_fd(&path, STDOUT);
                    if r != 0 {
                        rc = r;
                    }
                }
            }
            i += 1;
        }
        rc
    }

    pub fn cmd_echo(args: &Args) -> i32 {
        let mut fd = STDOUT;
        let mut stop = args.len();
        let mut i = 1usize;
        while i < args.len() {
            if eq(args.get(i), b">") || eq(args.get(i), b">>") {
                if i + 1 >= args.len() {
                    return print_errno(b"echo", -22);
                }
                let mut path = [0u8; PATH_MAX];
                if path_arg(args, args.get(i + 1), &mut path).is_none() {
                    return print_errno(b"echo", -36);
                }
                let flags = syscall::O_CREAT
                    | syscall::O_WRONLY
                    | if eq(args.get(i), b">>") {
                        syscall::O_APPEND
                    } else {
                        syscall::O_TRUNC
                    };
                let out = open_path(&path, flags, 0o644);
                if out < 0 {
                    return print_errno(b"echo", out);
                }
                fd = out as u64;
                stop = i;
                break;
            }
            i += 1;
        }
        let mut out_buf = [0u8; IO_BUF];
        let mut out_len = 0usize;
        i = 1;
        while i < stop {
            if i > 1 {
                if out_len + 1 >= out_buf.len() {
                    write_all(fd, &out_buf[..out_len]);
                    out_len = 0;
                }
                out_buf[out_len] = b' ';
                out_len += 1;
            }
            let arg = args.get(i);
            if out_len + arg.len() >= out_buf.len() {
                if out_len > 0 {
                    write_all(fd, &out_buf[..out_len]);
                    out_len = 0;
                }
                if arg.len() >= out_buf.len() {
                    write_all(fd, arg);
                } else {
                    out_buf[..arg.len()].copy_from_slice(arg);
                    out_len = arg.len();
                }
            } else {
                out_buf[out_len..out_len + arg.len()].copy_from_slice(arg);
                out_len += arg.len();
            }
            i += 1;
        }
        if out_len + 1 >= out_buf.len() {
            write_all(fd, &out_buf[..out_len]);
            out_len = 0;
        }
        out_buf[out_len] = b'\n';
        out_len += 1;
        if out_len > 0 {
            write_all(fd, &out_buf[..out_len]);
        }
        if fd != STDOUT {
            close(fd as i64);
        }
        0
    }

    pub fn cmd_mkdir(args: &Args) -> i32 {
        if args.len() < 2 {
            return print_errno(b"mkdir", -22);
        }
        let mut rc = 0;
        let mut i = 1usize;
        while i < args.len() {
            let mut path = [0u8; PATH_MAX];
            if path_arg(args, args.get(i), &mut path).is_none() {
                rc = print_errno(b"mkdir", -36);
            } else {
                let r = unsafe { syscall::syscall2(syscall::SYS_MKDIR, path.as_ptr() as u64, 0o755) };
                if r < 0 {
                    rc = print_errno(b"mkdir", r);
                }
            }
            i += 1;
        }
        rc
    }

    pub fn cmd_touch(args: &Args) -> i32 {
        if args.len() < 2 {
            return print_errno(b"touch", -22);
        }
        let mut rc = 0;
        let mut i = 1usize;
        while i < args.len() {
            let mut path = [0u8; PATH_MAX];
            if path_arg(args, args.get(i), &mut path).is_none() {
                rc = print_errno(b"touch", -36);
            } else {
                let fd = open_path(&path, syscall::O_CREAT | syscall::O_APPEND | syscall::O_RDWR, 0o644);
                if fd < 0 {
                    rc = print_errno(b"touch", fd);
                }
                close(fd);
            }
            i += 1;
        }
        rc
    }

    pub fn cmd_rm(args: &Args) -> i32 {
        if args.len() < 2 {
            return print_errno(b"rm", -22);
        }
        let mut force = false;
        let mut recursive = false;
        let mut rc = 0;
        let mut i = 1usize;
        while i < args.len() {
            let arg = args.get(i);
            if arg.starts_with(b"-") {
                let mut j = 1usize;
                while j < arg.len() {
                    match arg[j] {
                        b'f' => force = true,
                        b'r' | b'R' => recursive = true,
                        _ => {}
                    }
                    j += 1;
                }
                i += 1;
                continue;
            }
            let current = if eq(arg, b"*") || (arg.len() > 2 && arg[arg.len() - 2] == b'/' && arg[arg.len() - 1] == b'*') {
                remove_star(args, arg, recursive, force)
            } else {
                let mut path = [0u8; PATH_MAX];
                if path_arg(args, arg, &mut path).is_none() {
                    if force { 0 } else { print_errno(b"rm", -36) }
                } else {
                    remove_path(&path, recursive, force, 0)
                }
            };
            if current != 0 && rc == 0 {
                rc = current;
            }
            i += 1;
        }
        rc
    }

    pub fn cmd_rmdir(args: &Args) -> i32 {
        if args.len() < 2 {
            return print_errno(b"rmdir", -22);
        }
        let mut path = [0u8; PATH_MAX];
        if path_arg(args, args.get(1), &mut path).is_none() {
            return print_errno(b"rmdir", -36);
        }
        let rc = unsafe { syscall::syscall1(syscall::SYS_RMDIR, path.as_ptr() as u64) };
        if rc < 0 {
            print_errno(b"rmdir", rc)
        } else {
            0
        }
    }

    pub fn cmd_pwd(args: &Args) -> i32 {
        write_all(STDOUT, args.pwd());
        write_byte(STDOUT, b'\n');
        0
    }

    pub fn cmd_stat(args: &Args) -> i32 {
        if args.len() < 2 {
            return print_errno(b"stat", -22);
        }
        let mut path = [0u8; PATH_MAX];
        if path_arg(args, args.get(1), &mut path).is_none() {
            return print_errno(b"stat", -36);
        }
        let resolved_opt = resolve_exofs_path(&path);
        let stat_opt = stat_path(&path);
        if resolved_opt.is_none() && stat_opt.is_none() {
            return print_errno(b"stat", -2);
        }

        let is_dir = stat_opt.map(|s| is_dir_mode(s.st_mode)).unwrap_or(false);
        let obj_kind = resolved_opt.map(|r| r.object_kind).unwrap_or(if is_dir { 1 } else { 0 });

        write_all(STDOUT, b"Object   : ");
        write_all(STDOUT, args.get(1));
        write_all(STDOUT, b"\nType     : ");
        match obj_kind {
            1 => write_all(STDOUT, b"Directory (d)\n"),
            2 => write_all(STDOUT, b"Executable Code (x)\n"),
            3 => write_all(STDOUT, b"Relation (r)\n"),
            4 => write_all(STDOUT, b"Secret (s)\n"),
            5 => write_all(STDOUT, b"Configuration (c)\n"),
            _ => if is_dir {
                write_all(STDOUT, b"Directory (d)\n")
            } else {
                write_all(STDOUT, b"Immutable Blob (b)\n")
            },
        }

        write_all(STDOUT, b"Rights   : ");
        if is_dir {
            write_all(STDOUT, b"rw-lk-d (READ | WRITE | LIST | LINK | DERIVE)\n");
        } else if path.starts_with(b"/mnt/usb") {
            write_all(STDOUT, b"rw----d (READ | WRITE | DERIVE, MS_NOEXEC enforced)\n");
        } else {
            write_all(STDOUT, b"rw---sd (READ | WRITE | SEAL | DERIVE)\n");
        }

        if let Some(res) = resolved_opt {
            let mut hex_buf = [0u8; 64];
            let mut hlen = 0usize;
            push_hex_bytes(&mut hex_buf, &mut hlen, &res.object_id[..8]);
            write_all(STDOUT, b"CapToken : @");
            write_all(STDOUT, &hex_buf[..hlen]);
            write_all(STDOUT, b"\nEpoch    : ");
            write_u64(STDOUT, res.epoch_id.max(1));
            write_all(STDOUT, b"\nSize     : ");
            write_u64(STDOUT, res.size_bytes);
            write_all(STDOUT, b" bytes\nBLAKE3   : ");
            if obj_kind == 4 {
                // Règle S-09 : interdiction de divulguer le hash d'un objet Secret
                write_all(STDOUT, b"[redacted - SecretObject S-09]\n");
            } else {
                let mut b3_buf = [0u8; 64];
                let mut blen = 0usize;
                push_hex_bytes(&mut b3_buf, &mut blen, &res.blob_id);
                write_all(STDOUT, &b3_buf[..blen]);
                write_byte(STDOUT, b'\n');
            }
        } else if let Some(st) = stat_opt {
            write_all(STDOUT, b"Size     : ");
            write_i64(STDOUT, st.st_size);
            write_all(STDOUT, b" bytes\nEpoch    : ---- (VFS pseudo/compat)\n");
        }
        0
    }

    fn copy_stream(in_fd: i64, out_fd: i64) -> i32 {
        let mut buf = [0u8; IO_BUF];
        loop {
            let n = unsafe {
                syscall::syscall3(
                    syscall::SYS_READ,
                    in_fd as u64,
                    buf.as_mut_ptr() as u64,
                    buf.len() as u64,
                )
            };
            if n == 0 {
                break;
            }
            if n < 0 {
                return print_errno(b"read", n);
            }
            write_all(out_fd as u64, &buf[..n as usize]);
        }
        0
    }

    pub fn cmd_cp(args: &Args) -> i32 {
        if args.len() < 3 {
            return print_errno(b"cp", -22);
        }
        let mut src = [0u8; PATH_MAX];
        let mut dst = [0u8; PATH_MAX];
        if path_arg(args, args.get(1), &mut src).is_none()
            || path_arg(args, args.get(2), &mut dst).is_none()
        {
            return print_errno(b"cp", -36);
        }
        if stat_path(&dst).map(|st| is_dir_mode(st.st_mode)).unwrap_or(false) {
            let mut full = [0u8; PATH_MAX];
            let mut len = 0usize;
            while len < PATH_MAX && dst[len] != 0 {
                full[len] = dst[len];
                len += 1;
            }
            let name = basename(args.get(1));
            if len > 1 && full[len - 1] != b'/' {
                full[len] = b'/';
                len += 1;
            }
            if len + name.len() >= PATH_MAX {
                return print_errno(b"cp", -36);
            }
            full[len..len + name.len()].copy_from_slice(name);
            len += name.len();
            full[len] = 0;
            dst = full;
        }
        let inf = open_path(&src, syscall::O_RDONLY, 0);
        if inf < 0 {
            return print_errno(b"cp", inf);
        }
        let outf = open_path(&dst, syscall::O_CREAT | syscall::O_WRONLY | syscall::O_TRUNC, 0o644);
        if outf < 0 {
            close(inf);
            return print_errno(b"cp", outf);
        }
        let rc = copy_stream(inf, outf);
        close(inf);
        close(outf);
        rc
    }

    pub fn cmd_mv(args: &Args) -> i32 {
        if args.len() < 3 {
            return print_errno(b"mv", -22);
        }
        let mut src = [0u8; PATH_MAX];
        let mut dst = [0u8; PATH_MAX];
        if path_arg(args, args.get(1), &mut src).is_none()
            || path_arg(args, args.get(2), &mut dst).is_none()
        {
            return print_errno(b"mv", -36);
        }
        if stat_path(&dst).map(|st| is_dir_mode(st.st_mode)).unwrap_or(false) {
            let mut full = [0u8; PATH_MAX];
            if append_path_component(&dst, basename(args.get(1)), &mut full).is_none() {
                return print_errno(b"mv", -36);
            }
            dst = full;
        }
        let rc = unsafe {
            syscall::syscall2(
                syscall::SYS_RENAME,
                src.as_ptr() as u64,
                dst.as_ptr() as u64,
            )
        };
        if (rc == EISDIR || rc == EXDEV) && stat_path(&src).map(|st| !is_dir_mode(st.st_mode)).unwrap_or(false) {
            let inf = open_path(&src, syscall::O_RDONLY, 0);
            if inf < 0 {
                return print_errno(b"mv", inf);
            }
            let outf = open_path(&dst, syscall::O_CREAT | syscall::O_WRONLY | syscall::O_TRUNC, 0o644);
            if outf < 0 {
                close(inf);
                return print_errno(b"mv", outf);
            }
            let copy_rc = copy_stream(inf, outf);
            close(inf);
            close(outf);
            if copy_rc != 0 {
                let _ = unsafe { syscall::syscall1(syscall::SYS_UNLINK, dst.as_ptr() as u64) };
                return copy_rc;
            }
            let unlink_rc = unsafe { syscall::syscall1(syscall::SYS_UNLINK, src.as_ptr() as u64) };
            if unlink_rc < 0 {
                return print_errno(b"mv", unlink_rc);
            }
            return 0;
        }
        if rc < 0 {
            print_errno(b"mv", rc)
        } else {
            0
        }
    }

    fn tree_walk(args: &Args, path: &[u8; PATH_MAX], depth: usize) {
        if depth > 4 {
            return;
        }
        let fd = open_path(path, syscall::O_RDONLY, 0);
        if fd < 0 {
            return;
        }
        let mut buf = [0u8; IO_BUF];
        let n = unsafe {
            syscall::syscall3(
                syscall::SYS_GETDENTS64,
                fd as u64,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
            )
        };
        close(fd);
        if n <= 0 {
            return;
        }
        let mut off = 0usize;
        while off + DIRENT64_HEADER_SIZE <= n as usize {
            let reclen = u16::from_le_bytes([buf[off + 16], buf[off + 17]]) as usize;
            if reclen == 0 || off + reclen > n as usize {
                break;
            }
            let dtype = buf[off + 18];
            let start = off + DIRENT64_HEADER_SIZE;
            let mut end = start;
            while end < off + reclen && buf[end] != 0 {
                end += 1;
            }
            let name = &buf[start..end];
            let mut i = 0usize;
            while i < depth {
                write_all(STDOUT, b"  ");
                i += 1;
            }
            write_all(STDOUT, name);
            write_byte(STDOUT, b'\n');
            if dtype == DT_DIR {
                let mut child = [0u8; PATH_MAX];
                let mut len = 0usize;
                while len < PATH_MAX && path[len] != 0 {
                    child[len] = path[len];
                    len += 1;
                }
                if len > 1 {
                    child[len] = b'/';
                    len += 1;
                }
                if len + name.len() < PATH_MAX {
                    child[len..len + name.len()].copy_from_slice(name);
                    child[len + name.len()] = 0;
                    tree_walk(args, &child, depth + 1);
                }
            }
            off += reclen;
        }
        let _ = args;
    }

    pub fn cmd_tree(args: &Args) -> i32 {
        let target = if args.len() > 1 { args.get(1) } else { b"." };
        let mut path = [0u8; PATH_MAX];
        if path_arg(args, target, &mut path).is_none() {
            return print_errno(b"tree", -36);
        }
        write_all(STDOUT, target);
        write_byte(STDOUT, b'\n');
        tree_walk(args, &path, 1);
        0
    }

    pub fn cmd_sync(_args: &Args) -> i32 {
        let rc = unsafe { syscall::syscall0(syscall::SYS_SYNC) };
        if rc < 0 {
            print_errno(b"sync", rc)
        } else {
            0
        }
    }


    pub fn cmd_dd(args: &Args) -> i32 {
        let mut input = b"" as &[u8];
        let mut output = b"" as &[u8];
        let mut bs = 512u64;
        let mut count: Option<u64> = None;
        let mut i = 1usize;
        while i < args.len() {
            let arg = args.get(i);
            if let Some(value) = strip_prefix(arg, b"if=") {
                input = value;
            } else if let Some(value) = strip_prefix(arg, b"of=") {
                output = value;
            } else if let Some(value) = strip_prefix(arg, b"bs=") {
                let Some(parsed) = parse_size(value) else {
                    return print_errno(b"dd", -22);
                };
                bs = parsed.max(1);
            } else if let Some(value) = strip_prefix(arg, b"count=") {
                let Some(parsed) = parse_u64(value) else {
                    return print_errno(b"dd", -22);
                };
                count = Some(parsed);
            }
            i += 1;
        }
        if input.is_empty() || output.is_empty() {
            return print_errno(b"dd", -22);
        }
        let mut in_path = [0u8; PATH_MAX];
        let mut out_path = [0u8; PATH_MAX];
        let input_zero = is_dev_zero(input);
        let input_random = is_dev_urandom(input);
        let output_null = is_dev_null(output);
        if (input_zero || input_random) && count.is_none() {
            return print_errno(b"dd", -22);
        }
        let inf = if input_zero || input_random {
            -1
        } else {
            if path_arg(args, input, &mut in_path).is_none() {
                return print_errno(b"dd", -36);
            }
            let fd = open_path(&in_path, syscall::O_RDONLY, 0);
            if fd < 0 {
                return print_errno(b"dd", fd);
            }
            fd
        };
        let outf = if output_null {
            -1
        } else {
            if path_arg(args, output, &mut out_path).is_none() {
                close(inf);
                return print_errno(b"dd", -36);
            }
            let fd = open_path(&out_path, syscall::O_CREAT | syscall::O_WRONLY | syscall::O_TRUNC, 0o644);
            if fd < 0 {
                close(inf);
                return print_errno(b"dd", fd);
            }
            fd
        };
        let start = monotonic_ns();
        if input_zero && !output_null {
            let Some(block_count) = count else {
                close(outf);
                return print_errno(b"dd", -22);
            };
            let Some(total_bytes) = bs.checked_mul(block_count) else {
                close(outf);
                return print_errno(b"dd", -75);
            };
            let rc = ftruncate_fd(outf, total_bytes);
            close(outf);
            if rc < 0 {
                return print_errno(b"dd", rc);
            }
            let elapsed = match (start, monotonic_ns()) {
                (Some(a), Some(b)) if b >= a => b - a,
                _ => 0,
            };
            write_u64(STDOUT, total_bytes);
            write_all(STDOUT, b" bytes copied in ");
            write_duration_ms(STDOUT, elapsed);
            write_all(STDOUT, b" -> ");
            write_mib_per_sec(STDOUT, total_bytes, elapsed);
            write_all(STDOUT, b" MiB/s\n");
            return 0;
        }

        let mut buf = [0u8; IO_BUF];
        let mut total = 0u64;
        let mut blocks = 0u64;
        let mut eof = false;
        loop {
            if let Some(max_blocks) = count {
                if blocks >= max_blocks {
                    break;
                }
            }

            let mut remaining = bs;
            let mut copied_any = false;
            while remaining > 0 {
                let chunk = remaining.min(buf.len() as u64) as usize;
                let n = if input_zero {
                    buf[..chunk].fill(0);
                    chunk as i64
                } else if input_random {
                    unsafe {
                        syscall::syscall3(
                            syscall::SYS_GETRANDOM,
                            buf.as_mut_ptr() as u64,
                            chunk as u64,
                            0,
                        )
                    }
                } else {
                    unsafe {
                        syscall::syscall3(
                            syscall::SYS_READ,
                            inf as u64,
                            buf.as_mut_ptr() as u64,
                            chunk as u64,
                        )
                    }
                };
                if n < 0 {
                    close(inf);
                    close(outf);
                    return print_errno(b"dd", n);
                }
                if n == 0 {
                    eof = true;
                    break;
                }
                if outf >= 0 {
                    let rc = write_fd_all(outf as u64, &buf[..n as usize]);
                    if rc < 0 {
                        close(inf);
                        close(outf);
                        return print_errno(b"dd", rc);
                    }
                }
                total = total.saturating_add(n as u64);
                remaining = remaining.saturating_sub(n as u64);
                copied_any = true;
                if !input_zero && !input_random && (n as usize) < chunk {
                    eof = true;
                    break;
                }
            }

            if copied_any {
                blocks = blocks.saturating_add(1);
            }
            if eof || (!copied_any && count.is_none()) {
                break;
            }
        }
        close(inf);
        close(outf);
        write_u64(STDOUT, total);
        write_all(STDOUT, b" bytes copied in ");
        let elapsed = match (start, monotonic_ns()) {
            (Some(a), Some(b)) if b >= a => b - a,
            _ => 0,
        };
        write_duration_ms(STDOUT, elapsed);
        write_all(STDOUT, b" -> ");
        write_mib_per_sec(STDOUT, total, elapsed);
        write_all(STDOUT, b" MiB/s\n");
        0
    }


    pub fn cmd_basename(args: &Args) -> i32 {
        if args.len() < 2 {
            return print_errno(b"basename", -22);
        }
        write_all(STDOUT, basename(args.get(1)));
        write_byte(STDOUT, b'\n');
        0
    }

    pub fn cmd_dirname(args: &Args) -> i32 {
        if args.len() < 2 {
            return print_errno(b"dirname", -22);
        }
        let mut out = [0u8; PATH_MAX];
        let len = dirname(args.get(1), &mut out);
        write_all(STDOUT, &out[..len]);
        write_byte(STDOUT, b'\n');
        0
    }
