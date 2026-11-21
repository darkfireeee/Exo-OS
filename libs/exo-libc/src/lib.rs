//! `exo-libc` — Implémentation locale de la Platform Abstraction Layer (`Pal for ExoOs`)
//! et primitives stdio / errno sans dépendance `relibc` externe (`LIB-09` / `O-10`).

#![no_std]
#![forbid(unsafe_op_in_unsafe_fn)]

pub mod platform;

pub use platform::exo_os::ExoOs;
pub use platform::{Pal, PalSignal, PalSocket, PalThread};

use core::sync::atomic::{AtomicI32, Ordering};
use exo_rt::ExoTcb;

static EARLY_ERRNO: AtomicI32 = AtomicI32::new(0);
static STDIO_FLUSH_HOOK: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Retourne le pointeur vers le `errno` du thread courant (`%fs:0x34`).
#[inline]
pub fn errno_location() -> *mut i32 {
    let ptr = unsafe { ExoTcb::errno_ptr() };
    if ptr.is_null() {
        EARLY_ERRNO.as_ptr()
    } else {
        ptr
    }
}

/// Lit le `errno` courant (par thread via TCB `%fs:0x34`).
#[inline]
pub fn errno() -> i32 {
    let ptr = unsafe { ExoTcb::errno_ptr() };
    if ptr.is_null() {
        EARLY_ERRNO.load(Ordering::Acquire)
    } else {
        unsafe { ptr.read() }
    }
}

/// Met à jour le `errno` courant (par thread via TCB `%fs:0x34`).
#[inline]
pub fn set_errno(err: i32) {
    let ptr = unsafe { ExoTcb::errno_ptr() };
    if ptr.is_null() {
        EARLY_ERRNO.store(err, Ordering::Release);
    } else {
        unsafe { ptr.write(err) };
    }
}

/// Enregistre un hook de vidage des tampons stdio utilisateur (utilisé par `std::io::stdout`).
#[inline]
pub fn register_stdio_flush_hook(hook: fn()) {
    STDIO_FLUSH_HOOK.store(hook as usize, Ordering::Release);
}

/// Vide tous les tampons stdio avant `fork()` ou `exit()` (`RÈGLE PROC-04`).
#[inline]
pub fn fflush_all() {
    let hook_addr = STDIO_FLUSH_HOOK.load(Ordering::Acquire);
    if hook_addr != 0 {
        let hook: fn() = unsafe { core::mem::transmute(hook_addr) };
        hook();
    }
    core::sync::atomic::fence(Ordering::SeqCst);
}
