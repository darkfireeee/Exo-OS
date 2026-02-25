// kernel/src/fs/compatibility/linux_compat.rs
//
// COMPATIBILITE LINUX -- syscalls etendus (Exo-OS Couche 3)

use core::sync::atomic::{AtomicU64, Ordering};
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::fs::core::types::{
    FsError, FsResult, OpenFlags, FileMode, FileType, InodeNumber,
    Stat, SeekWhence, Timespec64, FS_STATS,
};
use crate::fs::core::vfs::{FileHandle, InodeAttr};
use crate::fs::core::descriptor::{Fd, FdTable};
use crate::fs::compatibility::posix::{posix_stat, posix_read, posix_write, posix_lseek};
use crate::security::capability::CapToken;

macro_rules! check_cap {
    ($cap:expr) => {
        if $cap.is_invalid() { return Err(FsError::PermissionDenied); }
    };
}

// 
// Statx
// 

bitflags::bitflags! {
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct StatxMask: u32 {
        const TYPE       = 0x0001;
        const MODE       = 0x0002;
        const NLINK      = 0x0004;
        const UID        = 0x0008;
        const GID        = 0x0010;
        const ATIME      = 0x0020;
        const MTIME      = 0x0040;
        const CTIME      = 0x0080;
        const INO        = 0x0100;
        const SIZE       = 0x0200;
        const BLOCKS     = 0x0400;
        const BTIME      = 0x0800;
    }
}

#[derive(Clone, Default)]
pub struct Statx {
    pub mask:         u32,
    pub blksize:      u32,
    pub attributes:   u64,
    pub nlink:        u32,
    pub uid:          u32,
    pub gid:          u32,
    pub file_type:    u16,
    pub mode:         u16,
    pub ino:          u64,
    pub size:         u64,
    pub blocks:       u64,
    pub atime:        Timespec64,
    pub btime:        Timespec64,
    pub ctime:        Timespec64,
    pub mtime:        Timespec64,
    pub rdev_major:   u32,
    pub rdev_minor:   u32,
    pub dev_major:    u32,
    pub dev_minor:    u32,
}

pub fn linux_statx(cap: &CapToken, path: &str, mask: StatxMask) -> FsResult<Statx> {
    check_cap!(cap);
    if path.is_empty() { return Err(FsError::InvalidArgument); }
    let stat = posix_stat(cap, path)?;
    let rdev = stat.st_rdev.0 as u64;
    let dev  = stat.st_dev.0 as u64;
    let stx = Statx {
        mask:       mask.bits(),
        blksize:    512,
        nlink:      stat.st_nlink,
        uid:        stat.st_uid,
        gid:        stat.st_gid,
        file_type:  stat.st_mode.file_type() as u16,
        mode:       stat.st_mode.0 as u16,
        ino:        stat.st_ino.0,
        size:       stat.st_size as u64,
        blocks:     stat.st_blocks as u64,
        atime:      stat.st_atim,
        ctime:      stat.st_ctim,
        mtime:      stat.st_mtim,
        btime:      Timespec64::default(),
        rdev_major: (rdev >> 20) as u32,
        rdev_minor: (rdev & 0xFFFFF) as u32,
        dev_major:  (dev >> 20) as u32,
        dev_minor:  (dev & 0xFFFFF) as u32,
        attributes: 0,
    };
    LINUX_STATS.statx_calls.fetch_add(1, Ordering::Relaxed);
    Ok(stx)
}

// 
// openat
// 

pub fn linux_openat(
    cap:   &CapToken,
    _dirfd: Fd,
    path:  &str,
    flags: OpenFlags,
    mode:  FileMode,
    fds:   &FdTable,
) -> FsResult<Fd> {
    crate::fs::compatibility::posix::posix_open(cap, path, flags, mode, fds)
}

// 
// copy_file_range
// 

pub fn linux_copy_file_range(
    cap:     &CapToken,
    fd_in:   Fd,
    off_in:  Option<&mut u64>,
    fd_out:  Fd,
    off_out: Option<&mut u64>,
    count:   usize,
    fds:     &FdTable,
) -> FsResult<usize> {
    check_cap!(cap);
    if count == 0 { return Ok(0); }
    const CHUNK: usize = 65536;
    let mut buf = alloc::vec![0u8; count.min(CHUNK)];
    let mut total = 0usize;
    let mut remaining = count;

    let entry_in  = fds.get(fd_in)?;
    let entry_out = fds.get(fd_out)?;

    while remaining > 0 {
        let to_read = remaining.min(CHUNK);
        let r_off   = if let Some(o) = &off_in  { **o } else { entry_in.handle.pos.load(Ordering::Relaxed) };
        let n = entry_in.handle.ops.read(&entry_in.handle, &mut buf[..to_read], r_off)?;
        if n == 0 { break; }

        let w_off = if let Some(o) = &off_out { **o } else { entry_out.handle.pos.load(Ordering::Relaxed) };
        let w = entry_out.handle.ops.write(&entry_out.handle, &buf[..n], w_off)?;

        if off_in.is_none()  { entry_in.handle.pos.fetch_add(n as u64, Ordering::Relaxed); }
        if off_out.is_none() { entry_out.handle.pos.fetch_add(w as u64, Ordering::Relaxed); }

        total     += w;
        remaining -= n;
        if w < n  { break; }
    }
    LINUX_STATS.copy_file_range_calls.fetch_add(1, Ordering::Relaxed);
    LINUX_STATS.bytes_copied.fetch_add(total as u64, Ordering::Relaxed);
    Ok(total)
}

// 
// IoVec pour preadv2 / pwritev2
// 

pub struct IoVec<'a> { pub base: &'a mut [u8] }
pub struct IoVecConst<'a> { pub base: &'a [u8] }

pub fn linux_preadv2(
    cap:    &CapToken,
    fd:     Fd,
    vecs:   &mut [IoVec<'_>],
    offset: i64,
    _flags: u32,
    fds:    &FdTable,
) -> FsResult<usize> {
    check_cap!(cap);
    let entry = fds.get(fd)?;
    let mut off = if offset < 0 { entry.handle.pos.load(Ordering::Relaxed) } else { offset as u64 };
    let mut total = 0usize;
    for v in vecs.iter_mut() {
        let n = entry.handle.ops.read(&entry.handle, v.base, off)?;
        off   += n as u64;
        total += n;
        if n < v.base.len() { break; }
    }
    if offset < 0 { entry.handle.pos.store(off, Ordering::Relaxed); }
    LINUX_STATS.preadv2_calls.fetch_add(1, Ordering::Relaxed);
    Ok(total)
}

pub fn linux_pwritev2(
    cap:    &CapToken,
    fd:     Fd,
    vecs:   &[IoVecConst<'_>],
    offset: i64,
    _flags: u32,
    fds:    &FdTable,
) -> FsResult<usize> {
    check_cap!(cap);
    let entry = fds.get(fd)?;
    let mut off = if offset < 0 { entry.handle.pos.load(Ordering::Relaxed) } else { offset as u64 };
    let mut total = 0usize;
    for v in vecs.iter() {
        let n = entry.handle.ops.write(&entry.handle, v.base, off)?;
        off   += n as u64;
        total += n;
        if n < v.base.len() { break; }
    }
    if offset < 0 { entry.handle.pos.store(off, Ordering::Relaxed); }
    LINUX_STATS.pwritev2_calls.fetch_add(1, Ordering::Relaxed);
    Ok(total)
}

// 
// fallocate
// 

pub fn linux_fallocate(
    cap:    &CapToken,
    fd:     Fd,
    mode:   u32,
    offset: u64,
    len:    u64,
    fds:    &FdTable,
) -> FsResult<()> {
    check_cap!(cap);
    if len == 0 { return Err(FsError::InvalidArgument); }
    let entry = fds.get(fd)?;
    entry.handle.ops.fallocate(&entry.handle, mode, offset, len)?;
    LINUX_STATS.fallocate_calls.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

// 
// ftruncate
// 

pub fn linux_ftruncate(cap: &CapToken, fd: Fd, size: u64, fds: &FdTable) -> FsResult<()> {
    check_cap!(cap);
    let entry = fds.get(fd)?;
    let inode = &entry.handle.inode;
    let ops   = inode.read().ops.clone().ok_or(FsError::NotSupported)?;
    let attr  = InodeAttr::new().with_size(size);
    ops.setattr(inode, &attr)?;
    LINUX_STATS.ftruncate_calls.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

// 
// LinuxCompatStats
// 

pub struct LinuxCompatStats {
    pub statx_calls:            AtomicU64,
    pub openat_calls:           AtomicU64,
    pub copy_file_range_calls:  AtomicU64,
    pub bytes_copied:           AtomicU64,
    pub preadv2_calls:          AtomicU64,
    pub pwritev2_calls:         AtomicU64,
    pub fallocate_calls:        AtomicU64,
    pub ftruncate_calls:        AtomicU64,
}

impl LinuxCompatStats {
    pub const fn new() -> Self {
        macro_rules! z { () => { AtomicU64::new(0) }; }
        Self {
            statx_calls:           z!(),
            openat_calls:          z!(),
            copy_file_range_calls: z!(),
            bytes_copied:          z!(),
            preadv2_calls:         z!(),
            pwritev2_calls:        z!(),
            fallocate_calls:       z!(),
            ftruncate_calls:       z!(),
        }
    }
}

pub static LINUX_STATS: LinuxCompatStats = LinuxCompatStats::new();
