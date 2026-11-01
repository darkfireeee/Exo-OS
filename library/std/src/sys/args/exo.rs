#![allow(dead_code)]

pub use super::common::Args;
use crate::ffi::CStr;
use crate::os::exo::ffi::OsStringExt;
use crate::ptr;
use crate::sync::atomic::{AtomicIsize, AtomicPtr, Ordering};
use crate::vec::Vec;

static ARGC: AtomicIsize = AtomicIsize::new(0);
static ARGV: AtomicPtr<*const u8> = AtomicPtr::new(ptr::null_mut());

pub unsafe fn init(argc: isize, argv: *const *const u8) {
    ARGC.store(argc, Ordering::Relaxed);
    ARGV.store(argv as *mut _, Ordering::Relaxed);
}

pub fn args() -> Args {
    let argv = ARGV.load(Ordering::Relaxed);
    let argc = if argv.is_null() { 0 } else { ARGC.load(Ordering::Relaxed) };

    let mut vec = Vec::with_capacity(argc.max(0) as usize);
    for i in 0..argc {
        let ptr = unsafe { argv.offset(i).read() };
        if ptr.is_null() {
            break;
        }
        let cstr = unsafe { CStr::from_ptr(ptr.cast()) };
        vec.push(OsStringExt::from_vec(cstr.to_bytes().to_vec()));
    }
    Args::new(vec)
}
