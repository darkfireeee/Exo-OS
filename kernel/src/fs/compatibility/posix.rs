// kernel/src/fs/compatibility/posix.rs
//
// 
// COMPATIBILITÉ POSIX.1-2024  couche de conformité stricte (Exo-OS  Couche 3)
// 

use core::sync::atomic::{AtomicU64, Ordering};
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::fs::core::types::{
    FsError, FsResult, OpenFlags, FileMode, FileType, InodeNumber,
    Stat, SeekWhence, Timespec64, Uid, Gid, FS_STATS,
};
use crate::fs::core::vfs::{path_lookup, LookupContext, FileHandle, FileOps, DefaultFileOps, RenameFlags};
use crate::fs::core::descriptor::{Fd, FdTable, FdEntry};
use crate::security::capability::CapToken;

// helper local
macro_rules! check_cap {
    ($cap:expr) => {
        if $cap.is_invalid() { return Err(FsError::PermissionDenied); }
    };
}

fn lookup(path: &str) -> FsResult<crate::fs::core::vfs::LookupResult> {
    use crate::security::capability::table::CapTable;
    let cap_table = CapTable::new();
    let ctx = LookupContext {
        start_dir: None,
        cap_table: &cap_table,
        uid: Uid(0),
        gid: Gid(0),
        nofollow: false,
        symlink_count: 0,
    };
    path_lookup(path.as_bytes(), &ctx)
}

// 
// posix_open
// 

pub fn posix_open(
    cap:   &CapToken,
    path:  &str,
    flags: OpenFlags,
    _mode: FileMode,
    fds:   &FdTable,
) -> FsResult<Fd> {
    check_cap!(cap);
    if path.is_empty() { return Err(FsError::InvalidArgument); }

    let result    = lookup(path)?;
    let inode     = result.inode.clone();

    if flags.create_on_missing() && flags.excl() {
        return Err(FsError::Exists);
    }

    let _inode_ops = inode.read().ops.clone().ok_or(FsError::NotSupported)?;
    let file_ops: Arc<dyn FileOps> = Arc::new(DefaultFileOps);
    let handle    = Arc::new(FileHandle::new(inode, file_ops, flags));
    let entry     = FdEntry::new(handle);
    let fd = fds.alloc_fd(entry, 0)?;

    POSIX_STATS.open_calls.fetch_add(1, Ordering::Relaxed);
    FS_STATS.open_files.fetch_add(1, Ordering::Relaxed);
    Ok(fd)
}

// 
// posix_close
// 

pub fn posix_close(cap: &CapToken, fd: Fd, fds: &FdTable) -> FsResult<()> {
    check_cap!(cap);
    fds.close_fd(fd)?;
    POSIX_STATS.close_calls.fetch_add(1, Ordering::Relaxed);
    FS_STATS.open_files.fetch_sub(1, Ordering::Relaxed);
    Ok(())
}

// 
// posix_read
// 

pub fn posix_read(
    cap: &CapToken,
    fd:  Fd,
    buf: &mut [u8],
    fds: &FdTable,
) -> FsResult<usize> {
    check_cap!(cap);
    if buf.is_empty() { return Ok(0); }
    let entry = fds.get(fd)?;
    let offset = entry.handle.pos.load(Ordering::Relaxed);
    let n = entry.handle.ops.read(&entry.handle, buf, offset)?;
    entry.handle.pos.fetch_add(n as u64, Ordering::Relaxed);
    POSIX_STATS.read_calls.fetch_add(1, Ordering::Relaxed);
    POSIX_STATS.bytes_read.fetch_add(n as u64, Ordering::Relaxed);
    Ok(n)
}

// 
// posix_write
// 

pub fn posix_write(
    cap: &CapToken,
    fd:  Fd,
    buf: &[u8],
    fds: &FdTable,
) -> FsResult<usize> {
    check_cap!(cap);
    if buf.is_empty() { return Ok(0); }
    let entry = fds.get(fd)?;
    let offset = entry.handle.pos.load(Ordering::Relaxed);
    let n = entry.handle.ops.write(&entry.handle, buf, offset)?;
    entry.handle.pos.fetch_add(n as u64, Ordering::Relaxed);
    POSIX_STATS.write_calls.fetch_add(1, Ordering::Relaxed);
    POSIX_STATS.bytes_written.fetch_add(n as u64, Ordering::Relaxed);
    Ok(n)
}

// 
// posix_lseek
// 

pub fn posix_lseek(
    cap:    &CapToken,
    fd:     Fd,
    offset: i64,
    whence: SeekWhence,
    fds:    &FdTable,
) -> FsResult<u64> {
    check_cap!(cap);
    let entry   = fds.get(fd)?;
    let new_pos = entry.handle.ops.seek(&entry.handle, offset, whence)?;
    entry.handle.pos.store(new_pos, Ordering::Relaxed);
    POSIX_STATS.seek_calls.fetch_add(1, Ordering::Relaxed);
    Ok(new_pos)
}

// 
// posix_stat / posix_fstat / posix_lstat
// 

pub fn posix_stat(cap: &CapToken, path: &str) -> FsResult<Stat> {
    check_cap!(cap);
    if path.is_empty() { return Err(FsError::InvalidArgument); }
    let result = lookup(path)?;
    let inode  = result.inode.clone();
    let ops    = inode.read().ops.clone().ok_or(FsError::NotSupported)?;
    let stat   = ops.getattr(&inode)?;
    POSIX_STATS.stat_calls.fetch_add(1, Ordering::Relaxed);
    Ok(stat)
}

pub fn posix_fstat(cap: &CapToken, fd: Fd, fds: &FdTable) -> FsResult<Stat> {
    check_cap!(cap);
    let entry = fds.get(fd)?;
    let ops   = entry.handle.inode.read().ops.clone().ok_or(FsError::NotSupported)?;
    let stat  = ops.getattr(&entry.handle.inode)?;
    POSIX_STATS.stat_calls.fetch_add(1, Ordering::Relaxed);
    Ok(stat)
}

pub fn posix_lstat(cap: &CapToken, path: &str) -> FsResult<Stat> {
    posix_stat(cap, path)
}

// 
// posix_chmod / posix_chown
// 

pub fn posix_chmod(cap: &CapToken, path: &str, mode: FileMode) -> FsResult<()> {
    check_cap!(cap);
    if path.is_empty() { return Err(FsError::InvalidArgument); }
    let result = lookup(path)?;
    let inode  = result.inode.clone();
    inode.write().mode = mode;
    POSIX_STATS.chmod_calls.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

pub fn posix_chown(cap: &CapToken, path: &str, uid: u32, gid: u32) -> FsResult<()> {
    check_cap!(cap);
    if path.is_empty() { return Err(FsError::InvalidArgument); }
    let result = lookup(path)?;
    let inode  = result.inode.clone();
    {
        let mut wr = inode.write();
        wr.uid = Uid(uid);
        wr.gid = Gid(gid);
    }
    POSIX_STATS.chown_calls.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

// 
// posix_mkdir / posix_rmdir / posix_unlink
// 

pub fn posix_mkdir(cap: &CapToken, path: &str, mode: FileMode) -> FsResult<()> {
    check_cap!(cap);
    if path.is_empty() { return Err(FsError::InvalidArgument); }
    let (parent_path, name) = split_path(path)?;
    let parent_result = lookup(parent_path)?;
    let parent_inode  = parent_result.inode.clone();
    let ops = parent_inode.read().ops.clone().ok_or(FsError::NotSupported)?;
    ops.mkdir(&parent_inode, name.as_bytes(), mode, Uid(0), Gid(0))?;
    POSIX_STATS.mkdir_calls.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

pub fn posix_rmdir(cap: &CapToken, path: &str) -> FsResult<()> {
    check_cap!(cap);
    let (parent_path, name) = split_path(path)?;
    let parent_result = lookup(parent_path)?;
    let parent_inode  = parent_result.inode.clone();
    let ops = parent_inode.read().ops.clone().ok_or(FsError::NotSupported)?;
    ops.rmdir(&parent_inode, name.as_bytes())?;
    POSIX_STATS.rmdir_calls.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

pub fn posix_unlink(cap: &CapToken, path: &str) -> FsResult<()> {
    check_cap!(cap);
    let (parent_path, name) = split_path(path)?;
    let parent_result = lookup(parent_path)?;
    let parent_inode  = parent_result.inode.clone();
    let ops = parent_inode.read().ops.clone().ok_or(FsError::NotSupported)?;
    ops.unlink(&parent_inode, name.as_bytes())?;
    POSIX_STATS.unlink_calls.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

// 
// posix_rename
// 

pub fn posix_rename(cap: &CapToken, old_path: &str, new_path: &str) -> FsResult<()> {
    check_cap!(cap);
    if old_path.is_empty() || new_path.is_empty() { return Err(FsError::InvalidArgument); }
    let (op, on) = split_path(old_path)?;
    let (np, nn) = split_path(new_path)?;
    let old_result = lookup(op)?;
    let new_result = lookup(np)?;
    let old_inode  = old_result.inode.clone();
    let new_inode  = new_result.inode.clone();
    let ops = old_inode.read().ops.clone().ok_or(FsError::NotSupported)?;
    ops.rename(&old_inode, on.as_bytes(), &new_inode, nn.as_bytes(), RenameFlags(0))?;
    POSIX_STATS.rename_calls.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

// 
// posix_fsync / posix_fdatasync
// 

pub fn posix_fsync(cap: &CapToken, fd: Fd, data_only: bool, fds: &FdTable) -> FsResult<()> {
    check_cap!(cap);
    let entry = fds.get(fd)?;
    entry.handle.ops.fsync(&entry.handle, data_only)?;
    POSIX_STATS.fsync_calls.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

// 
// split_path
// 

fn split_path(path: &str) -> FsResult<(&str, &str)> {
    let sep = path.rfind('/').ok_or(FsError::InvalidArgument)?;
    if sep == 0 {
        Ok(("/", &path[1..]))
    } else {
        Ok((&path[..sep], &path[sep + 1..]))
    }
}

// 
// PosixStats
// 

pub struct PosixStats {
    pub open_calls:    AtomicU64,
    pub close_calls:   AtomicU64,
    pub read_calls:    AtomicU64,
    pub write_calls:   AtomicU64,
    pub seek_calls:    AtomicU64,
    pub stat_calls:    AtomicU64,
    pub chmod_calls:   AtomicU64,
    pub chown_calls:   AtomicU64,
    pub mkdir_calls:   AtomicU64,
    pub rmdir_calls:   AtomicU64,
    pub unlink_calls:  AtomicU64,
    pub rename_calls:  AtomicU64,
    pub fsync_calls:   AtomicU64,
    pub bytes_read:    AtomicU64,
    pub bytes_written: AtomicU64,
}

impl PosixStats {
    pub const fn new() -> Self {
        macro_rules! z { () => { AtomicU64::new(0) }; }
        Self {
            open_calls:    z!(), close_calls:   z!(),
            read_calls:    z!(), write_calls:   z!(),
            seek_calls:    z!(), stat_calls:    z!(),
            chmod_calls:   z!(), chown_calls:   z!(),
            mkdir_calls:   z!(), rmdir_calls:   z!(),
            unlink_calls:  z!(), rename_calls:  z!(),
            fsync_calls:   z!(),
            bytes_read:    z!(), bytes_written: z!(),
        }
    }
}

pub static POSIX_STATS: PosixStats = PosixStats::new();
