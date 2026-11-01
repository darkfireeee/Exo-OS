#![allow(dead_code)]

use core::slice::memchr;

pub use super::common::Env;
use crate::ffi::{CStr, OsStr, OsString};
use crate::io;
use crate::os::exo::ffi::OsStringExt;
use crate::sync::Mutex;
use crate::vec::Vec;

static ENV: Mutex<Vec<(OsString, OsString)>> = Mutex::new(Vec::new());

pub unsafe fn init_from_envp(envp: *const *const u8) {
    let mut guard = ENV.lock().unwrap();
    guard.clear();
    if envp.is_null() {
        return;
    }

    let mut cur = envp;
    unsafe {
        while !(*cur).is_null() {
            let bytes = CStr::from_ptr((*cur).cast()).to_bytes();
            if !bytes.is_empty() {
                if let Some(pos) = memchr::memchr(b'=', &bytes[1..]).map(|p| p + 1) {
                    let k = OsStringExt::from_vec(bytes[..pos].to_vec());
                    let v = OsStringExt::from_vec(bytes[pos + 1..].to_vec());
                    if let Some(existing) = guard.iter_mut().find(|(ek, _)| ek == &k) {
                        existing.1 = v;
                    } else {
                        guard.push((k, v));
                    }
                }
            }
            cur = cur.add(1);
        }
    }
}

pub fn env() -> Env {
    let guard = ENV.lock().unwrap();
    Env::new(guard.clone())
}

pub fn getenv(k: &OsStr) -> Option<OsString> {
    let guard = ENV.lock().unwrap();
    guard.iter().find(|(ek, _)| ek.as_os_str() == k).map(|(_, v)| v.clone())
}

pub unsafe fn setenv(k: &OsStr, v: &OsStr) -> io::Result<()> {
    let mut guard = ENV.lock().unwrap();
    if let Some(existing) = guard.iter_mut().find(|(ek, _)| ek.as_os_str() == k) {
        existing.1 = v.to_owned();
    } else {
        guard.push((k.to_owned(), v.to_owned()));
    }
    Ok(())
}

pub unsafe fn unsetenv(k: &OsStr) -> io::Result<()> {
    let mut guard = ENV.lock().unwrap();
    guard.retain(|(ek, _)| ek.as_os_str() != k);
    Ok(())
}
