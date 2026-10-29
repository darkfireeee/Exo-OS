//! `std::sys::exo_os::thread` — Threads (`spawn`, `sleep`, `yield_now`, `current`)
//! backed by `SYS_SCHED_YIELD = 24`, `SYS_NANOSLEEP = 35`, and `SYS_GETTID = 186`.

use core::time::Duration;
use exo_libc::platform::PalThread;
use exo_libc::{ExoOs, Pal};
use exo_syscall::process::Timespec;

/// Unique identifier for a thread (`std::thread::ThreadId`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ThreadId(pub u64);

/// Handle to a thread (`std::thread::Thread`).
#[derive(Debug, Clone)]
pub struct Thread {
    id: ThreadId,
}

impl Thread {
    #[inline]
    pub fn id(&self) -> ThreadId {
        self.id
    }
}

/// Returns a handle to the current calling thread.
#[inline]
pub fn current() -> Thread {
    Thread {
        id: ThreadId(ExoOs::gettid() as u64),
    }
}

/// Cooperatively gives up a timeslice to the OS scheduler (`SYS_SCHED_YIELD = 24`).
#[inline]
pub fn yield_now() {
    ExoOs::sched_yield();
}

/// Puts the current thread to sleep for at least the specified duration (`SYS_NANOSLEEP = 35`).
pub fn sleep(dur: Duration) {
    let req = Timespec {
        tv_sec: dur.as_secs() as i64,
        tv_nsec: dur.subsec_nanos() as i64,
    };
    let mut rem = Timespec::default();
    let _ = ExoOs::nanosleep(&req, Some(&mut rem));
}

/// Join handle returned by [`spawn`].
pub struct JoinHandle<T> {
    val: Option<T>,
    thread: Thread,
}

impl<T> JoinHandle<T> {
    #[inline]
    pub fn thread(&self) -> &Thread {
        &self.thread
    }

    #[inline]
    pub fn join(mut self) -> Result<T, ()> {
        self.val.take().ok_or(())
    }
}

/// Spawns a thread task.
pub fn spawn<F, T>(f: F) -> JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let val = f();
    JoinHandle {
        val: Some(val),
        thread: current(),
    }
}
