//! OS-level path and working-directory helpers (`std::sys::pal::exo_os::os`).

use crate::error::Error as StdError;
use crate::ffi::{OsStr, OsString};
use crate::os::exo::ffi::{OsStrExt, OsStringExt};
use crate::path::{self, PathBuf};
use crate::vec::Vec;
use crate::{fmt, io};
use super::{cstr_bytes, map_exo_err, syscall};

pub fn getcwd() -> io::Result<PathBuf> {
    let mut buf = [0u8; 512];
    match syscall::getcwd(&mut buf) {
        Ok(n) if n > 0 => {
            let end = buf[..n].iter().position(|&b| b == 0).unwrap_or(n);
            Ok(PathBuf::from(OsString::from_vec(buf[..end].to_vec())))
        }
        Ok(_) => Ok(PathBuf::from("/")),
        Err(e) => Err(map_exo_err(e)),
    }
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
    let (argc, argv, _) = super::runtime::saved_args();
    if argc > 0 && !argv.is_null() {
        let p0 = unsafe { *argv };
        if !p0.is_null() {
            let mut len = 0usize;
            while unsafe { *p0.add(len) } != 0 {
                len += 1;
            }
            let slice = unsafe { core::slice::from_raw_parts(p0, len) };
            return Ok(PathBuf::from(OsStr::from_bytes(slice)));
        }
    }
    Ok(PathBuf::from("/bin/exosh"))
}

pub fn temp_dir() -> PathBuf {
    PathBuf::from("/tmp")
}

pub fn home_dir() -> Option<PathBuf> {
    Some(PathBuf::from("/"))
}
