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
    CLONE_SIGHAND, CLONE_SYSVSEM, CLONE_THREAD, CLONE_VM, PR_SET_NAME, RawTimespec, SYS_EXIT,
    SYS_MUNMAP,
};
use crate::thread::ThreadInit;
use crate::time::Duration;

pub const DEFAULT_MIN_STACK_SIZE: usize = 256 * 1024;

const STATE_ATTACHED: u32 = 0;
const STATE_DETACHED: u32 = 1;
const STATE_EXITED_ATTACHED: u32 = 2;

#[repr(C)]
struct ThreadShared {
    ctid: AtomicU32,
    state: AtomicU32,
    stack_base: usize,
    stack_alloc_size: usize,
    tls_alloc_base: usize,
    tls_alloc_len: usize,
}

struct ThreadStartCtx {
    init: Box<ThreadInit>,
    shared: *mut ThreadShared,
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

#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
unsafe extern "C" fn unmap_tls_stack_and_exit(
    _tls_base: usize,
    _tls_len: usize,
    _stack_base: usize,
    _stack_len: usize,
) -> ! {
    core::arch::naked_asm!(
        "mov r12, rdx",
        "mov r13, rcx",
        "test rdi, rdi",
        "jz 2f",
        "test rsi, rsi",
        "jz 2f",
        "mov eax, {sys_munmap}",
        "syscall",
        "2:",
        "test r12, r12",
        "jz 3f",
        "test r13, r13",
        "jz 3f",
        "mov rdi, r12",
        "mov rsi, r13",
        "mov eax, {sys_munmap}",
        "syscall",
        "3:",
        "xor edi, edi",
        "mov eax, {sys_exit}",
        "syscall",
        "ud2",
        sys_munmap = const SYS_MUNMAP,
        sys_exit = const SYS_EXIT,
    );
}

#[cfg(not(target_arch = "x86_64"))]
unsafe extern "C" fn unmap_tls_stack_and_exit(
    tls_base: usize,
    tls_len: usize,
    _stack_base: usize,
    _stack_len: usize,
) -> ! {
    unsafe {
        runtime::free_thread_tls(tls_base as *mut u8, tls_len);
    }
    syscall::exit_thread(0)
}

unsafe extern "C" fn thread_start_inner(arg: usize) -> ! {
    runtime::sync_thread_tcb_tid();
    let ctx = unsafe { Box::from_raw(arg as *mut ThreadStartCtx) };
    let shared_ptr = ctx.shared;
    let rust_start = ctx.init.init();
    rust_start();
    unsafe {
        crate::sys::thread_local::destructors::run();
    }
    crate::rt::thread_cleanup();
    unsafe {
        runtime::free_current_thread_dyn_tls();
    }

    let shared = unsafe { &*shared_ptr };
    match shared.state.compare_exchange(
        STATE_ATTACHED,
        STATE_EXITED_ATTACHED,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => syscall::exit_thread(0),
        Err(_) => {
            let stack_base = shared.stack_base;
            let stack_len = shared.stack_alloc_size;
            let tls_base = shared.tls_alloc_base;
            let tls_len = shared.tls_alloc_len;
            // Disarm clear_child_tid before dropping `shared_ptr` so kernel exit never
            // writes `0` or futex-wakes freed heap memory.
            let _ = syscall::set_tid_address(0);
            unsafe {
                drop(Box::from_raw(shared_ptr));
                unmap_tls_stack_and_exit(tls_base, tls_len, stack_base, stack_len);
            }
        }
    }
}

unsafe fn wait_and_free_shared(shared_ptr: *mut ThreadShared) {
    if shared_ptr.is_null() {
        return;
    }
    let shared = unsafe { &*shared_ptr };
    loop {
        let cur = shared.ctid.load(Ordering::Acquire);
        if cur == 0 {
            break;
        }
        let _ = syscall::futex_wait_raw(&shared.ctid as *const AtomicU32 as *const u32, cur, None);
    }
    unsafe {
        runtime::free_thread_tls(shared.tls_alloc_base as *mut u8, shared.tls_alloc_len);
        if shared.stack_base != 0 && shared.stack_alloc_size != 0 {
            let _ = sys_munmap(shared.stack_base, shared.stack_alloc_size);
        }
        drop(Box::from_raw(shared_ptr));
    }
}

pub struct Thread {
    shared: *mut ThreadShared,
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

        let shared = Box::into_raw(Box::new(ThreadShared {
            ctid: AtomicU32::new(1),
            state: AtomicU32::new(STATE_ATTACHED),
            stack_base,
            stack_alloc_size: total_alloc_size,
            tls_alloc_base: tls_alloc_base as usize,
            tls_alloc_len,
        }));
        let start_ctx = Box::into_raw(Box::new(ThreadStartCtx { init, shared }));

        let stack_top = (stack_base + total_alloc_size) & !15usize;
        let child_stack_frame = stack_top - 16;
        unsafe {
            (child_stack_frame as *mut u64).write(start_ctx as usize as u64);
            ((child_stack_frame + 8) as *mut u64)
                .write(thread_start_trampoline as *const () as usize as u64);
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

        let ctid_ptr = unsafe { &mut (*shared).ctid as *mut AtomicU32 as *mut u32 };
        let res = unsafe {
            syscall::clone_thread_raw(
                clone_flags,
                child_stack_frame,
                ctid_ptr,
                ctid_ptr,
                tls_fs_base as usize,
            )
        };

        match res {
            Ok(_) => Ok(Thread { shared }),
            Err(err) => {
                unsafe {
                    drop(Box::from_raw(start_ctx));
                    drop(Box::from_raw(shared));
                    runtime::free_thread_tls(tls_alloc_base, tls_alloc_len);
                    let _ = sys_munmap(stack_base, total_alloc_size);
                }
                Err(io::Error::from_raw_os_error(err.errno()))
            }
        }
    }

    pub fn join(mut self) {
        let shared_ptr = self.shared;
        self.shared = null_mut();
        unsafe {
            wait_and_free_shared(shared_ptr);
        }
    }
}

impl Drop for Thread {
    fn drop(&mut self) {
        let shared_ptr = self.shared;
        if shared_ptr.is_null() {
            return;
        }
        self.shared = null_mut();
        let shared = unsafe { &*shared_ptr };
        if shared
            .state
            .compare_exchange(
                STATE_ATTACHED,
                STATE_DETACHED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            unsafe {
                wait_and_free_shared(shared_ptr);
            }
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
