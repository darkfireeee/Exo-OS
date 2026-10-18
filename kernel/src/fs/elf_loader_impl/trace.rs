//! trace.rs — Primitives de traçage et débogage du chargeur ELF

#[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
use crate::fs::exofs::cache::BLOB_CACHE;
use crate::fs::exofs::core::types::BlobId;
#[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
use core::sync::atomic::{AtomicUsize, Ordering};

#[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
static ELF_TRACE_COUNT: AtomicUsize = AtomicUsize::new(0);

#[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
#[inline(always)]
pub fn debug_byte(byte: u8) {
    unsafe {
        core::arch::asm!("out 0xE9, al", in("al") byte, options(nomem, nostack));
    }
}

#[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
pub fn debug_write(bytes: &[u8]) {
    for &byte in bytes {
        debug_byte(byte);
    }
}

#[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
pub fn debug_usize(mut value: usize) {
    let mut buf = [0u8; 20];
    let mut len = 0usize;
    if value == 0 {
        debug_byte(b'0');
        return;
    }
    while value != 0 && len < buf.len() {
        buf[len] = b'0' + (value % 10) as u8;
        value /= 10;
        len += 1;
    }
    while len != 0 {
        len -= 1;
        debug_byte(buf[len]);
    }
}

#[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
pub fn debug_blob_prefix(blob_id: &BlobId) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in &blob_id.as_bytes()[..6] {
        debug_byte(HEX[(byte >> 4) as usize]);
        debug_byte(HEX[(byte & 0x0f) as usize]);
    }
}

#[inline]
pub fn trace_path(prefix: &[u8], path: &str) {
    let _ = (prefix, path);
    #[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
    {
        if ELF_TRACE_COUNT.fetch_add(1, Ordering::Relaxed) >= 96 {
            return;
        }
        debug_write(prefix);
        debug_write(path.as_bytes());
        debug_write(b"\n");
    }
}

#[inline]
pub fn trace_blob(prefix: &[u8], blob_id: &BlobId) {
    let _ = (prefix, blob_id);
    #[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
    {
        if ELF_TRACE_COUNT.fetch_add(1, Ordering::Relaxed) >= 96 {
            return;
        }
        debug_write(prefix);
        debug_blob_prefix(blob_id);
        debug_write(b" entries=");
        debug_usize(BLOB_CACHE.n_entries());
        debug_write(b"\n");
    }
}
