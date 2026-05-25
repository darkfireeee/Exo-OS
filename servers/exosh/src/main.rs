#![no_std]
#![no_main]

use core::cell::UnsafeCell;
use core::panic::PanicInfo;
use exo_syscall_abi as syscall;

const STDIN: u64 = 0;
const STDOUT: u64 = 1;
const AT_FDCWD: u64 = (-100i64) as u64;
const PATH_MAX: usize = 256;
const LINE_MAX: usize = 256;
const IO_BUF: usize = 2048;
const DD_IO_BUF: usize = 1024 * 1024;
const SHELL_WRITE_CHUNK: usize = 64 * 1024;
const FS_BENCH_CHUNK: usize = 64 * 1024;
const DIRENT64_HEADER_SIZE: usize = 24;
const DT_DIR: u8 = 4;
const TREE_MAX_DEPTH: usize = 4;
const RM_MAX_DEPTH: usize = 8;
const HISTORY_MAX: usize = 16;
const CLOCK_MONOTONIC: u64 = 1;
const DD_DEFAULT_BS: u64 = 512;
const S_IFMT: u32 = 0o170000;
const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;
const ANSI_RESET: &[u8] = b"\x1b[0m";
const ANSI_DIR: &[u8] = b"\x1b[1;34m";
const ANSI_EXEC: &[u8] = b"\x1b[1;32m";
const ANSI_REVERSE: &[u8] = b"\x1b[7m";
const SIGTERM: u64 = 15;
const SIGKILL: u64 = 9;
const AF_INET: u16 = 2;
const SOCK_STREAM: u64 = 1;
const SOCK_RAW: u64 = 3;
const IPPROTO_ICMP: u64 = 1;
const EAGAIN: i64 = -11;
const PING_IDENT: u16 = 0x4558;
const ICMP_ECHO_HEADER_LEN: usize = 8;
const PING_PAYLOAD: &[u8] = b"exoos-ping";
const PING_PACKET_LEN: usize = ICMP_ECHO_HEADER_LEN + PING_PAYLOAD.len();
const PING_RECV_ATTEMPTS: usize = 1000;

struct DdIoBuffer(UnsafeCell<[u8; DD_IO_BUF]>);

unsafe impl Sync for DdIoBuffer {}

static DD_IO_BUFFER: DdIoBuffer = DdIoBuffer(UnsafeCell::new([0; DD_IO_BUF]));

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Timespec {
    tv_sec: i64,
    tv_nsec: i64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxStat {
    st_dev: u64,
    st_ino: u64,
    st_nlink: u64,
    st_mode: u32,
    st_uid: u32,
    st_gid: u32,
    __pad0: u32,
    st_rdev: u64,
    st_size: i64,
    st_blksize: i64,
    st_blocks: i64,
    st_atim: Timespec,
    st_mtim: Timespec,
    st_ctim: Timespec,
    __unused: [i64; 3],
}

#[derive(Clone, Copy, Default)]
struct LsOptions {
    long: bool,
    all: bool,
    human: bool,
}

#[derive(Clone, Copy, Default)]
struct RmOptions {
    force: bool,
    recursive: bool,
}

#[derive(Clone, Copy)]
struct ShellState {
    cwd: [u8; PATH_MAX],
    cwd_len: usize,
    history: [[u8; LINE_MAX]; HISTORY_MAX],
    history_len: [usize; HISTORY_MAX],
    history_count: usize,
    history_next: usize,
}

impl ShellState {
    const fn new() -> Self {
        let mut cwd = [0u8; PATH_MAX];
        cwd[0] = b'/';
        Self {
            cwd,
            cwd_len: 1,
            history: [[0u8; LINE_MAX]; HISTORY_MAX],
            history_len: [0usize; HISTORY_MAX],
            history_count: 0,
            history_next: 0,
        }
    }

    fn cwd(&self) -> &[u8] {
        &self.cwd[..self.cwd_len]
    }

    fn set_cwd(&mut self, path: &[u8]) {
        let len = path.len().min(PATH_MAX - 1);
        self.cwd[..len].copy_from_slice(&path[..len]);
        self.cwd[len] = 0;
        self.cwd_len = len;
    }

    fn remember(&mut self, command: &[u8]) {
        let command = trim(command);
        if command.is_empty() {
            return;
        }
        if let Some(last) = self.history_at_offset(0) {
            if bytes_eq(last, command) {
                return;
            }
        }

        let len = command.len().min(LINE_MAX - 1);
        let idx = self.history_next;
        self.history[idx][..len].copy_from_slice(&command[..len]);
        self.history_len[idx] = len;
        self.history_next = (self.history_next + 1) % HISTORY_MAX;
        if self.history_count < HISTORY_MAX {
            self.history_count += 1;
        }
    }

    fn history_at_offset(&self, offset: usize) -> Option<&[u8]> {
        if offset >= self.history_count {
            return None;
        }
        let idx = (self.history_next + HISTORY_MAX - 1 - offset) % HISTORY_MAX;
        Some(&self.history[idx][..self.history_len[idx]])
    }
}

#[no_mangle]
pub extern "C" fn _start(_boot_info_virt: usize) -> ! {
    register_readiness_endpoint();

    let mut state = ShellState::new();
    write_all(b"\x0cExo-OS userspace console ready\n");
    write_all(b"Services launched by init_server. Type 'help' for commands.\n\n");

    loop {
        let mut line = [0u8; LINE_MAX];
        let len = read_line(&mut line, &mut state);
        let command = trim(&line[..len]);
        if command.is_empty() {
            continue;
        }
        state.remember(command);
        run_commands(command, &mut state);
    }
}

fn register_readiness_endpoint() {
    let pid = unsafe { syscall::syscall0(syscall::SYS_GETPID) };
    if pid <= 0 {
        return;
    }

    let endpoint = ((pid as u64) << 32) | 1;
    let name = b"exosh";
    let _ = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            endpoint,
        )
    };
}

fn run_commands(line: &[u8], state: &mut ShellState) {
    let mut start = 0usize;
    let mut i = 0usize;
    while i <= line.len() {
        if i == line.len() || line[i] == b';' {
            let part = trim(&line[start..i]);
            if !part.is_empty() {
                run_command(part, state);
            }
            start = i.saturating_add(1);
        }
        i = i.saturating_add(1);
    }
}

fn run_command(line: &[u8], state: &mut ShellState) {
    let (cmd, rest) = first_token(line);
    if bytes_eq(cmd, b"help") {
        cmd_help();
    } else if bytes_eq(cmd, b"pwd") {
        write_bytes(state.cwd());
        write_all(b"\n");
    } else if bytes_eq(cmd, b"cd") {
        cmd_cd(rest, state);
    } else if bytes_eq(cmd, b"ls") {
        cmd_ls(rest, state);
    } else if bytes_eq(cmd, b"mkdir") {
        cmd_mkdir(rest, state);
    } else if bytes_eq(cmd, b"touch") {
        cmd_touch(rest, state);
    } else if bytes_eq(cmd, b"cat") {
        cmd_cat(rest, state);
    } else if bytes_eq(cmd, b"echo") {
        cmd_echo(rest, state);
    } else if bytes_eq(cmd, b"rm") {
        cmd_rm(rest, state);
    } else if bytes_eq(cmd, b"cp") {
        cmd_cp(rest, state);
    } else if bytes_eq(cmd, b"mv") {
        cmd_mv(rest, state);
    } else if bytes_eq(cmd, b"rmdir") {
        cmd_rmdir(rest, state);
    } else if bytes_eq(cmd, b"tree") {
        cmd_tree(rest, state);
    } else if bytes_eq(cmd, b"history") {
        cmd_history(state);
    } else if bytes_eq(cmd, b"time") {
        cmd_time(rest, state);
    } else if bytes_eq(cmd, b"dd") {
        cmd_dd(rest, state);
    } else if bytes_eq(cmd, b"sync") {
        cmd_sync();
    } else if bytes_eq(cmd, b"ping") {
        cmd_ping(rest);
    } else if bytes_eq(cmd, b"tcping") {
        cmd_tcping(rest);
    } else if bytes_eq(cmd, b"bench") {
        cmd_bench(rest);
    } else if bytes_eq(cmd, b"top") || bytes_eq(cmd, b"ps") {
        cmd_top();
    } else if bytes_eq(cmd, b"kill") {
        cmd_kill(rest);
    } else if bytes_eq(cmd, b"clear") {
        write_all(b"\x0c");
    } else if bytes_eq(cmd, b"exit") {
        write_all(b"exosh: exit requested; init_server may restart the shell\n");
        unsafe {
            let _ = syscall::syscall1(syscall::SYS_EXIT, 0);
        }
    } else {
        write_all(b"exosh: unknown command: ");
        write_bytes(cmd);
        write_all(b"\n");
    }
}

fn cmd_help() {
    write_all(b"Commands:\n");
    write_all(
        b"  help clear pwd cd ls mkdir touch cat echo rm cp mv rmdir tree top ps kill history time dd sync ping tcping bench exit\n",
    );
    write_all(b"Examples:\n");
    write_all(b"  ls -lah /tmp ; rm -rf /tmp/t ; history\n");
    write_all(b"  time echo test ; dd if=/dev/urandom of=/tmp/bench bs=1M count=4\n");
    write_all(b"  ping 127.0.0.1 4\n");
    write_all(b"  tcping 127.0.0.1 80\n");
    write_all(b"  top ; kill <pid> ; kill -9 <pid>\n");
}

fn cmd_cd(rest: &[u8], state: &mut ShellState) {
    let (arg, _) = next_arg(rest);
    let target = if arg.is_empty() { b"/".as_slice() } else { arg };
    let mut path = [0u8; PATH_MAX];
    let Some(len) = absolute_path(state.cwd(), target, &mut path) else {
        write_all(b"cd: path too long\n");
        return;
    };

    let rc = unsafe { syscall::syscall1(syscall::SYS_CHDIR, path.as_ptr() as u64) };
    if rc == 0 {
        state.set_cwd(&path[..len]);
    } else {
        print_errno(b"cd", rc);
    }
}

fn cmd_ls(rest: &[u8], state: &ShellState) {
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
            write_colored_name(
                basename(&path[..path_len]),
                stat.st_mode,
                is_dir_mode(stat.st_mode) && path_len > 1,
            );
            write_all(b"\n");
        } else {
            print_errno(b"ls", fd);
        }
        return;
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
            print_errno(b"ls", n);
            break;
        }
        parse_ls_dirents(&buf[..n as usize], &path, path_len, opts, &mut printed);
    }
    if printed {
        write_all(b"\n");
    }
    let _ = close_fd(fd);
}

fn cmd_mkdir(rest: &[u8], state: &ShellState) {
    let (arg, _) = next_arg(rest);
    if arg.is_empty() {
        write_all(b"mkdir: missing path\n");
        return;
    }
    let mut path = [0u8; PATH_MAX];
    if absolute_path(state.cwd(), arg, &mut path).is_none() {
        write_all(b"mkdir: path too long\n");
        return;
    }
    let rc = unsafe { syscall::syscall2(syscall::SYS_MKDIR, path.as_ptr() as u64, 0o755) };
    if rc < 0 {
        print_errno(b"mkdir", rc);
    }
}

fn cmd_touch(rest: &[u8], state: &ShellState) {
    let mut args = rest;
    if trim(args).is_empty() {
        write_all(b"touch: missing path\n");
        return;
    }

    loop {
        let (arg, tail) = next_arg(args);
        if arg.is_empty() {
            break;
        }
        touch_one_arg(arg, state);
        args = tail;
    }
}

fn touch_one_arg(arg: &[u8], state: &ShellState) {
    let mut path = [0u8; PATH_MAX];
    if absolute_path(state.cwd(), arg, &mut path).is_none() {
        write_all(b"touch: path too long\n");
        return;
    }
    let fd = unsafe {
        syscall::syscall4(
            syscall::SYS_OPENAT,
            AT_FDCWD,
            path.as_ptr() as u64,
            syscall::O_CREAT | syscall::O_RDWR,
            0o644,
        )
    };
    if fd < 0 {
        print_errno(b"touch", fd);
    } else {
        let _ = close_fd(fd);
    }
}

fn cmd_cat(rest: &[u8], state: &ShellState) {
    let (arg, _) = next_arg(rest);
    if arg.is_empty() {
        write_all(b"cat: missing path\n");
        return;
    }
    let mut path = [0u8; PATH_MAX];
    if absolute_path(state.cwd(), arg, &mut path).is_none() {
        write_all(b"cat: path too long\n");
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
        print_errno(b"cat", fd);
        return;
    }

    let mut buf = [0u8; 512];
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
            print_errno(b"cat", n);
            break;
        }
        write_bytes(&buf[..n as usize]);
    }
    let _ = close_fd(fd);
}

fn cmd_echo(rest: &[u8], state: &ShellState) {
    let body = trim(rest);
    if let Some(redir) = find_byte(body, b'>') {
        let text = trim_end(&body[..redir]);
        let path_arg = trim(&body[redir + 1..]);
        if path_arg.is_empty() {
            write_all(b"echo: missing redirection path\n");
            return;
        }
        let mut path = [0u8; PATH_MAX];
        if absolute_path(state.cwd(), path_arg, &mut path).is_none() {
            write_all(b"echo: path too long\n");
            return;
        }
        let fd = unsafe {
            syscall::syscall4(
                syscall::SYS_OPENAT,
                AT_FDCWD,
                path.as_ptr() as u64,
                syscall::O_CREAT | syscall::O_WRONLY | syscall::O_TRUNC,
                0o644,
            )
        };
        if fd < 0 {
            print_errno(b"echo", fd);
            return;
        }
        let rc = unsafe {
            syscall::syscall3(
                syscall::SYS_WRITE,
                fd as u64,
                text.as_ptr() as u64,
                text.len() as u64,
            )
        };
        if rc >= 0 {
            let nl = [b'\n'];
            let _ =
                unsafe { syscall::syscall3(syscall::SYS_WRITE, fd as u64, nl.as_ptr() as u64, 1) };
        } else {
            print_errno(b"echo", rc);
        }
        let _ = close_fd(fd);
    } else {
        write_bytes(body);
        write_all(b"\n");
    }
}

fn cmd_rm(rest: &[u8], state: &ShellState) {
    let (opts, mut args) = parse_rm_options(rest);
    if trim(args).is_empty() {
        write_all(b"rm: missing path\n");
        return;
    }

    loop {
        let (arg, tail) = next_arg(args);
        if arg.is_empty() {
            break;
        }
        rm_one_arg(arg, state, opts);
        args = tail;
    }
}

fn cmd_cp(rest: &[u8], state: &ShellState) {
    let (src_arg, tail) = next_arg(rest);
    let (dst_arg, _) = next_arg(tail);
    if src_arg.is_empty() || dst_arg.is_empty() {
        write_all(b"cp: usage: cp <src> <dst>\n");
        return;
    }

    let mut src = [0u8; PATH_MAX];
    if absolute_path(state.cwd(), src_arg, &mut src).is_none() {
        write_all(b"cp: source path too long\n");
        return;
    }
    let mut dst = [0u8; PATH_MAX];
    let Some(mut dst_len) = absolute_path(state.cwd(), dst_arg, &mut dst) else {
        write_all(b"cp: destination path too long\n");
        return;
    };
    if is_dir_path(&dst) {
        let mut full_dst = [0u8; PATH_MAX];
        let Some(len) = append_path_component(&dst, dst_len, basename(&src_arg), &mut full_dst)
        else {
            write_all(b"cp: destination path too long\n");
            return;
        };
        dst = full_dst;
        dst_len = len;
    }
    let _ = dst_len;
    if is_dir_path(&src) {
        write_all(b"cp: omitting directory\n");
        return;
    }

    let in_fd = unsafe {
        syscall::syscall4(
            syscall::SYS_OPENAT,
            AT_FDCWD,
            src.as_ptr() as u64,
            syscall::O_RDONLY,
            0,
        )
    };
    if in_fd < 0 {
        print_errno(b"cp", in_fd);
        return;
    }

    let out_fd = unsafe {
        syscall::syscall4(
            syscall::SYS_OPENAT,
            AT_FDCWD,
            dst.as_ptr() as u64,
            syscall::O_CREAT | syscall::O_WRONLY | syscall::O_TRUNC,
            0o644,
        )
    };
    if out_fd < 0 {
        print_errno(b"cp", out_fd);
        let _ = close_fd(in_fd);
        return;
    }

    let mut buf = [0u8; 512];
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
            print_errno(b"cp", n);
            break;
        }
        let rc = write_fd_all(out_fd, &buf[..n as usize]);
        if rc < 0 {
            print_errno(b"cp", rc);
            break;
        }
    }

    let _ = close_fd(out_fd);
    let _ = close_fd(in_fd);
}

fn cmd_mv(rest: &[u8], state: &ShellState) {
    let (src_arg, tail) = next_arg(rest);
    let (dst_arg, _) = next_arg(tail);
    if src_arg.is_empty() || dst_arg.is_empty() {
        write_all(b"mv: usage: mv <src> <dst>\n");
        return;
    }

    let mut src = [0u8; PATH_MAX];
    if absolute_path(state.cwd(), src_arg, &mut src).is_none() {
        write_all(b"mv: source path too long\n");
        return;
    }
    let mut dst = [0u8; PATH_MAX];
    let Some(mut dst_len) = absolute_path(state.cwd(), dst_arg, &mut dst) else {
        write_all(b"mv: destination path too long\n");
        return;
    };
    if is_dir_path(&dst) {
        let mut full_dst = [0u8; PATH_MAX];
        let Some(len) = append_path_component(&dst, dst_len, basename(&src_arg), &mut full_dst)
        else {
            write_all(b"mv: destination path too long\n");
            return;
        };
        dst = full_dst;
        dst_len = len;
    }
    let _ = dst_len;

    let rc = unsafe {
        syscall::syscall2(
            syscall::SYS_RENAME,
            src.as_ptr() as u64,
            dst.as_ptr() as u64,
        )
    };
    if rc < 0 {
        print_errno(b"mv", rc);
    }
}

fn cmd_rmdir(rest: &[u8], state: &ShellState) {
    let (arg, _) = next_arg(rest);
    if arg.is_empty() {
        write_all(b"rmdir: missing path\n");
        return;
    }
    let mut path = [0u8; PATH_MAX];
    if absolute_path(state.cwd(), arg, &mut path).is_none() {
        write_all(b"rmdir: path too long\n");
        return;
    }
    let rc = unsafe { syscall::syscall1(syscall::SYS_RMDIR, path.as_ptr() as u64) };
    if rc < 0 {
        print_errno(b"rmdir", rc);
    }
}

fn cmd_tree(rest: &[u8], state: &ShellState) {
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

fn tree_walk(path: &[u8; PATH_MAX], path_len: usize, depth: usize) {
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

fn cmd_top() {
    write_all(b"PID  NAME              STATE\n");
    let mut entries = [syscall::ExoProcessInfo::zeroed(); 64];
    let count = process_list_snapshot(&mut entries);
    if count > 0 {
        let mut idx = 0usize;
        while idx < count as usize && idx < entries.len() {
            let entry = entries[idx];
            write_u32(entry.pid);
            if entry.pid < 10 {
                write_all(b"    ");
            } else {
                write_all(b"   ");
            }
            write_padded(process_entry_name(&entry), 17);
            write_process_state(entry.state);
            write_all(b"\n");
            idx += 1;
        }
        return;
    }

    write_all(b"process list unavailable\n");
}

fn process_list_snapshot(entries: &mut [syscall::ExoProcessInfo]) -> i64 {
    unsafe {
        syscall::syscall3(
            syscall::SYS_EXO_PROCESS_LIST,
            entries.as_mut_ptr() as u64,
            entries.len() as u64,
            core::mem::size_of::<syscall::ExoProcessInfo>() as u64,
        )
    }
}

fn process_entry_name(entry: &syscall::ExoProcessInfo) -> &[u8] {
    let mut len = 0usize;
    while len < entry.name.len() && entry.name[len] != 0 {
        len += 1;
    }
    if len == 0 {
        b"user_process"
    } else {
        &entry.name[..len]
    }
}

fn write_process_state(state: u32) {
    match state {
        0 => write_all(b"creating"),
        1 => write_all(b"running"),
        2 => write_all(b"sleeping"),
        3 => write_all(b"stopped"),
        4 => write_all(b"zombie"),
        5 => write_all(b"dead"),
        _ => write_all(b"unknown"),
    }
}

fn cmd_time(rest: &[u8], state: &mut ShellState) {
    let command = trim(rest);
    if command.is_empty() {
        write_all(b"time: usage: time <command>\n");
        return;
    }

    let start = monotonic_ns();
    run_command(command, state);
    let end = monotonic_ns();
    match (start, end) {
        (Some(a), Some(b)) if b >= a => {
            write_all(b"real ");
            write_duration_ms(b - a);
            write_all(b"\n");
        }
        _ => write_all(b"real unavailable\n"),
    }
}

fn cmd_dd(rest: &[u8], state: &ShellState) {
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

fn print_dd_summary(total: u64, elapsed: u64) {
    write_u64(total);
    write_all(b" bytes copied in ");
    write_duration_ms(elapsed);
    write_all(b" -> ");
    write_mib_per_sec(total, elapsed);
    write_all(b" MiB/s\n");
}

fn cmd_kill(rest: &[u8]) {
    let (first, tail) = next_arg(rest);
    if first.is_empty() {
        write_all(b"kill: usage: kill [-9] <pid|service>\n");
        return;
    }

    let mut signal = SIGTERM;
    let mut target = first;
    if first.len() > 1 && first[0] == b'-' {
        signal = if bytes_eq(&first[1..], b"9") {
            SIGKILL
        } else {
            match parse_u32(&first[1..]) {
                Some(sig) => sig as u64,
                None => {
                    write_all(b"kill: invalid signal\n");
                    return;
                }
            }
        };
        let (next, _) = next_arg(tail);
        target = next;
    }

    if target.is_empty() {
        write_all(b"kill: missing target\n");
        return;
    }

    let Some(pid) = parse_u32(target).or_else(|| service_pid(target)) else {
        write_all(b"kill: unknown target\n");
        return;
    };
    if pid == 1 {
        write_all(b"kill: refusing to kill init_server (pid 1)\n");
        return;
    }

    let rc = unsafe { syscall::syscall2(syscall::SYS_KILL, pid as u64, signal) };
    if rc < 0 {
        print_errno(b"kill", rc);
    } else {
        write_all(b"kill: signal sent to pid ");
        write_u32(pid);
        write_all(b"\n");
    }
}

fn cmd_history(state: &ShellState) {
    let count = state.history_count.min(HISTORY_MAX);
    let mut i = count;
    while i > 0 {
        let offset = i - 1;
        let number = count - offset;
        let mut out = [0u8; LINE_MAX + 16];
        let mut len = 0usize;
        push_u32(&mut out, &mut len, number as u32);
        push_bytes(&mut out, &mut len, b"  ");
        if let Some(line) = state.history_at_offset(offset) {
            push_bytes(&mut out, &mut len, line);
        }
        push_bytes(&mut out, &mut len, b"\n");
        write_bytes(&out[..len]);
        i -= 1;
    }
}

fn read_line(line: &mut [u8; LINE_MAX], state: &mut ShellState) -> usize {
    let mut len = 0usize;
    let mut cursor = 0usize;
    let mut history_offset: Option<usize> = None;
    let mut visible_len = 0usize;
    let mut cursor_visible = true;
    render_input_line(line, len, cursor, state, &mut visible_len, cursor_visible);

    loop {
        let Some(byte) = read_byte_poll() else {
            sleep_ms(25);
            continue;
        };
        let had_visible_cursor = cursor_visible;
        cursor_visible = true;
        let mut redraw = true;
        match byte {
            b'\n' | b'\r' => {
                render_input_line(line, len, cursor, state, &mut visible_len, false);
                write_all(b"\n");
                return len;
            }
            0x03 => {
                render_input_line(line, len, cursor, state, &mut visible_len, false);
                write_all(b"^C\n");
                return 0;
            }
            0x04 => {
                render_input_line(line, len, cursor, state, &mut visible_len, false);
                write_all(b"^D\n");
                return 0;
            }
            0x0C => {
                write_all(b"\x0c");
                visible_len = 0;
            }
            0x1B => {
                handle_escape_sequence(line, &mut len, &mut cursor, &mut history_offset, state);
            }
            0x08 | 0x7F => {
                if cursor > 0 {
                    delete_before_cursor(line, &mut len, &mut cursor);
                } else {
                    redraw = false;
                }
            }
            b'\t' => {
                let append_tail = had_visible_cursor && cursor == len;
                if insert_input_byte(line, &mut len, &mut cursor, b' ') {
                    history_offset = None;
                    if append_tail {
                        render_input_append(b' ');
                        visible_len = len.saturating_add(1);
                        redraw = false;
                    }
                } else {
                    redraw = false;
                }
            }
            b if b.is_ascii_graphic() || b == b' ' => {
                let append_tail = had_visible_cursor && cursor == len;
                if insert_input_byte(line, &mut len, &mut cursor, b) {
                    history_offset = None;
                    if append_tail {
                        render_input_append(b);
                        visible_len = len.saturating_add(1);
                        redraw = false;
                    }
                } else {
                    redraw = false;
                }
            }
            _ => redraw = false,
        }
        if redraw {
            render_input_line(line, len, cursor, state, &mut visible_len, cursor_visible);
        }
    }
}

fn cmd_ping(rest: &[u8]) {
    let (host, tail) = next_arg(rest);
    if host.is_empty() {
        write_all(b"ping: usage: ping <ipv4> [count]\n");
        return;
    }
    let Some(addr) = parse_ipv4(host) else {
        write_all(b"ping: invalid IPv4 address\n");
        return;
    };
    let (count_arg, _) = next_arg(tail);
    let count = if count_arg.is_empty() {
        4
    } else {
        parse_u64(count_arg).unwrap_or(4).min(32)
    };

    let mut fd = -1i64;
    let mut attempts = 0usize;
    while attempts < 3 {
        fd = unsafe {
            syscall::syscall3(syscall::SYS_SOCKET, AF_INET as u64, SOCK_RAW, IPPROTO_ICMP)
        };
        if fd >= 0 {
            break;
        }
        sleep_ms(50);
        attempts += 1;
    }
    if fd < 0 {
        print_errno(b"ping socket", fd);
        return;
    }

    let bind_addr = sockaddr_in([0, 0, 0, 0], PING_IDENT);
    let bind_rc = unsafe {
        syscall::syscall3(
            syscall::SYS_BIND,
            fd as u64,
            bind_addr.as_ptr() as u64,
            bind_addr.len() as u64,
        )
    };
    if bind_rc < 0 {
        print_errno(b"ping bind", bind_rc);
        let _ = close_fd(fd);
        return;
    }

    let sockaddr = sockaddr_in(addr, 0);
    let rc = unsafe {
        syscall::syscall3(
            syscall::SYS_CONNECT,
            fd as u64,
            sockaddr.as_ptr() as u64,
            sockaddr.len() as u64,
        )
    };
    if rc < 0 {
        print_errno(b"ping connect", rc);
        let _ = close_fd(fd);
        return;
    }

    write_all(b"PING ");
    write_bytes(host);
    write_all(b" via ExoNet V4\n");

    let mut seq = 0u64;
    while seq < count {
        let Some(start) = monotonic_ns() else {
            write_all(b"ping: clock unavailable\n");
            break;
        };

        let packet = icmp_echo_request(seq as u16);
        let send_rc = unsafe {
            syscall::syscall6(
                syscall::SYS_SENDTO,
                fd as u64,
                packet.as_ptr() as u64,
                packet.len() as u64,
                0,
                0,
                0,
            )
        };
        if send_rc < 0 {
            print_errno(b"ping sendto", send_rc);
            break;
        }

        let mut recv_buf = [0u8; 64];
        let mut from = [0u8; 16];
        let mut recv_rc = EAGAIN;
        let mut attempts = 0usize;
        while attempts < PING_RECV_ATTEMPTS {
            let mut from_len = from.len() as u32;
            recv_rc = unsafe {
                syscall::syscall6(
                    syscall::SYS_RECVFROM,
                    fd as u64,
                    recv_buf.as_mut_ptr() as u64,
                    recv_buf.len() as u64,
                    0,
                    from.as_mut_ptr() as u64,
                    &mut from_len as *mut u32 as u64,
                )
            };
            if recv_rc != EAGAIN {
                break;
            }
            sleep_ms(10);
            attempts += 1;
        }
        let elapsed = monotonic_ns().unwrap_or(start).saturating_sub(start);
        if recv_rc == EAGAIN {
            write_all(b"ping: timeout waiting for ICMP echo reply\n");
            break;
        }
        if recv_rc < 0 {
            print_errno(b"ping recvfrom", recv_rc);
            break;
        }
        if !icmp_echo_reply_matches(&recv_buf[..recv_rc as usize], seq as u16) {
            write_all(b"ping: unexpected ICMP packet\n");
            break;
        }

        write_u64(recv_rc as u64);
        write_all(b" bytes from ");
        write_ipv4(addr);
        write_all(b": seq=");
        write_u64(seq);
        write_all(b" time=");
        write_u64(elapsed / 1_000);
        write_all(b"us\n");

        seq = seq.saturating_add(1);
        if seq < count {
            sleep_ms(250);
        }
    }

    let _ = close_fd(fd);
}

fn icmp_echo_request(seq: u16) -> [u8; PING_PACKET_LEN] {
    let mut packet = [0u8; PING_PACKET_LEN];
    packet[0] = 8;
    packet[4..6].copy_from_slice(&PING_IDENT.to_be_bytes());
    packet[6..8].copy_from_slice(&seq.to_be_bytes());
    packet[ICMP_ECHO_HEADER_LEN..].copy_from_slice(PING_PAYLOAD);
    let checksum = internet_checksum(&packet);
    packet[2..4].copy_from_slice(&checksum.to_be_bytes());
    packet
}

fn icmp_echo_reply_matches(packet: &[u8], seq: u16) -> bool {
    packet.len() >= ICMP_ECHO_HEADER_LEN
        && packet[0] == 0
        && packet[1] == 0
        && u16::from_be_bytes([packet[4], packet[5]]) == PING_IDENT
        && u16::from_be_bytes([packet[6], packet[7]]) == seq
        && internet_checksum(packet) == 0
}

fn internet_checksum(bytes: &[u8]) -> u16 {
    let mut sum = 0u32;
    let mut idx = 0usize;
    while idx + 1 < bytes.len() {
        sum = sum.wrapping_add(u16::from_be_bytes([bytes[idx], bytes[idx + 1]]) as u32);
        idx += 2;
    }
    if idx < bytes.len() {
        sum = sum.wrapping_add((bytes[idx] as u32) << 8);
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

fn cmd_tcping(rest: &[u8]) {
    let (host, tail) = next_arg(rest);
    if host.is_empty() {
        write_all(b"tcping: usage: tcping <ipv4> [port]\n");
        return;
    }
    let Some(addr) = parse_ipv4(host) else {
        write_all(b"tcping: invalid IPv4 address\n");
        return;
    };
    let (port_arg, _) = next_arg(tail);
    let port = if port_arg.is_empty() {
        80
    } else {
        let Some(parsed) = parse_u64(port_arg) else {
            write_all(b"tcping: invalid port\n");
            return;
        };
        if parsed == 0 || parsed > u16::MAX as u64 {
            write_all(b"tcping: invalid port\n");
            return;
        }
        parsed as u16
    };

    let mut fd = -1i64;
    let mut attempts = 0usize;
    while attempts < 3 {
        fd = unsafe { syscall::syscall3(syscall::SYS_SOCKET, AF_INET as u64, SOCK_STREAM, 0) };
        if fd >= 0 {
            break;
        }
        sleep_ms(50);
        attempts += 1;
    }
    if fd < 0 {
        print_errno(b"tcping socket", fd);
        return;
    }

    let sockaddr = sockaddr_in(addr, port);
    let Some(start) = monotonic_ns() else {
        write_all(b"tcping: clock unavailable\n");
        let _ = close_fd(fd);
        return;
    };
    let rc = unsafe {
        syscall::syscall3(
            syscall::SYS_CONNECT,
            fd as u64,
            sockaddr.as_ptr() as u64,
            sockaddr.len() as u64,
        )
    };
    let elapsed = monotonic_ns().unwrap_or(start).saturating_sub(start);
    if rc < 0 {
        write_all(b"tcping ");
        write_ipv4(addr);
        write_all(b":");
        write_u64(port as u64);
        write_all(b" failed after ");
        write_u64(elapsed / 1_000);
        write_all(b"us");
        print_errno(b" connect", rc);
        let _ = close_fd(fd);
        return;
    }

    write_all(b"tcping ");
    write_ipv4(addr);
    write_all(b":");
    write_u64(port as u64);
    write_all(b" connected time=");
    write_u64(elapsed / 1_000);
    write_all(b"us\n");

    let _ = close_fd(fd);
}

fn handle_escape_sequence(
    line: &mut [u8; LINE_MAX],
    len: &mut usize,
    cursor: &mut usize,
    history_offset: &mut Option<usize>,
    state: &ShellState,
) {
    let Some(second) = read_byte_wait_ms(25) else {
        return;
    };
    if second != b'[' {
        return;
    }
    let Some(third) = read_byte_wait_ms(25) else {
        return;
    };

    match third {
        b'A' => {
            if state.history_count == 0 {
                return;
            }
            let next = match *history_offset {
                Some(offset) if offset + 1 < state.history_count => offset + 1,
                Some(offset) => offset,
                None => 0,
            };
            *history_offset = Some(next);
            if let Some(history) = state.history_at_offset(next) {
                replace_input_line(line, len, cursor, history);
            }
        }
        b'B' => match *history_offset {
            Some(offset) if offset > 0 => {
                let next = offset - 1;
                *history_offset = Some(next);
                if let Some(history) = state.history_at_offset(next) {
                    replace_input_line(line, len, cursor, history);
                }
            }
            Some(_) => {
                *history_offset = None;
                replace_input_line(line, len, cursor, b"");
            }
            None => {}
        },
        b'C' => {
            if *cursor < *len {
                *cursor += 1;
            }
        }
        b'D' => {
            if *cursor > 0 {
                *cursor -= 1;
            }
        }
        _ => {}
    }
}

fn replace_input_line(
    line: &mut [u8; LINE_MAX],
    len: &mut usize,
    cursor: &mut usize,
    new_line: &[u8],
) {
    let new_len = new_line.len().min(LINE_MAX - 1);
    line[..new_len].copy_from_slice(&new_line[..new_len]);
    *len = new_len;
    *cursor = new_len;
}

fn insert_input_byte(
    line: &mut [u8; LINE_MAX],
    len: &mut usize,
    cursor: &mut usize,
    byte: u8,
) -> bool {
    if *len + 1 >= LINE_MAX {
        return false;
    }
    let mut i = *len;
    while i > *cursor {
        line[i] = line[i - 1];
        i -= 1;
    }
    line[*cursor] = byte;
    *len += 1;
    *cursor += 1;
    true
}

fn delete_before_cursor(line: &mut [u8; LINE_MAX], len: &mut usize, cursor: &mut usize) {
    if *cursor == 0 || *len == 0 {
        return;
    }
    let start = *cursor - 1;
    let mut i = start;
    while i + 1 < *len {
        line[i] = line[i + 1];
        i += 1;
    }
    *len -= 1;
    *cursor -= 1;
}

fn render_input_line(
    line: &[u8; LINE_MAX],
    len: usize,
    cursor: usize,
    state: &ShellState,
    visible_len: &mut usize,
    show_cursor: bool,
) {
    write_all(b"\r");
    prompt(state);
    let cursor = cursor.min(len);
    write_bytes(&line[..cursor]);
    if show_cursor {
        write_all(ANSI_REVERSE);
        if cursor < len {
            write_bytes(&line[cursor..cursor + 1]);
        } else {
            write_all(b" ");
        }
        write_all(ANSI_RESET);
        if cursor < len {
            write_bytes(&line[cursor + 1..len]);
        }
    } else {
        write_bytes(&line[cursor..len]);
    }

    let next_visible_len = if show_cursor && cursor == len {
        len.saturating_add(1)
    } else {
        len
    };
    if *visible_len > next_visible_len {
        let mut i = 0usize;
        while i < *visible_len - next_visible_len {
            write_all(b" ");
            i += 1;
        }
    }
    *visible_len = next_visible_len;
}

fn render_input_append(byte: u8) {
    write_all(b"\x08");
    write_byte(byte);
    write_all(ANSI_REVERSE);
    write_all(b" ");
    write_all(ANSI_RESET);
}

fn read_byte_poll() -> Option<u8> {
    let mut byte = [0u8; 1];
    let rc = unsafe {
        syscall::syscall3(
            syscall::SYS_READ,
            STDIN,
            byte.as_mut_ptr() as u64,
            byte.len() as u64,
        )
    };
    if rc == 1 {
        Some(byte[0])
    } else {
        None
    }
}

fn read_byte_wait_ms(ms: u64) -> Option<u8> {
    let mut waited = 0u64;
    while waited < ms {
        if let Some(byte) = read_byte_poll() {
            return Some(byte);
        }
        sleep_ms(1);
        waited += 1;
    }
    None
}

fn prompt(state: &ShellState) {
    write_all(b"exosh:");
    write_bytes(state.cwd());
    write_all(b"$ ");
}

fn parse_ls_args(mut rest: &[u8]) -> (LsOptions, &[u8]) {
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

fn parse_ls_dirents(
    buf: &[u8],
    parent_path: &[u8; PATH_MAX],
    parent_len: usize,
    opts: LsOptions,
    printed: &mut bool,
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
                let mut child = [0u8; PATH_MAX];
                if let Some(child_len) =
                    append_path_component(parent_path, parent_len, name, &mut child)
                {
                    print_long_entry(&child, child_len, name, opts);
                }
            } else {
                if *printed {
                    write_all(b"  ");
                }
                if dtype == DT_DIR {
                    write_colored_name(name, S_IFDIR | 0o755, true);
                } else {
                    write_colored_name(name, S_IFREG | 0o644, false);
                }
            }
            *printed = true;
        }
        off += reclen;
    }
}

fn print_long_entry(path: &[u8; PATH_MAX], path_len: usize, name: &[u8], opts: LsOptions) {
    let Some(stat) = stat_path(path) else {
        write_all(b"?????????? ? ? ? ");
        write_bytes(name);
        write_all(b"\n");
        return;
    };
    write_mode(stat.st_mode);
    write_all(b" ");
    write_u64(stat.st_nlink);
    write_all(b" ");
    write_u32(stat.st_uid);
    write_all(b" ");
    write_u32(stat.st_gid);
    write_all(b" ");
    if opts.human {
        write_human_size(stat.st_size);
    } else {
        write_i64(stat.st_size);
    }
    write_all(b" ");
    write_colored_name(
        name,
        stat.st_mode,
        is_dir_mode(stat.st_mode) && path_len > 1,
    );
    write_all(b"\n");
}

fn write_colored_name(name: &[u8], mode: u32, append_slash: bool) {
    if is_dir_mode(mode) {
        write_all(ANSI_DIR);
        write_bytes(name);
        if append_slash {
            write_all(b"/");
        }
        write_all(ANSI_RESET);
    } else if mode & 0o111 != 0 {
        write_all(ANSI_EXEC);
        write_bytes(name);
        write_all(ANSI_RESET);
    } else {
        write_bytes(name);
    }
}

fn parse_tree_dirents(buf: &[u8], parent_path: &[u8; PATH_MAX], parent_len: usize, depth: usize) {
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

fn write_tree_indent(depth: usize) {
    let mut i = 0usize;
    while i <= depth {
        write_all(b"  ");
        i += 1;
    }
}

fn append_path_component(
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

fn basename(path: &[u8]) -> &[u8] {
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

fn trim_end_slashes(path: &[u8]) -> &[u8] {
    let mut end = path.len();
    while end > 1 && path[end - 1] == b'/' {
        end -= 1;
    }
    &path[..end]
}

fn stat_path(path: &[u8; PATH_MAX]) -> Option<LinuxStat> {
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

fn is_dir_path(path: &[u8; PATH_MAX]) -> bool {
    stat_path(path)
        .map(|stat| is_dir_mode(stat.st_mode))
        .unwrap_or(false)
}

fn is_dir_mode(mode: u32) -> bool {
    mode & S_IFMT == S_IFDIR
}

fn is_regular_mode(mode: u32) -> bool {
    mode & S_IFMT == S_IFREG
}

fn parse_rm_options(mut rest: &[u8]) -> (RmOptions, &[u8]) {
    let mut opts = RmOptions::default();
    loop {
        let (arg, tail) = next_arg(rest);
        if arg.len() < 2 || arg[0] != b'-' {
            return (opts, rest);
        }
        let mut i = 1usize;
        while i < arg.len() {
            match arg[i] {
                b'f' => opts.force = true,
                b'r' | b'R' => opts.recursive = true,
                _ => {}
            }
            i += 1;
        }
        rest = tail;
    }
}

fn rm_one_arg(arg: &[u8], state: &ShellState, opts: RmOptions) {
    if is_star_pattern(arg) {
        rm_star(arg, state, opts);
        return;
    }

    let mut path = [0u8; PATH_MAX];
    let Some(path_len) = absolute_path(state.cwd(), arg, &mut path) else {
        write_all(b"rm: path too long\n");
        return;
    };
    rm_path(&path, path_len, opts, 0, b"rm");
}

fn is_star_pattern(arg: &[u8]) -> bool {
    basename(arg) == b"*"
}

fn rm_star(arg: &[u8], state: &ShellState, opts: RmOptions) {
    let mut dir_arg = [0u8; PATH_MAX];
    let dir_slice = star_parent_arg(arg, &mut dir_arg);
    let mut dir = [0u8; PATH_MAX];
    let Some(dir_len) = absolute_path(state.cwd(), dir_slice, &mut dir) else {
        write_all(b"rm: path too long\n");
        return;
    };
    rm_directory_children(&dir, dir_len, opts, 0, false, true);
}

fn star_parent_arg<'a>(arg: &'a [u8], out: &'a mut [u8; PATH_MAX]) -> &'a [u8] {
    let arg = trim_end_slashes(arg);
    let mut i = arg.len();
    while i > 0 {
        if arg[i - 1] == b'/' {
            if i == 1 {
                return b"/";
            }
            let len = i - 1;
            out[..len].copy_from_slice(&arg[..len]);
            return &out[..len];
        }
        i -= 1;
    }
    b"."
}

fn rm_path(path: &[u8; PATH_MAX], path_len: usize, opts: RmOptions, depth: usize, prefix: &[u8]) {
    let Some(stat) = stat_path(path) else {
        if !opts.force {
            print_errno(prefix, -2);
        }
        return;
    };

    if is_dir_mode(stat.st_mode) {
        if !opts.recursive {
            print_errno(prefix, -21);
            return;
        }
        if depth >= RM_MAX_DEPTH {
            write_all(b"rm: recursion limit\n");
            return;
        }
        rm_directory_children(path, path_len, opts, depth + 1, true, false);
        return;
    }

    let rc = unsafe { syscall::syscall1(syscall::SYS_UNLINK, path.as_ptr() as u64) };
    if rc < 0 && !opts.force {
        print_errno(prefix, rc);
    }
}

fn rm_directory_children(
    path: &[u8; PATH_MAX],
    path_len: usize,
    opts: RmOptions,
    depth: usize,
    remove_self: bool,
    skip_hidden: bool,
) {
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
        if !opts.force {
            print_errno(b"rm", fd);
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
            if !opts.force {
                print_errno(b"rm", n);
            }
            break;
        }
        rm_dirent_batch(&buf[..n as usize], path, path_len, opts, depth, skip_hidden);
    }
    let _ = close_fd(fd);

    if remove_self {
        let rc = unsafe { syscall::syscall1(syscall::SYS_RMDIR, path.as_ptr() as u64) };
        if rc < 0 && !opts.force {
            print_errno(b"rm", rc);
        }
    }
}

fn rm_dirent_batch(
    buf: &[u8],
    parent_path: &[u8; PATH_MAX],
    parent_len: usize,
    opts: RmOptions,
    depth: usize,
    skip_hidden: bool,
) {
    let mut off = 0usize;
    while off + DIRENT64_HEADER_SIZE <= buf.len() {
        let reclen = u16::from_le_bytes([buf[off + 16], buf[off + 17]]) as usize;
        if reclen == 0 || off + reclen > buf.len() {
            break;
        }
        let name_start = off + DIRENT64_HEADER_SIZE;
        let mut name_end = name_start;
        while name_end < off + reclen && buf[name_end] != 0 {
            name_end += 1;
        }
        let name = &buf[name_start..name_end];
        if !name.is_empty()
            && !bytes_eq(name, b".")
            && !bytes_eq(name, b"..")
            && (!skip_hidden || name[0] != b'.')
        {
            let mut child = [0u8; PATH_MAX];
            if let Some(child_len) =
                append_path_component(parent_path, parent_len, name, &mut child)
            {
                rm_path(&child, child_len, opts, depth, b"rm");
            }
        }
        off += reclen;
    }
}

fn absolute_path(cwd: &[u8], input: &[u8], out: &mut [u8; PATH_MAX]) -> Option<usize> {
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

fn pop_component(out: &mut [u8; PATH_MAX], len: &mut usize) {
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

fn first_token(line: &[u8]) -> (&[u8], &[u8]) {
    next_arg(line)
}

fn next_arg(input: &[u8]) -> (&[u8], &[u8]) {
    let input = trim_start(input);
    let mut end = 0usize;
    while end < input.len() && !is_space(input[end]) {
        end += 1;
    }
    (&input[..end], &input[end..])
}

fn trim(input: &[u8]) -> &[u8] {
    trim_end(trim_start(input))
}

fn trim_start(input: &[u8]) -> &[u8] {
    let mut start = 0usize;
    while start < input.len() && is_space(input[start]) {
        start += 1;
    }
    &input[start..]
}

fn trim_end(input: &[u8]) -> &[u8] {
    let mut end = input.len();
    while end > 0 && is_space(input[end - 1]) {
        end -= 1;
    }
    &input[..end]
}

fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r')
}

fn find_byte(input: &[u8], needle: u8) -> Option<usize> {
    let mut i = 0usize;
    while i < input.len() {
        if input[i] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn bytes_eq(a: &[u8], b: &[u8]) -> bool {
    a == b
}

fn strip_prefix<'a>(input: &'a [u8], prefix: &[u8]) -> Option<&'a [u8]> {
    if input.len() < prefix.len() {
        return None;
    }
    if &input[..prefix.len()] == prefix {
        Some(&input[prefix.len()..])
    } else {
        None
    }
}

fn parse_u32(input: &[u8]) -> Option<u32> {
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

fn parse_u64(input: &[u8]) -> Option<u64> {
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

fn parse_ipv4(input: &[u8]) -> Option<[u8; 4]> {
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

fn sockaddr_in(addr: [u8; 4], port: u16) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[0..2].copy_from_slice(&AF_INET.to_ne_bytes());
    out[2..4].copy_from_slice(&port.to_be_bytes());
    out[4..8].copy_from_slice(&addr);
    out
}

fn parse_size(input: &[u8]) -> Option<u64> {
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

fn is_dev_zero(path: &[u8]) -> bool {
    bytes_eq(path, b"/dev/zero")
}

fn is_dev_null(path: &[u8]) -> bool {
    bytes_eq(path, b"/dev/null")
}

fn is_dev_urandom(path: &[u8]) -> bool {
    bytes_eq(path, b"/dev/urandom") || bytes_eq(path, b"/dev/random")
}

fn service_pid(name: &[u8]) -> Option<u32> {
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

fn write_padded(bytes: &[u8], width: usize) {
    write_bytes(bytes);
    let mut n = bytes.len();
    while n < width {
        write_all(b" ");
        n += 1;
    }
}

fn write_u32(mut value: u32) {
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

fn write_ipv4(addr: [u8; 4]) {
    write_u32(addr[0] as u32);
    write_all(b".");
    write_u32(addr[1] as u32);
    write_all(b".");
    write_u32(addr[2] as u32);
    write_all(b".");
    write_u32(addr[3] as u32);
}

fn push_u32(out: &mut [u8], len: &mut usize, mut value: u32) {
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

fn push_bytes(out: &mut [u8], len: &mut usize, bytes: &[u8]) {
    let available = out.len().saturating_sub(*len);
    let n = bytes.len().min(available);
    if n == 0 {
        return;
    }
    out[*len..*len + n].copy_from_slice(&bytes[..n]);
    *len += n;
}

fn write_mode(mode: u32) {
    let kind = if is_dir_mode(mode) {
        b'd'
    } else if is_regular_mode(mode) {
        b'-'
    } else {
        b'?'
    };
    let mut out = [b'-'; 10];
    out[0] = kind;
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
    write_bytes(&out);
}

fn write_human_size(size: i64) {
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

fn print_errno(prefix: &[u8], rc: i64) {
    write_bytes(prefix);
    write_all(b": errno ");
    write_i64(rc);
    write_all(b"\n");
}

fn write_i64(value: i64) {
    if value < 0 {
        write_all(b"-");
        write_u64(value.unsigned_abs());
    } else {
        write_u64(value as u64);
    }
}

fn write_u64(mut value: u64) {
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

fn write_duration_ms(ns: u64) {
    write_u64(ns / 1_000_000);
    write_all(b"ms");
}

fn write_mib_per_sec(bytes: u64, ns: u64) {
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

fn write_fixed_2(centi_units: u64) {
    let whole = centi_units / 100;
    let frac = centi_units % 100;
    write_u64(whole);
    write_all(b".");
    write_byte(b'0' + (frac / 10) as u8);
    write_byte(b'0' + (frac % 10) as u8);
}

fn write_all(bytes: &[u8]) {
    write_bytes(bytes);
}

fn write_byte(byte: u8) {
    write_bytes(&[byte]);
}

fn write_bytes(bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    let _ = unsafe {
        syscall::syscall3(
            syscall::SYS_WRITE,
            STDOUT,
            bytes.as_ptr() as u64,
            bytes.len() as u64,
        )
    };
}

fn write_fd_all(fd: i64, bytes: &[u8]) -> i64 {
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

fn close_fd(fd: i64) -> i64 {
    unsafe { syscall::syscall1(syscall::SYS_CLOSE, fd as u64) }
}

fn ftruncate_fd(fd: i64, len: u64) -> i64 {
    unsafe { syscall::syscall2(syscall::SYS_FTRUNCATE, fd as u64, len) }
}

fn fdatasync_fd(fd: i64) -> i64 {
    unsafe { syscall::syscall1(syscall::SYS_FDATASYNC, fd as u64) }
}

fn sleep_ms(ms: u64) {
    let ts = Timespec {
        tv_sec: (ms / 1000) as i64,
        tv_nsec: ((ms % 1000) * 1_000_000) as i64,
    };
    let _ = unsafe { syscall::syscall2(syscall::SYS_NANOSLEEP, &ts as *const _ as u64, 0) };
}

fn monotonic_ns() -> Option<u64> {
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

fn cmd_sync() {
    let rc = unsafe { syscall::syscall0(syscall::SYS_SYNC) };
    if rc < 0 {
        print_errno(b"sync", rc);
    }
}

fn cmd_bench(rest: &[u8]) {
    let (sub, args) = next_arg(rest);
    if bytes_eq(sub, b"ipc") {
        bench_ipc(args);
    } else if bytes_eq(sub, b"sched") {
        bench_sched(args);
    } else if bytes_eq(sub, b"crypto") {
        bench_crypto(args);
    } else if bytes_eq(sub, b"fs") {
        bench_fs(args);
    } else {
        write_all(b"bench: subcommands: ipc sched crypto fs\n");
        write_all(b"  bench ipc    [n=10000]  -- IPC round-trip latency\n");
        write_all(b"  bench sched  [n=10000]  -- scheduler yield cost\n");
        write_all(b"  bench crypto [n=1000]   -- crypto_server random throughput\n");
        write_all(b"  bench fs     [n=128]    -- write+read N MiB non-zero\n");
    }
}

fn parse_bench_n(args: &[u8], default: u64) -> u64 {
    let (arg, _) = next_arg(args);
    if let Some(val) = strip_prefix(arg, b"n=") {
        if let Some(n) = parse_u64(val) {
            if n > 0 {
                return n;
            }
        }
    }
    default
}

fn bench_ipc(args: &[u8]) {
    let n = parse_bench_n(args, 10_000);
    let pid = unsafe { syscall::syscall0(syscall::SYS_GETPID) };
    if pid <= 0 {
        write_all(b"bench ipc: getpid failed\n");
        return;
    }

    let name = b"ipc_router";
    let endpoint = unsafe {
        syscall::syscall2(
            syscall::SYS_IPC_LOOKUP,
            name.as_ptr() as u64,
            name.len() as u64,
        )
    };
    if endpoint <= 0 {
        write_all(b"bench ipc: ipc_router not found\n");
        return;
    }

    let reply_endpoint = ((pid as u64) << 32) | 0x4950_4342;
    let reply_name = b"exosh_ipc_bench";
    let _ = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            reply_name.as_ptr() as u64,
            reply_name.len() as u64,
            reply_endpoint,
        )
    };

    let mut msg = syscall::IpcMessage::zeroed();
    msg.msg_type = 2; // ipc_router heartbeat
    msg.payload[..8].copy_from_slice(&reply_endpoint.to_le_bytes());
    write_all(b"bench ipc: ");
    write_u64(n);
    write_all(b" heartbeats... ");

    let Some(start) = monotonic_ns() else {
        write_all(b"clock unavailable\n");
        return;
    };

    let mut i = 0u64;
    while i < n {
        let send_rc = unsafe {
            syscall::syscall6(
                syscall::SYS_IPC_SEND,
                endpoint as u64,
                &msg as *const syscall::IpcMessage as u64,
                core::mem::size_of::<syscall::IpcMessage>() as u64,
                syscall::IPC_FLAG_INJECT_SRC_PID,
                0,
                0,
            )
        };
        if send_rc < 0 {
            print_errno(b"bench ipc send", send_rc);
            return;
        }

        let mut reply = [0u8; 4];
        let recv_rc = unsafe {
            syscall::syscall4(
                syscall::SYS_IPC_RECV,
                reply_endpoint,
                reply.as_mut_ptr() as u64,
                reply.len() as u64,
                syscall::IPC_FLAG_TIMEOUT | 100,
            )
        };
        if recv_rc < 0 {
            print_errno(b"bench ipc recv", recv_rc);
            return;
        }

        i = i.wrapping_add(1);
    }

    let Some(end) = monotonic_ns() else {
        write_all(b"clock unavailable\n");
        return;
    };

    let elapsed_ns = end.saturating_sub(start);
    let elapsed_ms = elapsed_ns / 1_000_000;
    let ns_per_op = if n > 0 { elapsed_ns / n } else { 0 };
    let kops_centi = if elapsed_ns > 0 {
        n.saturating_mul(100_000_000) / elapsed_ns
    } else {
        0
    };

    write_u64(elapsed_ms);
    write_all(b"ms -> ");
    write_u64(ns_per_op);
    write_all(b"ns/op (");
    write_fixed_2(kops_centi);
    write_all(b" Kops/s)\n");
}

fn bench_sched(args: &[u8]) {
    let n = parse_bench_n(args, 10_000);

    write_all(b"bench sched: ");
    write_u64(n);
    write_all(b" yields... ");

    let Some(start) = monotonic_ns() else {
        write_all(b"clock unavailable\n");
        return;
    };

    let mut i = 0u64;
    while i < n {
        unsafe {
            syscall::syscall0(syscall::SYS_SCHED_YIELD);
        }
        i = i.wrapping_add(1);
    }

    let Some(end) = monotonic_ns() else {
        write_all(b"clock unavailable\n");
        return;
    };

    let elapsed_ns = end.saturating_sub(start);
    let elapsed_ms = elapsed_ns / 1_000_000;
    let ns_per_yield = if n > 0 { elapsed_ns / n } else { 0 };
    let us_per_yield = ns_per_yield / 1_000;
    let ns_frac = ns_per_yield % 1_000;

    write_u64(elapsed_ms);
    write_all(b"ms -> ");
    write_u64(us_per_yield);
    write_all(b".");
    if ns_frac < 100 {
        write_all(b"0");
    }
    if ns_frac < 10 {
        write_all(b"0");
    }
    write_u64(ns_frac);
    write_all(b"us/yield\n");
}

const CRYPTO_SERVER_ENDPOINT: u64 = 4;
const CRYPTO_SERVER_PID: u32 = 5;
const CRYPTO_PROTOCOL_VERSION: u8 = 3;
const CRYPTO_REQUEST_PAYLOAD_SIZE: usize = 200;
const CRYPTO_REPLY_DATA_SIZE: usize = 224;
const CRYPTO_RANDOM: u32 = 1;
const CRYPTO_OK: u32 = 0;
const IPC_RECV_TIMEOUT_MS: u64 = 5_000;

#[repr(C)]
struct CryptoRequestBench {
    sender_pid: u32,
    msg_type: u32,
    reply_endpoint: u64,
    payload_len: u16,
    version: u8,
    flags: u8,
    cap_token: syscall::ExoCapTokenWire,
    payload: [u8; CRYPTO_REQUEST_PAYLOAD_SIZE],
}

const _: () =
    assert!(core::mem::size_of::<CryptoRequestBench>() <= syscall::IPC_KERNEL_MAX_MSG_SIZE);

#[repr(C)]
struct CryptoReplyBench {
    sender_pid: u32,
    status: u32,
    key_handle: u32,
    data_len: u16,
    version: u8,
    flags: u8,
    data: [u8; CRYPTO_REPLY_DATA_SIZE],
}

fn bench_crypto(args: &[u8]) {
    let n = parse_bench_n(args, 1_000);
    let pid = unsafe { syscall::syscall0(syscall::SYS_GETPID) };
    if pid <= 0 {
        write_all(b"bench crypto: getpid failed\n");
        return;
    }

    let reply_endpoint = ((pid as u64) << 32) | 0x4352_5950;
    let reply_name = b"exosh_crypto_bench";
    let _ = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            reply_name.as_ptr() as u64,
            reply_name.len() as u64,
            reply_endpoint,
        )
    };

    let mut token = syscall::ExoCapTokenWire::empty();
    let cap_rc = unsafe {
        syscall::exo_cap_create(
            syscall::EXO_CAP_TYPE_IPC_ENDPOINT,
            syscall::EXO_CAP_RIGHT_IPC_SEND,
            CRYPTO_SERVER_PID,
            &mut token,
        )
    };
    if cap_rc < 0 || token.is_empty() {
        write_all(b"bench crypto: crypto_server capability unavailable\n");
        return;
    }

    let mut req = CryptoRequestBench {
        sender_pid: 0,
        msg_type: CRYPTO_RANDOM,
        reply_endpoint,
        payload_len: 1,
        version: CRYPTO_PROTOCOL_VERSION,
        flags: 0,
        cap_token: token,
        payload: [0u8; CRYPTO_REQUEST_PAYLOAD_SIZE],
    };
    req.payload[0] = 32;

    write_all(b"bench crypto: ");
    write_u64(n);
    write_all(b" ops RANDOM... ");

    let Some(start) = monotonic_ns() else {
        write_all(b"clock unavailable\n");
        return;
    };

    let mut i = 0u64;
    while i < n {
        let send_rc = unsafe {
            syscall::syscall6(
                syscall::SYS_IPC_SEND,
                CRYPTO_SERVER_ENDPOINT,
                &req as *const CryptoRequestBench as u64,
                core::mem::size_of::<CryptoRequestBench>() as u64,
                syscall::IPC_FLAG_INJECT_SRC_PID,
                0,
                0,
            )
        };
        if send_rc < 0 {
            print_errno(b"bench crypto send", send_rc);
            return;
        }

        let mut reply = CryptoReplyBench {
            sender_pid: 0,
            status: 0,
            key_handle: 0,
            data_len: 0,
            version: 0,
            flags: 0,
            data: [0u8; CRYPTO_REPLY_DATA_SIZE],
        };
        let recv_rc = unsafe {
            syscall::syscall4(
                syscall::SYS_IPC_RECV,
                reply_endpoint,
                &mut reply as *mut CryptoReplyBench as u64,
                core::mem::size_of::<CryptoReplyBench>() as u64,
                syscall::IPC_FLAG_TIMEOUT | IPC_RECV_TIMEOUT_MS,
            )
        };
        if recv_rc < 0 {
            print_errno(b"bench crypto recv", recv_rc);
            return;
        }
        if reply.status != CRYPTO_OK {
            write_all(b"bench crypto: crypto_server status ");
            write_u64(reply.status as u64);
            write_all(b"\n");
            return;
        }

        i = i.wrapping_add(1);
    }

    let Some(end) = monotonic_ns() else {
        write_all(b"clock unavailable\n");
        return;
    };

    let elapsed_ns = end.saturating_sub(start);
    let elapsed_ms = elapsed_ns / 1_000_000;
    let us_per_op = if n > 0 { elapsed_ns / n / 1_000 } else { 0 };
    let total_bytes = n.saturating_mul(32);
    let mb_per_sec_centi = if elapsed_ns > 0 {
        (total_bytes as u128).saturating_mul(100_000) / (elapsed_ns as u128)
    } else {
        0
    };

    write_u64(elapsed_ms);
    write_all(b"ms -> ");
    write_u64(us_per_op);
    write_all(b"us/op (");
    write_fixed_2(mb_per_sec_centi.min(u64::MAX as u128) as u64);
    write_all(b" MB/s CSPRNG)\n");
}

fn bench_fs(args: &[u8]) {
    let n = parse_bench_n(args, 128);
    let target_bytes = n.saturating_mul(1024 * 1024);
    let path = b"/tmp/bench_fs_data";
    let mut out_path = [0u8; PATH_MAX];
    out_path[..path.len()].copy_from_slice(path);

    write_all(b"bench fs write: ");
    write_u64(n);
    write_all(b" MiB in 64 KiB chunks + one fdatasync... ");

    let buf = unsafe { &mut *DD_IO_BUFFER.0.get() };
    let bench_chunk = FS_BENCH_CHUNK.min(buf.len());
    let getrandom_rc = unsafe {
        syscall::syscall3(
            syscall::SYS_GETRANDOM,
            buf.as_mut_ptr() as u64,
            bench_chunk as u64,
            0,
        )
    };
    if getrandom_rc <= 0 {
        let mut k = 0usize;
        while k < bench_chunk {
            buf[k] = (k.wrapping_mul(0x6B).wrapping_add(0xA5) & 0xFF) as u8;
            k = k.wrapping_add(1);
        }
    }

    let Some(write_start) = monotonic_ns() else {
        write_all(b"clock unavailable\n");
        return;
    };

    let out_fd = unsafe {
        syscall::syscall4(
            syscall::SYS_OPENAT,
            AT_FDCWD,
            out_path.as_ptr() as u64,
            syscall::O_CREAT | syscall::O_WRONLY | syscall::O_TRUNC,
            0o644,
        )
    };
    if out_fd < 0 {
        print_errno(b"bench fs", out_fd);
        return;
    }

    let mut written_bytes = 0u64;
    while written_bytes < target_bytes {
        let this_write = (target_bytes - written_bytes).min(bench_chunk as u64) as usize;
        let rc = write_fd_all(out_fd, &buf[..this_write]);
        if rc < 0 {
            print_errno(b"bench fs write", rc);
            let _ = close_fd(out_fd);
            return;
        }
        written_bytes = written_bytes.saturating_add(this_write as u64);
    }
    let rc = fdatasync_fd(out_fd);
    if rc < 0 {
        print_errno(b"bench fs fdatasync", rc);
        let _ = close_fd(out_fd);
        return;
    }
    let _ = close_fd(out_fd);

    let Some(write_end) = monotonic_ns() else {
        write_all(b"clock unavailable\n");
        return;
    };

    print_dd_summary(target_bytes, write_end.saturating_sub(write_start));

    write_all(b"bench fs read:  ");
    write_u64(n);
    write_all(b" MiB in 64 KiB chunks... ");

    let Some(read_start) = monotonic_ns() else {
        write_all(b"clock unavailable\n");
        return;
    };

    let mut total_read = 0u64;
    while total_read < target_bytes {
        let in_fd = unsafe {
            syscall::syscall4(
                syscall::SYS_OPENAT,
                AT_FDCWD,
                out_path.as_ptr() as u64,
                syscall::O_RDONLY,
                0,
            )
        };
        if in_fd < 0 {
            print_errno(b"bench fs read", in_fd);
            return;
        }

        let mut read_this_open = 0u64;
        while total_read < target_bytes {
            let request = (target_bytes - total_read).min(bench_chunk as u64) as usize;
            let n_read = unsafe {
                syscall::syscall3(
                    syscall::SYS_READ,
                    in_fd as u64,
                    buf.as_mut_ptr() as u64,
                    request as u64,
                )
            };
            if n_read <= 0 {
                break;
            }
            read_this_open = read_this_open.saturating_add(n_read as u64);
            total_read = total_read.saturating_add(n_read as u64);
        }
        let _ = close_fd(in_fd);
        if read_this_open == 0 {
            break;
        }
    }

    let Some(read_end) = monotonic_ns() else {
        write_all(b"clock unavailable\n");
        return;
    };

    print_dd_summary(total_read, read_end.saturating_sub(read_start));
    let _ = unsafe { syscall::syscall1(syscall::SYS_UNLINK, out_path.as_ptr() as u64) };
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nostack, nomem));
        }
    }
}
