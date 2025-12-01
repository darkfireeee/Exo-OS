//! System Information and Utility Syscalls
//!
//! Implements uname, sysinfo, umask, getrandom.

use core::ptr;

// UtsName structure for uname
#[repr(C)]
pub struct UtsName {
    pub sysname: [u8; 65],
    pub nodename: [u8; 65],
    pub release: [u8; 65],
    pub version: [u8; 65],
    pub machine: [u8; 65],
    pub domainname: [u8; 65],
}

// SysInfo structure for sysinfo
#[repr(C)]
pub struct SysInfo {
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
    pub totalhigh: u64,
    pub freehigh: u64,
    pub mem_unit: u32,
    pub _f: [u8; 0],
}

/// Get system information (name, version, etc.)
pub unsafe fn sys_uname(buf: *mut UtsName) -> i64 {
    if buf.is_null() {
        return -14; // EFAULT
    }

    log::info!("sys_uname: buf={:?}", buf);

    // Fill in system information
    let uts = &mut *buf;

    // Zero out the structure
    ptr::write_bytes(uts as *mut UtsName, 0, 1);

    // Fill in the fields
    copy_str(&mut uts.sysname, b"Exo-OS");
    copy_str(&mut uts.nodename, b"exo-kernel");
    copy_str(&mut uts.release, b"0.2.0");
    copy_str(&mut uts.version, b"#1 SMP PREEMPT");
    copy_str(&mut uts.machine, b"x86_64");
    copy_str(&mut uts.domainname, b"(none)");

    0
}

/// Get system statistics
pub unsafe fn sys_sysinfo(info: *mut SysInfo) -> i64 {
    if info.is_null() {
        return -14; // EFAULT
    }

    log::info!("sys_sysinfo: info={:?}", info);

    let sysinfo = &mut *info;

    // Zero out the structure
    ptr::write_bytes(sysinfo as *mut SysInfo, 0, 1);

    // Fill in basic info (stubs)
    sysinfo.uptime = 3600; // 1 hour uptime
    sysinfo.loads[0] = 0;
    sysinfo.loads[1] = 0;
    sysinfo.loads[2] = 0;
    sysinfo.totalram = 1024 * 1024 * 1024; // 1GB
    sysinfo.freeram = 512 * 1024 * 1024; // 512MB
    sysinfo.sharedram = 0;
    sysinfo.bufferram = 0;
    sysinfo.totalswap = 0;
    sysinfo.freeswap = 0;
    sysinfo.procs = 1;
    sysinfo.totalhigh = 0;
    sysinfo.freehigh = 0;
    sysinfo.mem_unit = 1;

    0
}

/// Get or set the file creation mask
pub unsafe fn sys_umask(mask: u32) -> i64 {
    log::info!("sys_umask: mask={:#o}", mask);
    // TODO: Store per-process umask
    // For now, return default umask (0022)
    0o022
}

/// Get random bytes
pub unsafe fn sys_getrandom(buf: *mut u8, buflen: usize, _flags: u32) -> i64 {
    if buf.is_null() {
        return -14; // EFAULT
    }

    log::info!("sys_getrandom: buflen={}, flags={:#x}", buflen, _flags);

    // Fill buffer with pseudo-random data
    // TODO: Use proper CSPRNG or hardware RNG
    for i in 0..buflen {
        *buf.add(i) = (i as u8).wrapping_mul(73).wrapping_add(37);
    }

    buflen as i64
}

// Helper function to copy a string into a fixed-size buffer
fn copy_str(dest: &mut [u8], src: &[u8]) {
    let len = core::cmp::min(dest.len() - 1, src.len());
    dest[..len].copy_from_slice(&src[..len]);
    dest[len] = 0; // Null terminator
}
