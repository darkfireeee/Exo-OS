use crate::sync::atomic::{AtomicU64, Ordering};
use crate::sys::pal::{runtime, syscall};

static FALLBACK_STATE: AtomicU64 = AtomicU64::new(0x9E37_79B9_7F4A_7C15);

#[inline]
fn next_u64() -> u64 {
    let mut s = FALLBACK_STATE.load(Ordering::Relaxed);
    let random_ptr = runtime::at_random_ptr();
    if !random_ptr.is_null() {
        let seed_bytes = unsafe { *random_ptr };
        let seed_lo = u64::from_le_bytes([
            seed_bytes[0],
            seed_bytes[1],
            seed_bytes[2],
            seed_bytes[3],
            seed_bytes[4],
            seed_bytes[5],
            seed_bytes[6],
            seed_bytes[7],
        ]);
        s ^= seed_lo;
    }
    #[cfg(target_arch = "x86_64")]
    {
        let lo: u32;
        let hi: u32;
        unsafe {
            core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack));
        }
        s ^= ((hi as u64) << 32) | (lo as u64);
    }
    let mut z = s.wrapping_add(0x9E37_79B9_7F4A_7C15);
    FALLBACK_STATE.store(z, Ordering::Relaxed);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

pub fn fill_bytes(bytes: &mut [u8]) {
    if bytes.is_empty() {
        return;
    }
    if let Ok(n) = syscall::getrandom(bytes, 0) {
        if n == bytes.len() {
            return;
        }
    }
    for chunk in bytes.chunks_mut(8) {
        let word = next_u64().to_le_bytes();
        chunk.copy_from_slice(&word[..chunk.len()]);
    }
}
