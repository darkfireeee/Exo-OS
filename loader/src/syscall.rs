//! Direct `#![no_std]` Linux/Exo-OS syscall primitives for `/lib/ld-exo.so`.

pub const SYS_READ: u64 = 0;
pub const SYS_WRITE: u64 = 1;
pub const SYS_OPEN: u64 = 2;
pub const SYS_CLOSE: u64 = 3;
pub const SYS_LSEEK: u64 = 8;
pub const SYS_MMAP: u64 = 9;
pub const SYS_MPROTECT: u64 = 10;
pub const SYS_MUNMAP: u64 = 11;
pub const SYS_PREAD64: u64 = 17;
pub const SYS_EXIT: u64 = 60;
pub const SYS_ARCH_PRCTL: u64 = 158;
pub const SYS_GETTID: u64 = 186;
pub const SYS_EXIT_GROUP: u64 = 231;
pub const SYS_EXO_LOG: u64 = 350;

pub const PROT_NONE: u32 = 0x0;
pub const PROT_READ: u32 = 0x1;
pub const PROT_WRITE: u32 = 0x2;
pub const PROT_EXEC: u32 = 0x4;

pub const MAP_SHARED: u32 = 0x01;
pub const MAP_PRIVATE: u32 = 0x02;
pub const MAP_FIXED: u32 = 0x10;
pub const MAP_ANONYMOUS: u32 = 0x20;

pub const O_RDONLY: u32 = 0;
pub const SEEK_SET: i32 = 0;
pub const SEEK_END: i32 = 2;

pub const ARCH_SET_FS: u64 = 0x1002;
pub const ARCH_GET_FS: u64 = 0x1003;

pub const EINTR: i64 = 4;

#[cfg(all(target_arch = "x86_64", not(test)))]
#[inline(always)]
pub unsafe fn syscall6(
    nr: u64,
    a1: u64,
    a2: u64,
    a3: u64,
    a4: u64,
    a5: u64,
    a6: u64,
) -> i64 {
    let ret: i64;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") nr,
            in("rdi") a1,
            in("rsi") a2,
            in("rdx") a3,
            in("r10") a4,
            in("r8") a5,
            in("r9") a6,
            lateout("rax") ret,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    ret
}

#[cfg(not(all(target_arch = "x86_64", not(test))))]
#[inline(always)]
pub unsafe fn syscall6(
    _nr: u64,
    _a1: u64,
    _a2: u64,
    _a3: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    -38 // -ENOSYS on host test builds
}

#[inline(always)]
unsafe fn decode(ret: i64) -> Result<usize, i32> {
    if ret < 0 && ret >= -4095 {
        Err((-ret) as i32)
    } else {
        Ok(ret as usize)
    }
}

pub unsafe fn sys_mmap(
    addr: usize,
    len: usize,
    prot: u32,
    flags: u32,
    fd: i32,
    offset: u64,
) -> Result<usize, i32> {
    let ret = unsafe {
        syscall6(
            SYS_MMAP,
            addr as u64,
            len as u64,
            prot as u64,
            flags as u64,
            fd as i64 as u64,
            offset,
        )
    };
    unsafe { decode(ret) }
}

pub unsafe fn sys_munmap(addr: usize, len: usize) -> Result<(), i32> {
    let ret = unsafe { syscall6(SYS_MUNMAP, addr as u64, len as u64, 0, 0, 0, 0) };
    unsafe { decode(ret).map(|_| ()) }
}

pub unsafe fn sys_mprotect(addr: usize, len: usize, prot: u32) -> Result<(), i32> {
    let ret = unsafe { syscall6(SYS_MPROTECT, addr as u64, len as u64, prot as u64, 0, 0, 0) };
    unsafe { decode(ret).map(|_| ()) }
}

pub unsafe fn sys_open_ro(path_cstr: *const u8) -> Result<i32, i32> {
    let ret = unsafe { syscall6(SYS_OPEN, path_cstr as u64, O_RDONLY as u64, 0, 0, 0, 0) };
    unsafe { decode(ret).map(|fd| fd as i32) }
}

pub unsafe fn sys_close(fd: i32) -> Result<(), i32> {
    let ret = unsafe { syscall6(SYS_CLOSE, fd as i64 as u64, 0, 0, 0, 0, 0) };
    unsafe { decode(ret).map(|_| ()) }
}

pub unsafe fn sys_lseek(fd: i32, offset: i64, whence: i32) -> Result<usize, i32> {
    let ret = unsafe { syscall6(SYS_LSEEK, fd as i64 as u64, offset as u64, whence as u64, 0, 0, 0) };
    unsafe { decode(ret) }
}

pub unsafe fn sys_pread_exact(fd: i32, buf: &mut [u8], mut offset: u64) -> Result<(), i32> {
    let mut done = 0usize;
    while done < buf.len() {
        let ret = unsafe {
            syscall6(
                SYS_PREAD64,
                fd as i64 as u64,
                buf.as_mut_ptr().add(done) as u64,
                (buf.len() - done) as u64,
                offset,
                0,
                0,
            )
        };
        if ret < 0 {
            let err = (-ret) as i32;
            if err as i64 == EINTR {
                continue;
            }
            return Err(err);
        }
        if ret == 0 {
            return Err(5); // EIO (unexpected EOF)
        }
        let n = ret as usize;
        done += n;
        offset += n as u64;
    }
    Ok(())
}

pub unsafe fn sys_arch_set_fs(fs_base: u64) -> Result<(), i32> {
    let ret = unsafe { syscall6(SYS_ARCH_PRCTL, ARCH_SET_FS, fs_base, 0, 0, 0, 0) };
    unsafe { decode(ret).map(|_| ()) }
}

pub unsafe fn sys_arch_get_fs() -> Result<u64, i32> {
    let mut out = 0u64;
    let ret = unsafe {
        syscall6(
            SYS_ARCH_PRCTL,
            ARCH_GET_FS,
            &mut out as *mut u64 as u64,
            0,
            0,
            0,
            0,
        )
    };
    unsafe { decode(ret).map(|_| out) }
}

pub fn sys_log(bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    unsafe {
        let _ = syscall6(SYS_EXO_LOG, bytes.as_ptr() as u64, bytes.len() as u64, 0, 0, 0, 0);
    }
}

pub fn sys_exit_group(code: i32) -> ! {
    unsafe {
        let _ = syscall6(SYS_EXIT_GROUP, code as i64 as u64, 0, 0, 0, 0, 0);
        let _ = syscall6(SYS_EXIT, code as i64 as u64, 0, 0, 0, 0, 0);
    }
    loop {
        core::hint::spin_loop();
    }
}
