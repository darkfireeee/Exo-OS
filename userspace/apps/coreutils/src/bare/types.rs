#![allow(unused_imports, dead_code)]
use core::panic::PanicInfo;
use exo_syscall_abi as syscall;


    pub const STDOUT: u64 = 1;
    pub const STDERR: u64 = 2;
    pub const AT_FDCWD: u64 = (-100i64) as u64;
    pub const PATH_MAX: usize = 256;
    pub const ARG_MAX: usize = 32;
    pub const ENV_MAX: usize = 16;
    pub const IO_BUF: usize = 4096;
    pub const DIRENT64_HEADER_SIZE: usize = 24;
    pub const DT_DIR: u8 = 4;
    pub const S_IFMT: u32 = 0o170000;
    pub const S_IFDIR: u32 = 0o040000;
    pub const S_IFREG: u32 = 0o100000;
    pub const SIGTERM: u64 = 15;
    pub const CLOCK_MONOTONIC: u64 = 1;
    pub const RM_MAX_DEPTH: usize = 8;
    pub const EXDEV: i64 = -18;
    pub const EISDIR: i64 = -21;
    pub const ANSI_RESET: &[u8] = b"\x1b[0m";
    pub const ANSI_DIR: &[u8] = b"\x1b[1;34m";
    pub const ANSI_EXEC: &[u8] = b"\x1b[1;32m";

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct LinuxTimespec {
        pub tv_sec: i64,
        pub tv_nsec: i64,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct LinuxStat {
        pub st_dev: u64,
        pub st_ino: u64,
        pub st_nlink: u64,
        pub st_mode: u32,
        pub st_uid: u32,
        pub st_gid: u32,
        pub __pad0: i32,
        pub st_rdev: u64,
        pub st_size: i64,
        pub st_blksize: i64,
        pub st_blocks: i64,
        pub st_atim: LinuxTimespec,
        pub st_mtim: LinuxTimespec,
        pub st_ctim: LinuxTimespec,
        pub __unused: [i64; 3],
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct LinuxSysInfo {
        pub uptime: i64,
        pub loads: [u64; 3],
        pub totalram: u64,
        pub freeram: u64,
        pub sharedram: u64,
        pub bufferram: u64,
        pub totalswap: u64,
        pub freeswap: u64,
        pub procs: u16,
        pub pad: u16,
        pub _pad2: u32,
        pub totalhigh: u64,
        pub freehigh: u64,
        pub mem_unit: u32,
        pub _pad3: [u8; 8],
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct LinuxUtsname {
        pub sysname: [u8; 65],
        pub nodename: [u8; 65],
        pub release: [u8; 65],
        pub version: [u8; 65],
        pub machine: [u8; 65],
        pub domainname: [u8; 65],
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
        pub argv: [&'a [u8]; ARG_MAX],
        pub argc: usize,
        pub envp: [&'a [u8]; ENV_MAX],
        pub envc: usize,
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

        pub fn pwd(&self) -> &'a [u8] {
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

    pub unsafe fn cstr_slice<'a>(ptr: *const u8, max: usize) -> &'a [u8] {
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
        unsafe {
            let msg = b"coreutils: panic\n";
            let _ = syscall::syscall3(syscall::SYS_WRITE, STDERR, msg.as_ptr() as u64, msg.len() as u64);
        }
        exit(125);
    }

