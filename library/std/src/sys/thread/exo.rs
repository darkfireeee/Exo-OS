use crate::boxed::Box;
use crate::ffi::CStr;
use crate::io;
use crate::num::NonZeroUsize;
use crate::ptr::null_mut;
use crate::sync::atomic::{AtomicU32, Ordering};
use crate::sys::pal::alloc_dlmalloc::{
    MAP_ANONYMOUS, MAP_PRIVATE, PROT_NONE, PROT_READ, PROT_WRITE, align_up, sys_mmap,
    sys_mprotect, sys_munmap,
};
use crate::sys::pal::runtime;
use crate::sys::pal::syscall::{
    self, CLONE_CHILD_CLEARTID, CLONE_FILES, CLONE_FS, CLONE_PARENT_SETTID, CLONE_SETTLS,
    CLONE_SIGHAND, CLONE_SYSVSEM, CLONE_THREAD, CLONE_VM, PR_SET_NAME, RawTimespec,
};
use crate::thread::ThreadInit;
use crate::time::Duration;

pub const DEFAULT_MIN_STACK_SIZE: usize = 256 * 1024;

struct ThreadStartCtx {
    init: Box<ThreadInit>,
    tls_alloc_base: *mut u8,
    tls_alloc_len: usize,
}

#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
unsafe extern "C" fn thread_start_trampoline(_arg: usize) -> ! {
    core::arch::naked_asm!(
        "xor rbp, rbp",
        "and rsp, -16",
        "call {inner}",
        "ud2",
        inner = sym thread_start_inner,
    );
}

#[cfg(not(target_arch = "x86_64"))]
unsafe extern "C" fn thread_start_trampoline(arg: usize) -> ! {
    unsafe { thread_start_inner(arg) }
}

unsafe extern "C" fn thread_start_inner(arg: usize) -> ! {
    let ctx = unsafe { Box::from_raw(arg as *mut ThreadStartCtx) };
    let tls_base = ctx.tls_alloc_base;
    let tls_len = ctx.tls_alloc_len;
    let rust_start = ctx.init.init();
    rust_start();
    unsafe {
        runtime::free_thread_tls(tls_base, tls_len);
    }
    syscall::exit_thread(0)
}

pub struct Thread {
    ctid_box: *mut AtomicU32,
    stack_base: usize,
    stack_alloc_size: usize,
}

unsafe impl Send for Thread {}
unsafe impl Sync for Thread {}

impl Thread {
    pub unsafe fn new(stack: usize, init: Box<ThreadInit>) -> io::Result<Thread> {
        let page_sz = runtime::page_size();
        let user_stack_size = align_up(stack.max(DEFAULT_MIN_STACK_SIZE), page_sz);
        let total_alloc_size = page_sz + user_stack_size;

        let stack_base = unsafe {
            sys_mmap(
                0,
                total_alloc_size,
                PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANONYMOUS,
                !0,
                0,
            )
            .map_err(io::Error::from_raw_os_error)?
        };

        // Guard page at bottom of stack.
        let _ = unsafe { sys_mprotect(stack_base, page_sz, PROT_NONE) };

        let (tls_alloc_base, tls_alloc_len, tls_fs_base) = match unsafe { runtime::allocate_thread_tls() } {
            Ok(v) => v,
            Err(errno) => {
                let _ = unsafe { sys_munmap(stack_base, total_alloc_size) };
                return Err(io::Error::from_raw_os_error(errno));
            }
        };

        let ctid_box = Box::into_raw(Box::new(AtomicU32::new(1)));
        let start_ctx = Box::into_raw(Box::new(ThreadStartCtx {
            init,
            tls_alloc_base,
            tls_alloc_len,
        }));

        let stack_top = (stack_base + total_alloc_size) & !15usize;
        let child_stack = stack_top - 16;
        unsafe {
            (child_stack as *mut u64).write(start_ctx as usize as u64);
        }

        let clone_flags = CLONE_VM
            | CLONE_FS
            | CLONE_FILES
            | CLONE_SIGHAND
            | CLONE_THREAD
            | CLONE_SYSVSEM
            | CLONE_SETTLS
            | CLONE_PARENT_SETTID
            | CLONE_CHILD_CLEARTID;

        let ctid_ptr = ctid_box as *mut u32;
        let res = unsafe {
            syscall::clone_raw(
                clone_flags,
                child_stack,
                ctid_ptr,
                ctid_ptr,
                tls_fs_base as usize,
                thread_start_trampoline as *const () as usize as u64,
            )
        };

        match res {
            Ok(_) => Ok(Thread {
                ctid_box,
                stack_base,
                stack_alloc_size: total_alloc_size,
            }),
            Err(err) => {
                unsafe {
                    drop(Box::from_raw(start_ctx));
                    drop(Box::from_raw(ctid_box));
                    runtime::free_thread_tls(tls_alloc_base, tls_alloc_len);
                    let _ = sys_munmap(stack_base, total_alloc_size);
                }
                Err(io::Error::from_raw_os_error(err.errno()))
            }
        }
    }

    pub fn join(mut self) {
        if !self.ctid_box.is_null() {
            loop {
                let cur = unsafe { (*self.ctid_box).load(Ordering::Acquire) };
                if cur == 0 {
                    break;
                }
                let _ = syscall::futex_wait_raw(self.ctid_box as *const u32, cur, None);
            }
            unsafe {
                drop(Box::from_raw(self.ctid_box));
                if self.stack_base != 0 && self.stack_alloc_size != 0 {
                    let _ = sys_munmap(self.stack_base, self.stack_alloc_size);
                }
            }
            self.ctid_box = null_mut();
            self.stack_base = 0;
            self.stack_alloc_size = 0;
        }
    }
}

pub fn set_name(name: &CStr) {
    let _ = syscall::prctl(PR_SET_NAME, name.as_ptr() as usize as u64, 0, 0, 0);
}

pub fn current_os_id() -> Option<u64> {
    Some(syscall::gettid() as u64)
}

pub fn available_parallelism() -> io::Result<NonZeroUsize> {
    let mut mask = [0u8; 32];
    if let Ok(len) = syscall::sched_getaffinity(0, &mut mask) {
        let n_bytes = len.min(mask.len());
        let mut count = 0usize;
        for &b in &mask[..n_bytes] {
            count += b.count_ones() as usize;
        }
        if let Some(nz) = NonZeroUsize::new(count) {
            return Ok(nz);
        }
    }
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
