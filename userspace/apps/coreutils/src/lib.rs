#![cfg_attr(target_os = "none", no_std)]

#[cfg(not(target_os = "none"))]
pub mod host {
    use std::fs::{self, File, OpenOptions};
    use std::io::{self, Read, Write};
    use std::path::Path;

    pub fn ls(path: &Path, out: &mut dyn Write) -> io::Result<()> {
        let mut entries = fs::read_dir(path)?.collect::<io::Result<Vec<_>>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            writeln!(out, "{}", entry.file_name().to_string_lossy())?;
        }
        Ok(())
    }

    pub fn mkdir(path: &Path) -> io::Result<()> {
        fs::create_dir(path)
    }

    pub fn rm(path: &Path) -> io::Result<()> {
        fs::remove_file(path)
    }

    pub fn rmdir(path: &Path) -> io::Result<()> {
        fs::remove_dir(path)
    }

    pub fn touch(path: &Path) -> io::Result<()> {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map(|_| ())
    }

    pub fn cat(path: &Path, out: &mut dyn Write) -> io::Result<()> {
        let mut file = File::open(path)?;
        let mut buf = [0u8; 4096];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                return Ok(());
            }
            out.write_all(&buf[..n])?;
        }
    }

    pub fn echo(args: &[String], out: &mut dyn Write) -> io::Result<()> {
        for (idx, arg) in args.iter().enumerate() {
            if idx != 0 {
                write!(out, " ")?;
            }
            write!(out, "{arg}")?;
        }
        writeln!(out)
    }

    pub fn host_main(command: &str) -> i32 {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let result = match command {
            "cat" => args.first().map(|p| cat(Path::new(p), &mut std::io::stdout())).unwrap_or_else(|| {
                eprintln!("cat: missing operand");
                Err(io::Error::from(io::ErrorKind::InvalidInput))
            }),
            "echo" => echo(&args, &mut std::io::stdout()),
            "ls" => ls(Path::new(args.first().map(String::as_str).unwrap_or(".")), &mut std::io::stdout()),
            "mkdir" => args.first().map(|p| mkdir(Path::new(p))).unwrap_or_else(|| {
                eprintln!("mkdir: missing operand");
                Err(io::Error::from(io::ErrorKind::InvalidInput))
            }),
            "rm" => args.first().map(|p| rm(Path::new(p))).unwrap_or_else(|| {
                eprintln!("rm: missing operand");
                Err(io::Error::from(io::ErrorKind::InvalidInput))
            }),
            "rmdir" => args.first().map(|p| rmdir(Path::new(p))).unwrap_or_else(|| {
                eprintln!("rmdir: missing operand");
                Err(io::Error::from(io::ErrorKind::InvalidInput))
            }),
            "touch" => args.first().map(|p| touch(Path::new(p))).unwrap_or_else(|| {
                eprintln!("touch: missing operand");
                Err(io::Error::from(io::ErrorKind::InvalidInput))
            }),
            _ => Ok(()),
        };
        if let Err(err) = result {
            eprintln!("{command}: {err}");
            1
        } else {
            0
        }
    }
}

#[cfg(not(target_os = "none"))]
pub use host::{cat, echo, ls, mkdir, rm, rmdir, touch};

#[cfg(target_os = "none")]
pub mod bare {
    use core::panic::PanicInfo;
    use exo_syscall_abi as syscall;

    const STDOUT: u64 = 1;
    const STDERR: u64 = 2;
    const AT_FDCWD: u64 = (-100i64) as u64;
    const PATH_MAX: usize = 256;
    const ARG_MAX: usize = 32;
    const ENV_MAX: usize = 16;
    const IO_BUF: usize = 4096;
    const DIRENT64_HEADER_SIZE: usize = 24;
    const DT_DIR: u8 = 4;
    const S_IFMT: u32 = 0o170000;
    const S_IFDIR: u32 = 0o040000;
    const SIGTERM: u64 = 15;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct LinuxTimespec {
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
        __pad0: i32,
        st_rdev: u64,
        st_size: i64,
        st_blksize: i64,
        st_blocks: i64,
        st_atim: LinuxTimespec,
        st_mtim: LinuxTimespec,
        st_ctim: LinuxTimespec,
        __unused: [i64; 3],
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct LinuxSysInfo {
        uptime: i64,
        loads: [u64; 3],
        totalram: u64,
        freeram: u64,
        sharedram: u64,
        bufferram: u64,
        totalswap: u64,
        freeswap: u64,
        procs: u16,
        pad: u16,
        _pad2: u32,
        totalhigh: u64,
        freehigh: u64,
        mem_unit: u32,
        _pad3: [u8; 8],
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct LinuxUtsname {
        sysname: [u8; 65],
        nodename: [u8; 65],
        release: [u8; 65],
        version: [u8; 65],
        machine: [u8; 65],
        domainname: [u8; 65],
    }

    impl Default for LinuxUtsname {
        fn default() -> Self {
            Self {
                sysname: [0; 65],
                nodename: [0; 65],
                release: [0; 65],
                version: [0; 65],
                machine: [0; 65],
                domainname: [0; 65],
            }
        }
    }

    pub struct Args<'a> {
        argv: [&'a [u8]; ARG_MAX],
        argc: usize,
        envp: [&'a [u8]; ENV_MAX],
        envc: usize,
    }

    impl<'a> Args<'a> {
        pub unsafe fn from_stack(stack: usize) -> Self {
            let mut argv = [&[][..]; ARG_MAX];
            let mut envp = [&[][..]; ENV_MAX];
            let mut argc = *(stack as *const u64) as usize;
            if argc > ARG_MAX {
                argc = ARG_MAX;
            }
            let mut ptr = (stack as *const u64).add(1);
            let mut i = 0usize;
            while i < argc {
                let raw = *ptr;
                argv[i] = cstr_slice(raw as *const u8, 4096);
                ptr = ptr.add(1);
                i += 1;
            }
            if *ptr == 0 {
                ptr = ptr.add(1);
            }
            let mut envc = 0usize;
            while envc < ENV_MAX {
                let raw = *ptr;
                if raw == 0 {
                    break;
                }
                envp[envc] = cstr_slice(raw as *const u8, 4096);
                ptr = ptr.add(1);
                envc += 1;
            }
            Self {
                argv,
                argc,
                envp,
                envc,
            }
        }

        pub fn len(&self) -> usize {
            self.argc
        }

        pub fn get(&self, idx: usize) -> &'a [u8] {
            if idx < self.argc {
                self.argv[idx]
            } else {
                &[]
            }
        }

        fn pwd(&self) -> &'a [u8] {
            let mut i = 0usize;
            while i < self.envc {
                if self.envp[i].starts_with(b"PWD=") && self.envp[i].len() > 4 {
                    return &self.envp[i][4..];
                }
                i += 1;
            }
            b"/"
        }
    }

    unsafe fn cstr_slice<'a>(ptr: *const u8, max: usize) -> &'a [u8] {
        if ptr.is_null() {
            return &[];
        }
        let mut len = 0usize;
        while len < max && *ptr.add(len) != 0 {
            len += 1;
        }
        core::slice::from_raw_parts(ptr, len)
    }

    pub fn exit(code: i32) -> ! {
        unsafe {
            let _ = syscall::syscall1(syscall::SYS_EXIT, (code as u64) & 0xff);
            let _ = syscall::syscall1(syscall::SYS_EXIT_GROUP, (code as u64) & 0xff);
        }
        loop {
            core::hint::spin_loop();
        }
    }

    #[panic_handler]
    fn panic(_info: &PanicInfo) -> ! {
        write_all(STDERR, b"coreutils: panic\n");
        exit(125);
    }

    fn write_all(fd: u64, bytes: &[u8]) {
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

    fn write_byte(fd: u64, byte: u8) {
        write_all(fd, &[byte]);
    }

    fn write_u64(fd: u64, mut value: u64) {
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

    fn write_i64(fd: u64, value: i64) {
        if value < 0 {
            write_byte(fd, b'-');
            write_u64(fd, value.unsigned_abs());
        } else {
            write_u64(fd, value as u64);
        }
    }

    fn print_errno(cmd: &[u8], rc: i64) -> i32 {
        write_all(STDERR, cmd);
        write_all(STDERR, b": errno ");
        write_i64(STDERR, rc);
        write_byte(STDERR, b'\n');
        1
    }

    fn eq(a: &[u8], b: &[u8]) -> bool {
        a == b
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

    fn parse_i32(input: &[u8]) -> Option<i32> {
        parse_u64(input).and_then(|v| i32::try_from(v).ok())
    }

    fn copy_component(out: &mut [u8; PATH_MAX], len: &mut usize, comp: &[u8]) -> bool {
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

    fn pop_component(out: &mut [u8; PATH_MAX], len: &mut usize) {
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

    fn normalize_into(src: &[u8], out: &mut [u8; PATH_MAX]) -> Option<usize> {
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

    fn path_arg(args: &Args, input: &[u8], out: &mut [u8; PATH_MAX]) -> Option<usize> {
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

    fn basename(path: &[u8]) -> &[u8] {
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

    fn dirname(path: &[u8], out: &mut [u8; PATH_MAX]) -> usize {
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

    fn open_path(path: &[u8; PATH_MAX], flags: u64, mode: u64) -> i64 {
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

    fn close(fd: i64) {
        if fd >= 0 {
            let _ = unsafe { syscall::syscall1(syscall::SYS_CLOSE, fd as u64) };
        }
    }

    fn stat_path(path: &[u8; PATH_MAX]) -> Option<LinuxStat> {
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

    fn is_dir_mode(mode: u32) -> bool {
        mode & S_IFMT == S_IFDIR
    }

    fn write_mode(mode: u32) {
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

    fn read_file_to_fd(path: &[u8; PATH_MAX], out_fd: u64) -> i32 {
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

    pub fn cmd_ls(args: &Args) -> i32 {
        let mut all = false;
        let mut long = false;
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
                    write_mode(st.st_mode);
                    write_all(STDOUT, b" ");
                    write_i64(STDOUT, st.st_size);
                    write_all(STDOUT, b" ");
                }
                write_all(STDOUT, basename(target));
                write_byte(STDOUT, b'\n');
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
                        if dtype == DT_DIR {
                            write_all(STDOUT, b"drwxr-xr-x ");
                        } else {
                            write_all(STDOUT, b"-rw-r--r-- ");
                        }
                    } else if printed {
                        write_all(STDOUT, b"  ");
                    }
                    write_all(STDOUT, name);
                    if long {
                        write_byte(STDOUT, b'\n');
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
            return print_errno(b"cat", -22);
        }
        let mut rc = 0;
        let mut i = 1usize;
        while i < args.len() {
            let mut path = [0u8; PATH_MAX];
            if path_arg(args, args.get(i), &mut path).is_none() {
                rc = print_errno(b"cat", -36);
            } else {
                let r = read_file_to_fd(&path, STDOUT);
                if r != 0 {
                    rc = r;
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
        i = 1;
        while i < stop {
            if i > 1 {
                write_byte(fd, b' ');
            }
            write_all(fd, args.get(i));
            i += 1;
        }
        write_byte(fd, b'\n');
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
        let mut rc = 0;
        let mut i = 1usize;
        while i < args.len() {
            let arg = args.get(i);
            if arg.starts_with(b"-") {
                i += 1;
                continue;
            }
            let mut path = [0u8; PATH_MAX];
            if path_arg(args, arg, &mut path).is_none() {
                rc = print_errno(b"rm", -36);
            } else {
                let r = unsafe { syscall::syscall1(syscall::SYS_UNLINK, path.as_ptr() as u64) };
                if r < 0 {
                    rc = print_errno(b"rm", r);
                }
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
        let Some(st) = stat_path(&path) else {
            return print_errno(b"stat", -2);
        };
        write_all(STDOUT, b"File: ");
        write_all(STDOUT, args.get(1));
        write_all(STDOUT, b"\nSize: ");
        write_i64(STDOUT, st.st_size);
        write_all(STDOUT, b"\nMode: ");
        write_mode(st.st_mode);
        write_all(STDOUT, b"\nInode: ");
        write_u64(STDOUT, st.st_ino);
        write_byte(STDOUT, b'\n');
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
        let rc = unsafe {
            syscall::syscall2(
                syscall::SYS_RENAME,
                src.as_ptr() as u64,
                dst.as_ptr() as u64,
            )
        };
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

    pub fn cmd_sleep(args: &Args) -> i32 {
        if args.len() < 2 {
            return print_errno(b"sleep", -22);
        }
        let Some(ms) = parse_u64(args.get(1)) else {
            return print_errno(b"sleep", -22);
        };
        let ts = LinuxTimespec {
            tv_sec: (ms / 1000) as i64,
            tv_nsec: ((ms % 1000) * 1_000_000) as i64,
        };
        let _ = unsafe { syscall::syscall2(syscall::SYS_NANOSLEEP, &ts as *const _ as u64, 0) };
        0
    }

    fn sysinfo() -> Option<LinuxSysInfo> {
        let mut info = LinuxSysInfo::default();
        let rc =
            unsafe { syscall::syscall1(syscall::SYS_SYSINFO, &mut info as *mut _ as u64) };
        if rc < 0 {
            None
        } else {
            Some(info)
        }
    }

    fn write_human(value: u64) {
        let mut v = value;
        let units = [b'B', b'K', b'M', b'G', b'T'];
        let mut unit = 0usize;
        while v >= 1024 && unit + 1 < units.len() {
            v = v.saturating_add(512) / 1024;
            unit += 1;
        }
        write_u64(STDOUT, v);
        write_byte(STDOUT, units[unit]);
    }

    pub fn cmd_meminfo(_args: &Args) -> i32 {
        let Some(info) = sysinfo() else {
            return print_errno(b"meminfo", -38);
        };
        let unit = if info.mem_unit == 0 { 1 } else { info.mem_unit as u64 };
        write_all(STDOUT, b"MemTotal: ");
        write_human(info.totalram.saturating_mul(unit));
        write_all(STDOUT, b"\nMemFree:  ");
        write_human(info.freeram.saturating_mul(unit));
        write_all(STDOUT, b"\nProcs:    ");
        write_u64(STDOUT, info.procs as u64);
        write_byte(STDOUT, b'\n');
        0
    }

    fn perf(metric: u64, index: u64) -> Option<u64> {
        let rc = unsafe { syscall::syscall2(syscall::SYS_EXO_PERF_READ, metric, index) };
        if rc < 0 {
            None
        } else {
            Some(rc as u64)
        }
    }

    pub fn cmd_syscall_stat(_args: &Args) -> i32 {
        let rows = [
            (b"read".as_slice(), syscall::SYS_READ),
            (b"write".as_slice(), syscall::SYS_WRITE),
            (b"open".as_slice(), syscall::SYS_OPEN),
            (b"execve".as_slice(), syscall::SYS_EXECVE),
            (b"wait4".as_slice(), syscall::SYS_WAIT4),
            (b"sync".as_slice(), syscall::SYS_SYNC),
        ];
        let mut i = 0usize;
        while i < rows.len() {
            write_all(STDOUT, rows[i].0);
            write_all(STDOUT, b" ");
            match perf(syscall::EXO_PERF_SYSCALL_COUNT, rows[i].1) {
                Some(v) => write_u64(STDOUT, v),
                None => write_all(STDOUT, b"unavailable"),
            }
            write_byte(STDOUT, b'\n');
            i += 1;
        }
        0
    }

    pub fn cmd_ipc_stat(_args: &Args) -> i32 {
        let rows = [
            (b"messages_sent".as_slice(), syscall::EXO_PERF_IPC_MESSAGES_SENT),
            (b"messages_recv".as_slice(), syscall::EXO_PERF_IPC_MESSAGES_RECEIVED),
            (b"messages_drop".as_slice(), syscall::EXO_PERF_IPC_MESSAGES_DROPPED),
            (b"rpc_calls".as_slice(), syscall::EXO_PERF_IPC_RPC_CALLS),
        ];
        let mut i = 0usize;
        while i < rows.len() {
            write_all(STDOUT, rows[i].0);
            write_all(STDOUT, b" ");
            match perf(rows[i].1, 0) {
                Some(v) => write_u64(STDOUT, v),
                None => write_all(STDOUT, b"unavailable"),
            }
            write_byte(STDOUT, b'\n');
            i += 1;
        }
        0
    }

    pub fn cmd_ps(_args: &Args) -> i32 {
        let mut entries = [syscall::ExoProcessInfo::zeroed(); 64];
        let rc = unsafe {
            syscall::syscall2(
                syscall::SYS_EXO_PROCESS_LIST,
                entries.as_mut_ptr() as u64,
                entries.len() as u64,
            )
        };
        if rc < 0 {
            return print_errno(b"ps", rc);
        }
        write_all(STDOUT, b"PID PPID NAME\n");
        let mut i = 0usize;
        while i < rc as usize && i < entries.len() {
            write_u64(STDOUT, entries[i].pid as u64);
            write_byte(STDOUT, b' ');
            write_u64(STDOUT, entries[i].ppid as u64);
            write_byte(STDOUT, b' ');
            let mut end = 0usize;
            while end < entries[i].name.len() && entries[i].name[end] != 0 {
                end += 1;
            }
            write_all(STDOUT, &entries[i].name[..end]);
            write_byte(STDOUT, b'\n');
            i += 1;
        }
        0
    }

    pub fn cmd_top(args: &Args) -> i32 {
        cmd_ps(args)
    }

    pub fn cmd_kill(args: &Args) -> i32 {
        if args.len() < 2 {
            return print_errno(b"kill", -22);
        }
        let mut sig = SIGTERM;
        let mut target = args.get(1);
        if target.starts_with(b"-") && args.len() > 2 {
            sig = parse_u64(&target[1..]).unwrap_or(SIGTERM);
            target = args.get(2);
        }
        let Some(pid) = parse_i32(target) else {
            return print_errno(b"kill", -22);
        };
        let rc = unsafe { syscall::syscall2(syscall::SYS_KILL, pid as u64, sig) };
        if rc < 0 {
            print_errno(b"kill", rc)
        } else {
            0
        }
    }

    pub fn cmd_dd(args: &Args) -> i32 {
        let mut input = b"/dev/zero" as &[u8];
        let mut output = b"/dev/null" as &[u8];
        let mut bs = 512usize;
        let mut count = 1usize;
        let mut i = 1usize;
        while i < args.len() {
            let arg = args.get(i);
            if arg.starts_with(b"if=") {
                input = &arg[3..];
            } else if arg.starts_with(b"of=") {
                output = &arg[3..];
            } else if arg.starts_with(b"bs=") {
                bs = parse_u64(&arg[3..]).unwrap_or(512) as usize;
            } else if arg.starts_with(b"count=") {
                count = parse_u64(&arg[6..]).unwrap_or(1) as usize;
            }
            i += 1;
        }
        let mut in_path = [0u8; PATH_MAX];
        let mut out_path = [0u8; PATH_MAX];
        let inf = if eq(input, b"/dev/zero") {
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
        let outf = if eq(output, b"/dev/null") {
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
        let mut buf = [0u8; IO_BUF];
        let chunk = bs.min(buf.len());
        let mut total = 0u64;
        let mut nblocks = 0usize;
        while nblocks < count {
            let n = if inf < 0 {
                chunk as i64
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
            if n <= 0 {
                break;
            }
            if outf >= 0 {
                write_all(outf as u64, &buf[..n as usize]);
            }
            total = total.saturating_add(n as u64);
            nblocks += 1;
        }
        close(inf);
        close(outf);
        write_u64(STDOUT, total);
        write_all(STDOUT, b" bytes copied\n");
        0
    }

    pub fn cmd_uptime(_args: &Args) -> i32 {
        let Some(info) = sysinfo() else {
            return print_errno(b"uptime", -38);
        };
        write_all(STDOUT, b"up ");
        write_i64(STDOUT, info.uptime);
        write_all(STDOUT, b" seconds\n");
        0
    }

    pub fn cmd_whoami(_args: &Args) -> i32 {
        let uid = unsafe { syscall::syscall0(syscall::SYS_GETUID) };
        if uid == 0 {
            write_all(STDOUT, b"root\n");
        } else {
            write_all(STDOUT, b"uid");
            write_i64(STDOUT, uid);
            write_byte(STDOUT, b'\n');
        }
        0
    }

    pub fn cmd_uname(_args: &Args) -> i32 {
        let mut uts = LinuxUtsname::default();
        let rc = unsafe { syscall::syscall1(syscall::SYS_UNAME, &mut uts as *mut _ as u64) };
        if rc < 0 {
            return print_errno(b"uname", rc);
        }
        let mut end = 0usize;
        while end < uts.sysname.len() && uts.sysname[end] != 0 {
            end += 1;
        }
        write_all(STDOUT, &uts.sysname[..end]);
        write_all(STDOUT, b" ");
        end = 0;
        while end < uts.machine.len() && uts.machine[end] != 0 {
            end += 1;
        }
        write_all(STDOUT, &uts.machine[..end]);
        write_byte(STDOUT, b'\n');
        0
    }

    pub fn cmd_clear(_args: &Args) -> i32 {
        write_byte(STDOUT, 0x0c);
        0
    }

    pub fn cmd_true(_args: &Args) -> i32 {
        0
    }

    pub fn cmd_false(_args: &Args) -> i32 {
        1
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

    pub fn cmd_wc(args: &Args) -> i32 {
        if args.len() < 2 {
            return print_errno(b"wc", -22);
        }
        let mut path = [0u8; PATH_MAX];
        if path_arg(args, args.get(1), &mut path).is_none() {
            return print_errno(b"wc", -36);
        }
        let fd = open_path(&path, syscall::O_RDONLY, 0);
        if fd < 0 {
            return print_errno(b"wc", fd);
        }
        let mut buf = [0u8; IO_BUF];
        let mut bytes = 0u64;
        let mut lines = 0u64;
        let mut words = 0u64;
        let mut in_word = false;
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
                return print_errno(b"wc", n);
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
        close(fd);
        write_u64(STDOUT, lines);
        write_byte(STDOUT, b' ');
        write_u64(STDOUT, words);
        write_byte(STDOUT, b' ');
        write_u64(STDOUT, bytes);
        write_byte(STDOUT, b'\n');
        0
    }
}

#[cfg(target_os = "none")]
#[macro_export]
macro_rules! exo_command {
    ($run:path) => {
        core::arch::global_asm!(
            ".global _start",
            "_start:",
            "mov rdi, rsp",
            "and rsp, -16",
            "call {entry}",
            entry = sym __exo_coreutils_entry,
        );

        #[no_mangle]
        extern "C" fn __exo_coreutils_entry(stack: usize) -> ! {
            let args = unsafe { $crate::bare::Args::from_stack(stack) };
            let code = $run(&args);
            $crate::bare::exit(code);
        }
    };
}

#[cfg(test)]
mod tests {
    use super::host::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmpdir() -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        path.push(format!("exo-coreutils-{nonce}"));
        fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn touch_cat_ls_rm_roundtrip() {
        let dir = tmpdir();
        let file = dir.join("a");
        touch(&file).unwrap();
        fs::write(&file, b"hi").unwrap();
        let mut cat_out = Vec::new();
        cat(&file, &mut cat_out).unwrap();
        assert_eq!(cat_out, b"hi");
        let mut ls_out = Vec::new();
        ls(&dir, &mut ls_out).unwrap();
        assert_eq!(String::from_utf8(ls_out).unwrap(), "a\n");
        rm(&file).unwrap();
        fs::remove_dir(&dir).unwrap();
    }
}
