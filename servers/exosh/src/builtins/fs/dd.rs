//! Block-copy utility (`dd`).
#![allow(dead_code, unused_imports)]
use exo_syscall_abi as syscall;
use crate::builtins::*;
use crate::types::*;
use crate::util::*;
use crate::terminal::*;
use crate::exec::*;

pub fn cmd_dd(rest: &[u8], state: &ShellState) {
    let mut input_arg: &[u8] = b"";
    let mut output_arg: &[u8] = b"";
    let mut block_size = DD_DEFAULT_BS;
    let mut count: Option<u64> = None;

    let mut args = rest;
    loop {
        let (arg, tail) = next_arg(args);
        if arg.is_empty() {
            break;
        }
        if let Some(value) = strip_prefix(arg, b"if=") {
            input_arg = value;
        } else if let Some(value) = strip_prefix(arg, b"of=") {
            output_arg = value;
        } else if let Some(value) = strip_prefix(arg, b"bs=") {
            let Some(parsed) = parse_size(value) else {
                write_all(b"dd: invalid bs\n");
                return;
            };
            block_size = parsed.max(1);
        } else if let Some(value) = strip_prefix(arg, b"count=") {
            let Some(parsed) = parse_u64(value) else {
                write_all(b"dd: invalid count\n");
                return;
            };
            count = Some(parsed);
        } else {
            write_all(b"dd: unknown operand: ");
            write_bytes(arg);
            write_all(b"\n");
            return;
        }
        args = tail;
    }

    if input_arg.is_empty() || output_arg.is_empty() {
        write_all(
            b"dd: usage: dd if=<path|/dev/zero|/dev/urandom> of=<path|/dev/null> [bs=1M] [count=N]\n",
        );
        return;
    }
    if (is_dev_zero(input_arg) || is_dev_urandom(input_arg)) && count.is_none() {
        write_all(b"dd: count required with generated input\n");
        return;
    }

    let input_zero = is_dev_zero(input_arg);
    let input_random = is_dev_urandom(input_arg);
    let output_null = is_dev_null(output_arg);
    let mut input_fd = -1i64;
    let mut output_fd = -1i64;

    if !input_zero && !input_random {
        let mut path = [0u8; PATH_MAX];
        if absolute_path(state.cwd(), input_arg, &mut path).is_none() {
            write_all(b"dd: input path too long\n");
            return;
        }
        input_fd = unsafe {
            syscall::syscall4(
                syscall::SYS_OPENAT,
                AT_FDCWD,
                path.as_ptr() as u64,
                syscall::O_RDONLY,
                0,
            )
        };
        if input_fd < 0 {
            print_errno(b"dd", input_fd);
            return;
        }
    }

    if !output_null {
        let mut path = [0u8; PATH_MAX];
        if absolute_path(state.cwd(), output_arg, &mut path).is_none() {
            write_all(b"dd: output path too long\n");
            if input_fd >= 0 {
                let _ = close_fd(input_fd);
            }
            return;
        }
        output_fd = unsafe {
            syscall::syscall4(
                syscall::SYS_OPENAT,
                AT_FDCWD,
                path.as_ptr() as u64,
                syscall::O_CREAT | syscall::O_WRONLY | syscall::O_TRUNC,
                0o644,
            )
        };
        if output_fd < 0 {
            print_errno(b"dd", output_fd);
            if input_fd >= 0 {
                let _ = close_fd(input_fd);
            }
            return;
        }
    }

    let start = monotonic_ns();
    if input_zero {
        let Some(max_blocks) = count else {
            write_all(b"dd: count required with /dev/zero\n");
            return;
        };
        let Some(total_bytes) = block_size.checked_mul(max_blocks) else {
            write_all(b"dd: size overflow\n");
            if output_fd >= 0 {
                let _ = close_fd(output_fd);
            }
            return;
        };

        if !output_null {
            let rc = ftruncate_fd(output_fd, total_bytes);
            if rc < 0 {
                print_errno(b"dd", rc);
                let _ = close_fd(output_fd);
                return;
            }
            let _ = close_fd(output_fd);
        }

        let elapsed = match (start, monotonic_ns()) {
            (Some(a), Some(b)) if b >= a => b - a,
            _ => 0,
        };
        print_dd_summary(total_bytes, elapsed);
        return;
    }

    let buf = unsafe { &mut *DD_IO_BUFFER.0.get() };
    let mut total = 0u64;
    let mut blocks = 0u64;
    let mut eof = false;

    loop {
        if let Some(max_blocks) = count {
            if blocks >= max_blocks {
                break;
            }
        }

        let mut remaining = block_size;
        let mut copied_any = false;
        while remaining > 0 {
            let chunk = remaining.min(buf.len() as u64) as usize;
            let n = if input_random {
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
                        input_fd as u64,
                        buf.as_mut_ptr() as u64,
                        chunk as u64,
                    )
                }
            };

            if n < 0 {
                print_errno(b"dd", n);
                eof = true;
                break;
            }
            if n == 0 {
                eof = true;
                break;
            }

            let n_usize = n as usize;
            if !output_null {
                let rc = write_fd_all(output_fd, &buf[..n_usize]);
                if rc < 0 {
                    print_errno(b"dd", rc);
                    eof = true;
                    break;
                }
            }
            total = total.saturating_add(n as u64);
            remaining = remaining.saturating_sub(n as u64);
            copied_any = true;

            if !input_random && n_usize < chunk {
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

    if input_fd >= 0 {
        let _ = close_fd(input_fd);
    }
    if output_fd >= 0 {
        let _ = close_fd(output_fd);
    }

    let elapsed = match (start, monotonic_ns()) {
        (Some(a), Some(b)) if b >= a => b - a,
        _ => 0,
    };
    print_dd_summary(total, elapsed);
}

pub fn print_dd_summary(total: u64, elapsed: u64) {
    write_u64(total);
    write_all(b" bytes copied in ");
    write_duration_ms(elapsed);
    write_all(b" -> ");
    write_mib_per_sec(total, elapsed);
    write_all(b" MiB/s\n");
}

