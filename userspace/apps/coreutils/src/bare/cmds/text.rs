#![allow(unused_imports, dead_code)]
use crate::bare::types::*;
use crate::bare::util::*;
use exo_syscall_abi as syscall;

    fn count_wc_fd(fd: u64) -> (u64, u64, u64, i32) {
        let mut buf = [0u8; IO_BUF];
        let mut bytes = 0u64;
        let mut lines = 0u64;
        let mut words = 0u64;
        let mut in_word = false;
        loop {
            let n = unsafe {
                syscall::syscall3(
                    syscall::SYS_READ,
                    fd,
                    buf.as_mut_ptr() as u64,
                    buf.len() as u64,
                )
            };
            if n == 0 {
                break;
            }
            if n < 0 {
                return (lines, words, bytes, n as i32);
            }
            let mut i = 0usize;
            while i < n as usize {
                let b = buf[i];
                bytes += 1;
                if b == b'\n' {
                    lines += 1;
                }
                if b == b' ' || b == b'\n' || b == b'\t' || b == b'\r' {
                    in_word = false;
                } else if !in_word {
                    words += 1;
                    in_word = true;
                }
                i += 1;
            }
        }
        (lines, words, bytes, 0)
    }

    pub fn cmd_wc(args: &Args) -> i32 {
        if args.len() < 2 || eq(args.get(1), b"-") {
            let (lines, words, bytes, rc) = count_wc_fd(0);
            if rc < 0 {
                return print_errno(b"wc", rc as i64);
            }
            write_u64(STDOUT, lines);
            write_byte(STDOUT, b' ');
            write_u64(STDOUT, words);
            write_byte(STDOUT, b' ');
            write_u64(STDOUT, bytes);
            write_byte(STDOUT, b'\n');
            return 0;
        }
        let mut path = [0u8; PATH_MAX];
        if path_arg(args, args.get(1), &mut path).is_none() {
            return print_errno(b"wc", -36);
        }
        let fd = open_path(&path, syscall::O_RDONLY, 0);
        if fd < 0 {
            return print_errno(b"wc", fd);
        }
        let (lines, words, bytes, rc) = count_wc_fd(fd as u64);
        close(fd);
        if rc < 0 {
            return print_errno(b"wc", rc as i64);
        }
        write_u64(STDOUT, lines);
        write_byte(STDOUT, b' ');
        write_u64(STDOUT, words);
        write_byte(STDOUT, b' ');
        write_u64(STDOUT, bytes);
        write_byte(STDOUT, b'\n');
        0
    }

    fn to_lower_byte(b: u8) -> u8 {
        if b >= b'A' && b <= b'Z' {
            b + (b'a' - b'A')
        } else {
            b
        }
    }

    fn line_contains_sub(line: &[u8], pat: &[u8], ignore_case: bool) -> bool {
        if pat.is_empty() {
            return true;
        }
        if line.len() < pat.len() {
            return false;
        }
        let mut i = 0usize;
        while i + pat.len() <= line.len() {
            let mut matched = true;
            let mut j = 0usize;
            while j < pat.len() {
                let lb = if ignore_case { to_lower_byte(line[i + j]) } else { line[i + j] };
                let pb = if ignore_case { to_lower_byte(pat[j]) } else { pat[j] };
                if lb != pb {
                    matched = false;
                    break;
                }
                j += 1;
            }
            if matched {
                return true;
            }
            i += 1;
        }
        false
    }

    fn grep_fd(fd: u64, pat: &[u8], ignore_case: bool, line_num: bool, invert: bool) -> bool {
        let mut buf = [0u8; IO_BUF];
        let mut line_buf = [0u8; 1024];
        let mut line_len = 0usize;
        let mut current_line = 1u64;
        let mut matched_any = false;

        loop {
            let n = unsafe {
                syscall::syscall3(
                    syscall::SYS_READ,
                    fd,
                    buf.as_mut_ptr() as u64,
                    buf.len() as u64,
                )
            };
            if n <= 0 {
                break;
            }
            let mut i = 0usize;
            while i < n as usize {
                let b = buf[i];
                if b == b'\n' {
                    let has_match = line_contains_sub(&line_buf[..line_len], pat, ignore_case);
                    if has_match ^ invert {
                        matched_any = true;
                        if line_num {
                            write_u64(STDOUT, current_line);
                            write_byte(STDOUT, b':');
                        }
                        write_all(STDOUT, &line_buf[..line_len]);
                        write_byte(STDOUT, b'\n');
                    }
                    line_len = 0;
                    current_line += 1;
                } else if line_len < 1024 {
                    line_buf[line_len] = b;
                    line_len += 1;
                }
                i += 1;
            }
        }
        if line_len > 0 {
            let has_match = line_contains_sub(&line_buf[..line_len], pat, ignore_case);
            if has_match ^ invert {
                matched_any = true;
                if line_num {
                    write_u64(STDOUT, current_line);
                    write_byte(STDOUT, b':');
                }
                write_all(STDOUT, &line_buf[..line_len]);
                write_byte(STDOUT, b'\n');
            }
        }
        matched_any
    }

    pub fn cmd_grep(args: &Args) -> i32 {
        if args.len() < 2 {
            write_all(STDERR, b"usage: grep [-i] [-n] [-v] PATTERN [FILE...]\n");
            return 2;
        }

        let mut opt_ignore_case = false;
        let mut opt_line_num = false;
        let mut opt_invert = false;
        let mut pattern: Option<&[u8]> = None;
        let mut file_start = args.len();

        let mut i = 1usize;
        while i < args.len() {
            let arg = args.get(i);
            if !arg.is_empty() && arg[0] == b'-' && arg.len() > 1 && pattern.is_none() {
                let mut j = 1usize;
                while j < arg.len() {
                    match arg[j] {
                        b'i' => opt_ignore_case = true,
                        b'n' => opt_line_num = true,
                        b'v' => opt_invert = true,
                        _ => {}
                    }
                    j += 1;
                }
            } else if pattern.is_none() {
                pattern = Some(arg);
                file_start = i + 1;
            }
            i += 1;
        }

        let Some(pat) = pattern else {
            write_all(STDERR, b"grep: missing pattern\n");
            return 2;
        };

        let mut match_found = false;

        if file_start >= args.len() {
            if grep_fd(0, pat, opt_ignore_case, opt_line_num, opt_invert) {
                match_found = true;
            }
        } else {
            let mut f_idx = file_start;
            while f_idx < args.len() {
                let file_arg = args.get(f_idx);
                if eq(file_arg, b"-") {
                    if grep_fd(0, pat, opt_ignore_case, opt_line_num, opt_invert) {
                        match_found = true;
                    }
                } else {
                    let mut path = [0u8; PATH_MAX];
                    if path_arg(args, file_arg, &mut path).is_some() {
                        let fd = open_path(&path, syscall::O_RDONLY, 0);
                        if fd >= 0 {
                            if grep_fd(fd as u64, pat, opt_ignore_case, opt_line_num, opt_invert) {
                                match_found = true;
                            }
                            close(fd);
                        } else {
                            print_errno(b"grep", fd);
                        }
                    }
                }
                f_idx += 1;
            }
        }

        if match_found { 0 } else { 1 }
    }
