use crate::ffi::OsString;
use crate::fs::TryLockError;
use crate::hash::Hash;
use crate::io::{self, BorrowedCursor, IoSlice, IoSliceMut, SeekFrom};
use crate::os::exo::ffi::{OsStrExt, OsStringExt};
use crate::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, RawFd};
use crate::path::{Path, PathBuf};
use crate::sys::fd::FileDesc;
pub use crate::sys::fs::common::{Dir, copy, exists, remove_dir_all};
use crate::sys::pal::syscall::{self, ExoDirEntry, ExoFsFileStat};
use crate::sys::pal::{cstr_bytes, map_exo_err, unsupported};
use crate::sys::time::SystemTime;
use crate::sys::{AsInner, AsInnerMut, FromInner, IntoInner};
use crate::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct FileAttr {
    pub(crate) stat: ExoFsFileStat,
}

impl FileAttr {
    pub fn size(&self) -> u64 {
        self.stat.size
    }

    pub fn perm(&self) -> FilePermissions {
        FilePermissions { mode: self.stat.mode }
    }

    pub fn file_type(&self) -> FileType {
        FileType { mode: self.stat.mode }
    }

    pub fn modified(&self) -> io::Result<SystemTime> {
        let secs = (self.stat.mtime_ns / 1_000_000_000) as i64;
        let nsecs = (self.stat.mtime_ns % 1_000_000_000) as i64;
        Ok(SystemTime::new(secs, nsecs))
    }

    pub fn accessed(&self) -> io::Result<SystemTime> {
        self.modified()
    }

    pub fn created(&self) -> io::Result<SystemTime> {
        let secs = (self.stat.ctime_ns / 1_000_000_000) as i64;
        let nsecs = (self.stat.ctime_ns % 1_000_000_000) as i64;
        Ok(SystemTime::new(secs, nsecs))
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FilePermissions {
    pub(crate) mode: u32,
}

impl FilePermissions {
    pub fn readonly(&self) -> bool {
        (self.mode & 0o222) == 0
    }

    pub fn set_readonly(&mut self, readonly: bool) {
        if readonly {
            self.mode &= !0o222;
        } else {
            self.mode |= 0o200;
        }
    }

    pub fn mode(&self) -> u32 {
        self.mode
    }
}

#[derive(Copy, Clone, Debug, Default)]
pub struct FileTimes {}

impl FileTimes {
    pub fn set_accessed(&mut self, _t: SystemTime) {}
    pub fn set_modified(&mut self, _t: SystemTime) {}
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct FileType {
    pub(crate) mode: u32,
}

impl FileType {
    pub fn is_dir(&self) -> bool {
        (self.mode & 0o170000) == 0o040000
    }

    pub fn is_file(&self) -> bool {
        let kind = self.mode & 0o170000;
        kind == 0o100000 || (kind == 0 && !self.is_dir())
    }

    pub fn is_symlink(&self) -> bool {
        (self.mode & 0o170000) == 0o120000
    }
}

#[derive(Clone, Debug)]
pub struct OpenOptions {
    read: bool,
    write: bool,
    append: bool,
    truncate: bool,
    create: bool,
    create_new: bool,
    mode: u32,
    custom_flags: i32,
}

impl OpenOptions {
    pub fn new() -> OpenOptions {
        OpenOptions {
            read: false,
            write: false,
            append: false,
            truncate: false,
            create: false,
            create_new: false,
            mode: 0o666,
            custom_flags: 0,
        }
    }

    pub fn read(&mut self, read: bool) {
        self.read = read;
    }
    pub fn write(&mut self, write: bool) {
        self.write = write;
    }
    pub fn append(&mut self, append: bool) {
        self.append = append;
    }
    pub fn truncate(&mut self, truncate: bool) {
        self.truncate = truncate;
    }
    pub fn create(&mut self, create: bool) {
        self.create = create;
    }
    pub fn create_new(&mut self, create_new: bool) {
        self.create_new = create_new;
    }
    pub fn custom_flags(&mut self, flags: i32) {
        self.custom_flags = flags;
    }
    pub fn mode(&mut self, mode: u32) {
        self.mode = mode;
    }
}

#[derive(Debug)]
pub struct File(pub(crate) FileDesc);

impl File {
    pub fn open(path: &Path, opts: &OpenOptions) -> io::Result<File> {
        let mut flags = match (opts.read, opts.write, opts.append) {
            (true, false, false) => syscall::EXOFS_OPEN_RDONLY,
            (false, true, false) => syscall::EXOFS_OPEN_WRONLY,
            (true, true, false) => syscall::EXOFS_OPEN_RDWR,
            (false, _, true) => syscall::EXOFS_OPEN_WRONLY | syscall::EXOFS_OPEN_APPEND,
            (true, _, true) => syscall::EXOFS_OPEN_RDWR | syscall::EXOFS_OPEN_APPEND,
            (false, false, false) => return Err(io::Error::from(io::ErrorKind::InvalidInput)),
        };
        if opts.truncate {
            flags |= syscall::EXOFS_OPEN_TRUNC;
        }
        if opts.create {
            flags |= syscall::EXOFS_OPEN_CREATE;
        }
        if opts.create_new {
            flags |= syscall::EXOFS_OPEN_CREATE | syscall::EXOFS_OPEN_EXCL;
        }
        flags |= opts.custom_flags as u32;

        let cpath = cstr_bytes(path.as_os_str().as_bytes())?;
        let fd = syscall::exofs_open(&cpath, flags, opts.mode).map_err(map_exo_err)?;
        Ok(File(unsafe { FileDesc::from_raw_fd(fd) }))
    }

    pub fn file_attr(&self) -> io::Result<FileAttr> {
        let stat = syscall::exofs_fstat(self.as_raw_fd()).map_err(map_exo_err)?;
        Ok(FileAttr { stat })
    }

    pub fn fsync(&self) -> io::Result<()> {
        syscall::exofs_fsync(self.as_raw_fd()).map_err(map_exo_err)
    }

    pub fn datasync(&self) -> io::Result<()> {
        self.fsync()
    }

    pub fn truncate(&self, size: u64) -> io::Result<()> {
        syscall::exofs_ftruncate(self.as_raw_fd(), size).map_err(map_exo_err)
    }

    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }

    pub fn read_vectored(&self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        self.0.read_vectored(bufs)
    }

    #[inline]
    pub fn is_read_vectored(&self) -> bool {
        self.0.is_read_vectored()
    }

    pub fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        self.0.read_at(buf, offset)
    }

    pub fn read_buf(&self, cursor: BorrowedCursor<'_>) -> io::Result<()> {
        self.0.read_buf(cursor)
    }

    pub fn write(&self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    pub fn write_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.0.write_vectored(bufs)
    }

    #[inline]
    pub fn is_write_vectored(&self) -> bool {
        self.0.is_write_vectored()
    }

    pub fn write_at(&self, buf: &[u8], offset: u64) -> io::Result<usize> {
        self.0.write_at(buf, offset)
    }

    #[inline]
    pub fn flush(&self) -> io::Result<()> {
        Ok(())
    }

    pub fn seek(&self, pos: SeekFrom) -> io::Result<u64> {
        let (whence, off) = match pos {
            SeekFrom::Start(n) => (syscall::EXOFS_SEEK_SET, n as i64),
            SeekFrom::End(n) => (syscall::EXOFS_SEEK_END, n),
            SeekFrom::Current(n) => (syscall::EXOFS_SEEK_CUR, n),
        };
        syscall::exofs_lseek(self.as_raw_fd(), off, whence).map_err(map_exo_err)
    }

    pub fn tell(&self) -> io::Result<u64> {
        self.seek(SeekFrom::Current(0))
    }

    pub fn duplicate(&self) -> io::Result<File> {
        Ok(Self(self.0.duplicate()?))
    }

    pub fn set_permissions(&self, perm: FilePermissions) -> io::Result<()> {
        syscall::exofs_fchmod(self.as_raw_fd(), perm.mode).map_err(map_exo_err)
    }

    pub fn set_times(&self, _times: FileTimes) -> io::Result<()> {
        Ok(())
    }

    pub fn lock(&self) -> io::Result<()> {
        unsupported()
    }

    pub fn lock_shared(&self) -> io::Result<()> {
        unsupported()
    }

    pub fn try_lock(&self) -> Result<(), TryLockError> {
        Err(TryLockError::Error(io::Error::from(io::ErrorKind::Unsupported)))
    }

    pub fn try_lock_shared(&self) -> Result<(), TryLockError> {
        Err(TryLockError::Error(io::Error::from(io::ErrorKind::Unsupported)))
    }

    pub fn unlock(&self) -> io::Result<()> {
        unsupported()
    }

    pub fn size(&self) -> Option<io::Result<u64>> {
        Some(self.file_attr().map(|a| a.size()))
    }
}

#[derive(Debug)]
pub struct DirBuilder {
    mode: u32,
}

impl DirBuilder {
    pub fn new() -> DirBuilder {
        DirBuilder { mode: 0o777 }
    }

    pub fn mkdir(&self, p: &Path) -> io::Result<()> {
        let cpath = cstr_bytes(p.as_os_str().as_bytes())?;
        syscall::exofs_mkdir(&cpath, self.mode).map_err(map_exo_err)
    }

    pub fn set_mode(&mut self, mode: u32) {
        self.mode = mode;
    }
}

pub fn unlink(p: &Path) -> io::Result<()> {
    let cpath = cstr_bytes(p.as_os_str().as_bytes())?;
    syscall::exofs_unlink(&cpath, false).map_err(map_exo_err)
}

pub fn rename(old: &Path, new: &Path) -> io::Result<()> {
    let cold = cstr_bytes(old.as_os_str().as_bytes())?;
    let cnew = cstr_bytes(new.as_os_str().as_bytes())?;
    syscall::exofs_rename(&cold, &cnew).map_err(map_exo_err)
}

pub fn rmdir(p: &Path) -> io::Result<()> {
    let cpath = cstr_bytes(p.as_os_str().as_bytes())?;
    syscall::exofs_unlink(&cpath, true).map_err(map_exo_err)
}

pub fn set_perm(p: &Path, perm: FilePermissions) -> io::Result<()> {
    let cpath = cstr_bytes(p.as_os_str().as_bytes())?;
    syscall::exofs_chmod(&cpath, perm.mode).map_err(map_exo_err)
}

pub fn set_times(_p: &Path, _times: FileTimes) -> io::Result<()> {
    Ok(())
}

pub fn set_times_nofollow(_p: &Path, _times: FileTimes) -> io::Result<()> {
    Ok(())
}

pub fn readlink(p: &Path) -> io::Result<PathBuf> {
    let cpath = cstr_bytes(p.as_os_str().as_bytes())?;
    let mut buf = [0u8; 512];
    let n = syscall::exofs_readlink(&cpath, &mut buf).map_err(map_exo_err)?;
    Ok(PathBuf::from(OsString::from_vec(buf[..n].to_vec())))
}

pub fn symlink(original: &Path, link: &Path) -> io::Result<()> {
    let corig = cstr_bytes(original.as_os_str().as_bytes())?;
    let clink = cstr_bytes(link.as_os_str().as_bytes())?;
    syscall::exofs_symlink(&corig, &clink).map_err(map_exo_err)
}

pub fn link(src: &Path, dst: &Path) -> io::Result<()> {
    let csrc = cstr_bytes(src.as_os_str().as_bytes())?;
    let cdst = cstr_bytes(dst.as_os_str().as_bytes())?;
    syscall::exofs_link(&csrc, &cdst).map_err(map_exo_err)
}

pub fn stat(p: &Path) -> io::Result<FileAttr> {
    let cpath = cstr_bytes(p.as_os_str().as_bytes())?;
    let st = syscall::exofs_stat(&cpath).map_err(map_exo_err)?;
    Ok(FileAttr { stat: st })
}

pub fn lstat(p: &Path) -> io::Result<FileAttr> {
    stat(p)
}

pub fn canonicalize(p: &Path) -> io::Result<PathBuf> {
    if p.is_absolute() {
        Ok(p.to_path_buf())
    } else {
        let mut cwd = crate::sys::pal::os::getcwd()?;
        cwd.push(p);
        Ok(cwd)
    }
}

#[derive(Debug)]
pub struct ReadDir {
    entries: Vec<DirEntry>,
    index: usize,
}

pub fn readdir(p: &Path) -> io::Result<ReadDir> {
    let mut opts = OpenOptions::new();
    opts.read(true);
    let file = File::open(p, &opts)?;
    let mut entries = Vec::new();
    let mut buf = [0u8; 4096];

    loop {
        let bytes_read = match syscall::getdents64(file.as_raw_fd(), &mut buf) {
            Ok(n) => n,
            Err(_) => match syscall::exofs_readdir_raw(file.as_raw_fd(), &mut buf) {
                Ok(n) => n,
                Err(e) => return Err(map_exo_err(e)),
            },
        };

        if bytes_read == 0 {
            break;
        }

        let mut offset = 0usize;
        while offset < bytes_read {
            if offset + 19 > bytes_read {
                break;
            }
            let ino = u64::from_ne_bytes(buf[offset..offset + 8].try_into().unwrap());
            let reclen = u16::from_ne_bytes(buf[offset + 16..offset + 18].try_into().unwrap()) as usize;
            if reclen < 19 || offset + reclen > bytes_read {
                break;
            }
            let d_type = buf[offset + 18];
            let name_bytes = &buf[offset + 19..offset + reclen];
            let name_len = name_bytes.iter().position(|&b| b == 0).unwrap_or(name_bytes.len());
            let name = &name_bytes[..name_len];

            if !name.is_empty() && name != b"." && name != b".." {
                let file_name = OsString::from_vec(name.to_vec());
                let mut full_path = p.to_path_buf();
                full_path.push(&file_name);
                let mode = if d_type == 4 {
                    syscall::EXOFS_MODE_DIR
                } else {
                    syscall::EXOFS_MODE_FILE
                };
                entries.push(DirEntry {
                    path: full_path,
                    file_name,
                    ino,
                    mode,
                });
            }
            offset += reclen;
        }
    }
    Ok(ReadDir { entries, index: 0 })
}

impl Iterator for ReadDir {
    type Item = io::Result<DirEntry>;

    fn next(&mut self) -> Option<io::Result<DirEntry>> {
        if self.index < self.entries.len() {
            let item = self.entries[self.index].clone();
            self.index += 1;
            Some(Ok(item))
        } else {
            None
        }
    }
}

#[derive(Clone, Debug)]
pub struct DirEntry {
    path: PathBuf,
    file_name: OsString,
    ino: u64,
    mode: u32,
}

impl DirEntry {
    pub fn path(&self) -> PathBuf {
        self.path.clone()
    }

    pub fn file_name(&self) -> OsString {
        self.file_name.clone()
    }

    pub fn metadata(&self) -> io::Result<FileAttr> {
        stat(&self.path)
    }

    pub fn file_type(&self) -> io::Result<FileType> {
        Ok(FileType { mode: self.mode })
    }

    pub fn ino(&self) -> u64 {
        self.ino
    }
}

impl AsInner<FileDesc> for File {
    #[inline]
    fn as_inner(&self) -> &FileDesc {
        &self.0
    }
}

impl AsInnerMut<FileDesc> for File {
    #[inline]
    fn as_inner_mut(&mut self) -> &mut FileDesc {
        &mut self.0
    }
}

impl IntoInner<FileDesc> for File {
    fn into_inner(self) -> FileDesc {
        self.0
    }
}

impl FromInner<FileDesc> for File {
    fn from_inner(file_desc: FileDesc) -> Self {
        Self(file_desc)
    }
}

impl AsFd for File {
    #[inline]
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl AsRawFd for File {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

impl IntoRawFd for File {
    fn into_raw_fd(self) -> RawFd {
        self.0.into_raw_fd()
    }
}

impl FromRawFd for File {
    unsafe fn from_raw_fd(raw_fd: RawFd) -> Self {
        unsafe { Self(FromRawFd::from_raw_fd(raw_fd)) }
    }
}
