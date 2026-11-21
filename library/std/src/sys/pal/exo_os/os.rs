//! OS-level path and working-directory helpers (`std::sys::pal::exo_os::os`).

use crate::error::Error as StdError;
use crate::ffi::{OsStr, OsString};
use crate::os::exo::ffi::{OsStrExt, OsStringExt};
use crate::path::{self, PathBuf};
use crate::vec::Vec;
use crate::{fmt, io};
use super::{cstr_bytes, map_exo_err, syscall};

pub fn getcwd() -> io::Result<PathBuf> {
    for cap in [512usize, 1024, 4096] {
        let mut buf = crate::vec![0u8; cap];
        match syscall::getcwd(&mut buf) {
            Ok(n) if n > 0 => {
                let end = buf[..n].iter().position(|&b| b == 0).unwrap_or(n);
                buf.truncate(end);
                return Ok(PathBuf::from(OsString::from_vec(buf)));
            }
            Ok(_) => return Err(io::Error::from_raw_os_error(syscall::SysError::ENOENT.errno())),
            Err(e) if e == syscall::SysError::ERANGE && cap < 4096 => continue,
            Err(e) => return Err(map_exo_err(e)),
        }
    }
    Err(map_exo_err(syscall::SysError::ERANGE))
}

pub fn chdir(path: &path::Path) -> io::Result<()> {
    let cpath = cstr_bytes(path.as_os_str().as_bytes())?;
    syscall::chdir(&cpath).map_err(map_exo_err)
}

pub struct SplitPaths<'a> {
    iter: core::slice::Split<'a, u8, fn(&u8) -> bool>,
}

pub fn split_paths(unparsed: &OsStr) -> SplitPaths<'_> {
    fn is_colon(b: &u8) -> bool {
        *b == b':'
    }
    SplitPaths {
        iter: unparsed.as_bytes().split(is_colon as fn(&u8) -> bool),
    }
}

impl<'a> Iterator for SplitPaths<'a> {
    type Item = PathBuf;
    fn next(&mut self) -> Option<PathBuf> {
        self.iter.next().map(|s| PathBuf::from(OsStr::from_bytes(s)))
    }
}

#[derive(Debug)]
pub struct JoinPathsError;

pub fn join_paths<I, T>(paths: I) -> Result<OsString, JoinPathsError>
where
    I: Iterator<Item = T>,
    T: AsRef<OsStr>,
{
    let mut joined = Vec::new();
    for (i, path) in paths.enumerate() {
        let path = path.as_ref().as_bytes();
        if path.contains(&b':') {
            return Err(JoinPathsError);
        }
        if i > 0 {
            joined.push(b':');
        }
        joined.extend_from_slice(path);
    }
    Ok(OsString::from_vec(joined))
}

impl fmt::Display for JoinPathsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        "path segment contains separator `:`".fmt(f)
    }
}

impl StdError for JoinPathsError {}

pub fn current_exe() -> io::Result<PathBuf> {
    let execfn = super::runtime::at_execfn_ptr();
    if !execfn.is_null() {
        let mut len = 0usize;
        while unsafe { *execfn.add(len) } != 0 {
            len += 1;
        }
        if len > 0 {
            let slice = unsafe { core::slice::from_raw_parts(execfn, len) };
            return Ok(PathBuf::from(OsStr::from_bytes(slice)));
        }
    }
    let (argc, argv, _) = super::runtime::saved_args();
    if argc > 0 && !argv.is_null() {
        let p0 = unsafe { *argv };
        if !p0.is_null() {
            let mut len = 0usize;
            while unsafe { *p0.add(len) } != 0 {
                len += 1;
            }
            if len > 0 {
                let slice = unsafe { core::slice::from_raw_parts(p0, len) };
                return Ok(PathBuf::from(OsStr::from_bytes(slice)));
            }
        }
    }
    Err(io::Error::from(io::ErrorKind::NotFound))
}

pub fn temp_dir() -> PathBuf {
    PathBuf::from("/tmp")
}

pub fn home_dir() -> Option<PathBuf> {
    if let Some(home) = crate::sys::env::getenv(OsStr::from_bytes(b"HOME")) {
        if !home.is_empty() {
            return Some(PathBuf::from(home));
        }
    }
    Some(PathBuf::from("/"))
}
