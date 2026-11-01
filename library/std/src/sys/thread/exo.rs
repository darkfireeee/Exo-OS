use crate::ffi::CStr;
use crate::io;
use crate::num::NonZeroUsize;
use crate::sys::pal::syscall::{self, RawTimespec};
use crate::thread::ThreadInit;
use crate::time::Duration;

pub const DEFAULT_MIN_STACK_SIZE: usize = 256 * 1024;

pub struct Thread {
    _private: (),
}

unsafe impl Send for Thread {}
unsafe impl Sync for Thread {}

impl Thread {
    pub unsafe fn new(_stack: usize, init: Box<ThreadInit>) -> io::Result<Thread> {
        let rust_start = init.init();
        rust_start();
        Ok(Thread { _private: () })
    }

    pub fn join(self) {}
}

pub fn set_name(_name: &CStr) {}

pub fn current_os_id() -> Option<u64> {
    Some(syscall::gettid() as u64)
}

pub fn available_parallelism() -> io::Result<NonZeroUsize> {
    Ok(unsafe { NonZeroUsize::new_unchecked(1) })
}

pub fn yield_now() {
    syscall::sched_yield();
}

pub fn sleep(dur: Duration) {
    let req = RawTimespec {
        tv_sec: dur.as_secs() as i64,
        tv_nsec: dur.subsec_nanos() as i64,
    };
    let mut rem = RawTimespec::default();
    let _ = syscall::nanosleep(&req, Some(&mut rem));
}
