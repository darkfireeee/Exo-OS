//! `std::sys::exo_os::env` — Process arguments, environment variables, and working directory.

use crate::alloc_crate::string::{String, ToString};
use crate::alloc_crate::vec::Vec;
use exo_syscall::exofs::{chdir, getcwd};
use super::stdio::{Error, ErrorKind, Result};

/// Iterator over process command-line arguments (`std::env::Args`).
pub struct Args {
    items: Vec<String>,
    idx: usize,
}

impl Iterator for Args {
    type Item = String;

    fn next(&mut self) -> Option<Self::Item> {
        if self.idx < self.items.len() {
            let s = self.items[self.idx].clone();
            self.idx += 1;
            Some(s)
        } else {
            None
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let rem = self.items.len().saturating_sub(self.idx);
        (rem, Some(rem))
    }
}

impl ExactSizeIterator for Args {}

unsafe fn read_cstr(ptr: *const u8) -> String {
    if ptr.is_null() {
        return String::new();
    }
    let mut len = 0usize;
    while len < 4096 {
        let b = unsafe { *ptr.add(len) };
        if b == 0 {
            break;
        }
        len += 1;
    }
    let slice = unsafe { core::slice::from_raw_parts(ptr, len) };
    String::from_utf8_lossy(slice).into_owned()
}

/// Returns the arguments that this program was started with (parsed by `exo_rt` from `_start`).
pub fn args() -> Args {
    let (argc, argv, _) = exo_rt::saved_args();
    let mut items = Vec::new();
    if !argv.is_null() {
        for i in 0..argc {
            let p = unsafe { *argv.add(i) };
            if p.is_null() {
                break;
            }
            items.push(unsafe { read_cstr(p) });
        }
    }
    Args { items, idx: 0 }
}

/// Iterator over process environment variables (`std::env::Vars`).
pub struct Vars {
    items: Vec<(String, String)>,
    idx: usize,
}

impl Iterator for Vars {
    type Item = (String, String);

    fn next(&mut self) -> Option<Self::Item> {
        if self.idx < self.items.len() {
            let pair = self.items[self.idx].clone();
            self.idx += 1;
            Some(pair)
        } else {
            None
        }
    }
}

/// Returns an iterator of `(variable, value)` pairs of strings for the current process.
pub fn vars() -> Vars {
    let (_, _, envp) = exo_rt::saved_args();
    let mut items = Vec::new();
    if !envp.is_null() {
        let mut i = 0usize;
        while i < 512 {
            let p = unsafe { *envp.add(i) };
            if p.is_null() {
                break;
            }
            let raw = unsafe { read_cstr(p) };
            if let Some((k, v)) = raw.split_once('=') {
                items.push((k.to_string(), v.to_string()));
            }
            i += 1;
        }
    }
    Vars { items, idx: 0 }
}

/// Fetches the environment variable `key` from the current process.
pub fn var(key: &str) -> core::result::Result<String, Error> {
    for (k, v) in vars() {
        if k == key {
            return Ok(v);
        }
    }
    Err(Error::new(
        ErrorKind::NotFound,
        "environment variable not found",
    ))
}

/// Returns the current working directory (`SYS_GETCWD = 79`).
pub fn current_dir() -> Result<String> {
    let mut buf = [0u8; 512];
    let n = getcwd(&mut buf).map_err(|e| Error::from_raw_os_error(e.errno()))?;
    let end = buf[..n].iter().position(|&b| b == 0).unwrap_or(n);
    Ok(String::from_utf8_lossy(&buf[..end]).into_owned())
}

/// Changes the current working directory (`SYS_CHDIR = 80`).
pub fn set_current_dir(path: &str) -> Result<()> {
    let mut cpath = Vec::with_capacity(path.len() + 1);
    cpath.extend_from_slice(path.as_bytes());
    cpath.push(0);
    chdir(&cpath).map_err(|e| Error::from_raw_os_error(e.errno()))
}
