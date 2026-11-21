use crate::ffi::OsString;
use crate::io;
use crate::os::exo::ffi::OsStringExt;
use crate::sys::pal::map_exo_err;
use crate::sys::pal::syscall::{self, SYS_UNAME};

pub fn hostname() -> io::Result<OsString> {
    if let Ok(raw) = crate::fs::read_to_string("/etc/hostname") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return Ok(OsString::from(trimmed));
        }
    }
    // Linux x86_64 `struct utsname`: 6 fields of 65 bytes each; `nodename` is field #1 (offset 65..130).
    let mut uts = [0u8; 65 * 6];
    let ret = unsafe { syscall::syscall1(SYS_UNAME, uts.as_mut_ptr() as u64) };
    syscall::decode_ret(ret).map_err(map_exo_err)?;
    let nodename = &uts[65..130];
    let len = nodename.iter().position(|&b| b == 0).unwrap_or(nodename.len());
    Ok(OsString::from_vec(nodename[..len].to_vec()))
}
