//! Primitives ExoFS natives (`500..=520`) et I/O de descripteurs (`read`/`write`/`open`/`close`).

use crate::{abi, decode_ret, SysError, SysResult};

pub use abi::{
    ExoCapTokenWire, ExofsOpenArgs, ExofsPathResolveResult, EXOFS_RIGHT_ADMIN, EXOFS_RIGHT_ALL,
    EXOFS_RIGHT_CREATE, EXOFS_RIGHT_DELETE, EXOFS_RIGHT_EXEC, EXOFS_RIGHT_LIST, EXOFS_RIGHT_READ,
    EXOFS_RIGHT_READ_ONLY, EXOFS_RIGHT_READ_WRITE, EXOFS_RIGHT_SETMETA, EXOFS_RIGHT_STAT,
    EXOFS_RIGHT_WRITE,
};

pub const EXOFS_STAT_LEN: usize = 104;

pub const EXOFS_OPEN_RDONLY: u32 = 0;
pub const EXOFS_OPEN_WRONLY: u32 = 1;
pub const EXOFS_OPEN_RDWR: u32 = 2;
pub const EXOFS_OPEN_CREATE: u32 = 0x0040;
pub const EXOFS_OPEN_EXCL: u32 = 0x0080;
pub const EXOFS_OPEN_TRUNC: u32 = 0x0200;
pub const EXOFS_OPEN_APPEND: u32 = 0x0400;
pub const EXOFS_OPEN_DIRECTORY: u32 = 0x0001_0000;

pub const EXOFS_SEEK_SET: u32 = 0;
pub const EXOFS_SEEK_CUR: u32 = 1;
pub const EXOFS_SEEK_END: u32 = 2;

pub const EXOFS_MODE_FILE: u32 = 0o100644;
pub const EXOFS_MODE_DIR: u32 = 0o040755;
pub const EXOFS_MODE_SYMLINK: u32 = 0o120777;

/// Token de capacité utilisateur 48 octets transmis via `AT_CAP_TOKEN = 52` ou `SYS_EXO_CAP_DELEGATE`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct CapTokenUser {
    pub raw: [u8; 48],
}

impl Default for CapTokenUser {
    fn default() -> Self {
        Self { raw: [0u8; 48] }
    }
}

impl CapTokenUser {
    #[inline]
    pub fn generation(&self) -> u32 {
        u32::from_le_bytes([self.raw[32], self.raw[33], self.raw[34], self.raw[35]])
    }

    #[inline]
    pub fn rights(&self) -> u32 {
        u32::from_le_bytes([self.raw[36], self.raw[37], self.raw[38], self.raw[39]])
    }

    #[inline]
    pub fn owner_pid(&self) -> u32 {
        u32::from_le_bytes([self.raw[40], self.raw[41], self.raw[42], self.raw[43]])
    }
}

/// Statut d'objet ExoFS (104 octets — `EXOFS_STAT_LEN`).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ExoFsFileStat {
    pub object_id: [u8; 32],
    pub content_hash: [u8; 32],
    pub size: u64,
    pub mode: u32,
    pub flags: u32,
    pub ctime_ns: u64,
    pub mtime_ns: u64,
    pub generation: u64,
}

const _: () = assert!(core::mem::size_of::<ExoFsFileStat>() == EXOFS_STAT_LEN);

impl Default for ExoFsFileStat {
    fn default() -> Self {
        Self {
            object_id: [0u8; 32],
            content_hash: [0u8; 32],
            size: 0,
            mode: 0,
            flags: 0,
            ctime_ns: 0,
            mtime_ns: 0,
            generation: 0,
        }
    }
}

/// Entrée de répertoire ExoFS retournée par `SYS_EXOFS_READDIR (520)`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ExoDirEntry {
    pub inode: u64,
    pub file_type: u8,
    pub name_len: u8,
    pub name: [u8; 246],
}

impl Default for ExoDirEntry {
    fn default() -> Self {
        Self {
            inode: 0,
            file_type: 0,
            name_len: 0,
            name: [0u8; 246],
        }
    }
}

impl ExoDirEntry {
    #[inline]
    pub fn name_str(&self) -> &str {
        let len = (self.name_len as usize).min(self.name.len());
        let actual_len = self.name[..len]
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(len);
        core::str::from_utf8(&self.name[..actual_len]).unwrap_or("")
    }
}

pub const O_CLOEXEC: i32 = 0o2000000;

#[inline]
pub fn pipe2(fds: &mut [i32; 2], flags: i32) -> SysResult<()> {
    let ret = unsafe {
        abi::syscall2(
            abi::SYS_PIPE2,
            fds.as_mut_ptr() as u64,
            flags as u32 as u64,
        )
    };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn dup2(oldfd: i32, newfd: i32) -> SysResult<i32> {
    let ret = unsafe { abi::syscall2(abi::SYS_DUP2, oldfd as u64, newfd as u64) };
    decode_ret(ret).map(|fd| fd as i32)
}

#[inline]
pub fn getcwd(buf: &mut [u8]) -> SysResult<usize> {
    let ret = unsafe { abi::syscall2(abi::SYS_GETCWD, buf.as_mut_ptr() as u64, buf.len() as u64) };
    decode_ret(ret)
}

#[inline]
pub fn chdir(path_cstr: &[u8]) -> SysResult<()> {
    let ret = unsafe { abi::syscall1(abi::SYS_CHDIR, path_cstr.as_ptr() as u64) };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn read(fd: i32, buf: &mut [u8]) -> SysResult<usize> {
    let ret = unsafe {
        abi::syscall3(
            abi::SYS_READ,
            fd as u64,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        )
    };
    decode_ret(ret)
}

#[inline]
pub fn write(fd: i32, buf: &[u8]) -> SysResult<usize> {
    let ret = unsafe {
        abi::syscall3(
            abi::SYS_WRITE,
            fd as u64,
            buf.as_ptr() as u64,
            buf.len() as u64,
        )
    };
    decode_ret(ret)
}

#[inline]
pub fn close(fd: i32) -> SysResult<()> {
    let ret = unsafe { abi::syscall1(abi::SYS_CLOSE, fd as u64) };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn exofs_open(path_cstr: &[u8], flags: u32, mode: u32) -> SysResult<i32> {
    if path_cstr.is_empty() {
        return Err(SysError::ENOENT);
    }
    let cap_rights = if (flags & (EXOFS_OPEN_WRONLY | EXOFS_OPEN_RDWR | EXOFS_OPEN_CREATE)) != 0 {
        EXOFS_RIGHT_READ_WRITE
    } else {
        EXOFS_RIGHT_READ_ONLY
    };
    let ret = unsafe {
        abi::exofs_open_by_path_raw(
            path_cstr.as_ptr() as u64,
            flags as u64,
            mode as u64,
            cap_rights as u64,
        )
    };
    if ret >= 0 {
        return Ok(ret as i32);
    }
    // Fallback vers SYS_OPEN (2) qui dispatche vers le pont VFS/ExoFS du noyau
    let ret2 = unsafe {
        abi::syscall3(
            abi::SYS_OPEN,
            path_cstr.as_ptr() as u64,
            flags as u64,
            mode as u64,
        )
    };
    decode_ret(ret2).map(|fd| fd as i32)
}

#[inline]
pub fn exofs_read(fd: i32, buf: &mut [u8]) -> SysResult<usize> {
    read(fd, buf)
}

#[inline]
pub fn exofs_write(fd: i32, buf: &[u8]) -> SysResult<usize> {
    write(fd, buf)
}

#[inline]
pub fn exofs_close(fd: i32) -> SysResult<()> {
    close(fd)
}

#[inline]
pub fn exofs_stat(path_cstr: &[u8]) -> SysResult<ExoFsFileStat> {
    if path_cstr.is_empty() {
        return Err(SysError::ENOENT);
    }
    let trimmed_len = path_cstr
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(path_cstr.len());
    let mut resolved = ExofsPathResolveResult::default();
    let ret = unsafe {
        abi::exofs_path_resolve_raw(
            path_cstr.as_ptr() as u64,
            trimmed_len as u64,
            0,
            &mut resolved,
            EXOFS_RIGHT_STAT as u64,
        )
    };
    decode_ret(ret).map(|_| ExoFsFileStat {
        object_id: resolved.object_id,
        content_hash: resolved.blob_id,
        size: resolved.size_bytes,
        mode: if resolved.object_kind == 2 {
            EXOFS_MODE_DIR
        } else {
            EXOFS_MODE_FILE
        },
        flags: resolved.flags,
        ctime_ns: 0,
        mtime_ns: 0,
        generation: resolved.epoch_id,
    })
}

#[inline]
pub fn exofs_fstat(fd: i32) -> SysResult<ExoFsFileStat> {
    let mut st = ExoFsFileStat::default();
    let ret = unsafe {
        abi::syscall3(
            abi::SYS_EXOFS_OBJECT_STAT,
            fd as u64,
            &mut st as *mut ExoFsFileStat as u64,
            EXOFS_STAT_LEN as u64,
        )
    };
    decode_ret(ret).map(|_| st)
}

#[inline]
pub fn exofs_lseek(fd: i32, offset: i64, whence: u32) -> SysResult<u64> {
    let ret = unsafe {
        abi::syscall3(
            abi::SYS_LSEEK,
            fd as u64,
            offset as u64,
            whence as u64,
        )
    };
    decode_ret(ret).map(|pos| pos as u64)
}

#[inline]
pub fn exofs_mkdir(path_cstr: &[u8], mode: u32) -> SysResult<()> {
    let ret = unsafe { abi::syscall2(abi::SYS_MKDIR, path_cstr.as_ptr() as u64, mode as u64) };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn exofs_unlink(path_cstr: &[u8], flags: u32) -> SysResult<()> {
    let nr = if flags != 0 {
        abi::SYS_RMDIR
    } else {
        abi::SYS_UNLINK
    };
    let ret = unsafe { abi::syscall1(nr, path_cstr.as_ptr() as u64) };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn exofs_rename(old_cstr: &[u8], new_cstr: &[u8]) -> SysResult<()> {
    let ret = unsafe {
        abi::syscall2(
            abi::SYS_RENAME,
            old_cstr.as_ptr() as u64,
            new_cstr.as_ptr() as u64,
        )
    };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn exofs_readdir(fd: i32, entries: &mut [ExoDirEntry]) -> SysResult<usize> {
    let byte_len = core::mem::size_of_val(entries);
    let ret = unsafe {
        abi::exofs_readdir_raw(
            fd as u64,
            entries.as_mut_ptr() as u64,
            byte_len as u64,
            EXOFS_RIGHT_LIST as u64,
        )
    };
    decode_ret(ret)
}

#[inline]
pub fn exofs_ftruncate(fd: i32, length: u64) -> SysResult<()> {
    let ret = unsafe { abi::syscall2(abi::SYS_FTRUNCATE, fd as u64, length) };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn exofs_fsync(fd: i32) -> SysResult<()> {
    let ret = unsafe { abi::syscall1(abi::SYS_FSYNC, fd as u64) };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn exofs_delegate(
    parent_tok: &CapTokenUser,
    rights_mask: u32,
    target_pid: u32,
) -> SysResult<CapTokenUser> {
    let mut child = CapTokenUser::default();
    let ret = unsafe {
        abi::syscall4(
            abi::SYS_EXO_CAP_DELEGATE,
            parent_tok as *const CapTokenUser as u64,
            rights_mask as u64,
            target_pid as u64,
            &mut child as *mut CapTokenUser as u64,
        )
    };
    decode_ret(ret).map(|_| child)
}
