//! e9_log.rs — Sortie de diagnostic ultra-rapide sur le port QEMU 0xE9 (#![no_std])
//!
//! Permet une télémétrie à latence quasi-nulle sans allocation ni verrou,
//! indispensable pour les traces critiques en Ring 0 / Ring 1 et la détection d'anomalies.

/// Écrit un octet brut directement sur le port de debug QEMU 0xE9
#[inline(always)]
pub fn write_byte_e9(byte: u8) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") 0xE9u16,
            in("al") byte,
            options(nomem, nostack, preserves_flags)
        );
    }
    #[cfg(not(target_arch = "x86_64"))]
    let _ = byte;
}

/// Écrit une chaîne de caractères sur le port 0xE9
#[inline]
pub fn write_str_e9(s: &str) {
    write_bytes_e9(s.as_bytes());
}

/// Écrit une tranche d'octets sur le port 0xE9
#[inline]
pub fn write_bytes_e9(bytes: &[u8]) {
    for &b in bytes {
        write_byte_e9(b);
    }
}
