//! `std::sys::exo_os::fs` — File system primitives over ExoFS (`500..=520`) + POSIX VFS bridge.

use crate::alloc_crate::string::{String, ToString};
use crate::alloc_crate::vec::Vec;
use exo_libc::{ExoOs, Pal};
use exo_syscall::exofs::{ExoDirEntry, ExoFsFileStat};
use super::stdio::{Error, ErrorKind, Read, Result, Write};

/// Options and flags for opening a [`File`].
#[derive(Debug, Clone)]
pub struct OpenOptions {
    read: bool,
    write: bool,
    append: bool,
    truncate: bool,
    create: bool,
    create_new: bool,
    mode: u32,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenOptions {
    #[inline]
    pub const fn new() -> Self {
        Self {
            read: false,
            write: false,
            append: false,
            truncate: false,
            create: false,
            create_new: false,
            mode: 0o644,
        }
    }

    #[inline]
    pub fn read(&mut self, read: bool) -> &mut Self {
        self.read = read;
        self
    }

    #[inline]
    pub fn write(&mut self, write: bool) -> &mut Self {
        self.write = write;
        self
    }

    #[inline]
    pub fn append(&mut self, append: bool) -> &mut Self {
        self.append = append;
        self
    }

    #[inline]
    pub fn truncate(&mut self, truncate: bool) -> &mut Self {
        self.truncate = truncate;
        self
    }

    #[inline]
    pub fn create(&mut self, create: bool) -> &mut Self {
        self.create = create;
        self
    }

    #[inline]
    pub fn create_new(&mut self, create_new: bool) -> &mut Self {
        self.create_new = create_new;
        self
    }

    pub fn open(&self, path: &str) -> Result<File> {
        const O_RDONLY: i32 = 0;
        const O_WRONLY: i32 = 1;
        const O_RDWR: i32 = 2;
        const O_CREAT: i32 = 0o100;
        const O_EXCL: i32 = 0o200;
        const O_TRUNC: i32 = 0o1000;
        const O_APPEND: i32 = 0o2000;

        let mut flags = match (self.read, self.write || self.append) {
            (true, false) => O_RDONLY,
            (false, true) => O_WRONLY,
            (true, true) => O_RDWR,
            (false, false) => return Err(Error::new(ErrorKind::InvalidInput, "invalid open mode")),
        };
        if self.create {
            flags |= O_CREAT;
        }
        if self.create_new {
            flags |= O_CREAT | O_EXCL;
        }
        if self.truncate {
            flags |= O_TRUNC;
        }
        if self.append {
            flags |= O_APPEND;
        }

        let mut cpath = Vec::with_capacity(path.len() + 1);
        cpath.extend_from_slice(path.as_bytes());
        cpath.push(0);

        let fd = ExoOs::open(&cpath, flags, self.mode)
            .map_err(|e| Error::from_raw_os_error(e.errno()))?;
        Ok(File { fd })
    }
}

/// An open file handle on Exo-OS (`std::fs::File`).
#[derive(Debug)]
pub struct File {
    fd: i32,
}

impl File {
    pub fn open(path: &str) -> Result<Self> {
        OpenOptions::new().read(true).open(path)
    }

    pub fn create(path: &str) -> Result<Self> {
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
    }

    #[inline]
    pub const fn as_raw_fd(&self) -> i32 {
        self.fd
    }

    #[inline]
    pub const unsafe fn from_raw_fd(fd: i32) -> Self {
        Self { fd }
    }

    pub fn sync_all(&self) -> Result<()> {
        ExoOs::fsync(self.fd).map_err(|e| Error::from_raw_os_error(e.errno()))
    }

    pub fn metadata(&self) -> Result<Metadata> {
        let stat = ExoOs::fstat(self.fd).map_err(|e| Error::from_raw_os_error(e.errno()))?;
        Ok(Metadata { stat })
    }
}

impl Read for File {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        ExoOs::read(self.fd, buf).map_err(|e| Error::from_raw_os_error(e.errno()))
    }
}

impl Write for File {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        ExoOs::write(self.fd, buf).map_err(|e| Error::from_raw_os_error(e.errno()))
    }

    fn flush(&mut self) -> Result<()> {
        self.sync_all()
    }
}

impl Drop for File {
    fn drop(&mut self) {
        if self.fd >= 0 {
            let _ = ExoOs::close(self.fd);
        }
    }
}

/// File metadata (`std::fs::Metadata`) backed by `ExoFsFileStat`.
#[derive(Debug, Clone, Copy)]
pub struct Metadata {
    stat: ExoFsFileStat,
}

impl Metadata {
    #[inline]
    pub const fn len(&self) -> u64 {
        self.stat.size
    }

    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.stat.size == 0
    }

    #[inline]
    pub const fn is_dir(&self) -> bool {
        (self.stat.mode & 0o170000) == 0o040000
    }

    #[inline]
    pub const fn is_file(&self) -> bool {
        (self.stat.mode & 0o170000) == 0o100000 || !self.is_dir()
    }

    #[inline]
    pub const fn raw_stat(&self) -> &ExoFsFileStat {
        &self.stat
    }
}

/// Directory entry (`std::fs::DirEntry`).
#[derive(Debug, Clone)]
pub struct DirEntry {
    name: String,
    ino: u64,
    file_type: u8,
}

impl DirEntry {
    #[inline]
    pub fn file_name(&self) -> &str {
        &self.name
    }

    #[inline]
    pub const fn ino(&self) -> u64 {
        self.ino
    }

    #[inline]
    pub const fn is_dir(&self) -> bool {
        self.file_type == 4
    }
}

/// Iterator over entries in a directory (`std::fs::ReadDir`).
pub struct ReadDir {
    entries: Vec<DirEntry>,
    index: usize,
}

impl Iterator for ReadDir {
    type Item = Result<DirEntry>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index < self.entries.len() {
            let item = self.entries[self.index].clone();
            self.index += 1;
            Some(Ok(item))
        } else {
            None
        }
    }
}

/// Reads all entries of a directory using native `SYS_EXOFS_READDIR` (`520`).
pub fn read_dir(path: &str) -> Result<ReadDir> {
    let file = File::open(path)?;
    let mut raw_entries = [ExoDirEntry::default(); 64];
    let mut entries = Vec::new();

    if let Ok(count) = ExoOs::getdents(file.as_raw_fd(), &mut raw_entries) {
        for raw in raw_entries.iter().take(count) {
            let name = raw.name_str().to_string();
            if !name.is_empty() && name != "." && name != ".." {
                entries.push(DirEntry {
                    name,
                    ino: raw.inode,
                    file_type: raw.file_type,
                });
            }
        }
    }
    Ok(ReadDir { entries, index: 0 })
}

pub fn read(path: &str) -> Result<Vec<u8>> {
    let mut f = File::open(path)?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    Ok(buf)
}

pub fn read_to_string(path: &str) -> Result<String> {
    let mut f = File::open(path)?;
    let mut s = String::new();
    f.read_to_string(&mut s)?;
    Ok(s)
}

pub fn write(path: &str, contents: &[u8]) -> Result<()> {
    let mut f = File::create(path)?;
    f.write_all(contents)
}

pub fn metadata(path: &str) -> Result<Metadata> {
    let mut cpath = Vec::with_capacity(path.len() + 1);
    cpath.extend_from_slice(path.as_bytes());
    cpath.push(0);
    let stat = ExoOs::stat(&cpath).map_err(|e| Error::from_raw_os_error(e.errno()))?;
    Ok(Metadata { stat })
}

pub fn create_dir(path: &str) -> Result<()> {
    let mut cpath = Vec::with_capacity(path.len() + 1);
    cpath.extend_from_slice(path.as_bytes());
    cpath.push(0);
    ExoOs::mkdir(&cpath, 0o755).map_err(|e| Error::from_raw_os_error(e.errno()))
}

pub fn remove_file(path: &str) -> Result<()> {
    let mut cpath = Vec::with_capacity(path.len() + 1);
    cpath.extend_from_slice(path.as_bytes());
    cpath.push(0);
    ExoOs::unlink(&cpath, 0).map_err(|e| Error::from_raw_os_error(e.errno()))
}

pub fn remove_dir(path: &str) -> Result<()> {
    let mut cpath = Vec::with_capacity(path.len() + 1);
    cpath.extend_from_slice(path.as_bytes());
    cpath.push(0);
    ExoOs::unlink(&cpath, 1).map_err(|e| Error::from_raw_os_error(e.errno()))
}

pub fn rename(from: &str, to: &str) -> Result<()> {
    let mut cfrom = Vec::with_capacity(from.len() + 1);
    cfrom.extend_from_slice(from.as_bytes());
    cfrom.push(0);
    let mut cto = Vec::with_capacity(to.len() + 1);
    cto.extend_from_slice(to.as_bytes());
    cto.push(0);
    ExoOs::rename(&cfrom, &cto).map_err(|e| Error::from_raw_os_error(e.errno()))
}
