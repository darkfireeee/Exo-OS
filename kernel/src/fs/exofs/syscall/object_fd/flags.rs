//! # fs/exofs/syscall/object_fd/flags.rs — Drapeaux d'ouverture de descripteur d'objet
//!
//! Définit les constantes POSIX-compatibles pour `open` (O_RDONLY, O_WRONLY, O_RDWR, etc.)
//! et les méthodes d'analyse et de validation.

pub const O_RDONLY: u32 = 0x0000;
pub const O_WRONLY: u32 = 0x0001;
pub const O_RDWR: u32 = 0x0002;
pub const O_CREAT: u32 = 0x0040;
pub const O_EXCL: u32 = 0x0080;
pub const O_TRUNC: u32 = 0x0200;
pub const O_APPEND: u32 = 0x0400;

/// Retourne true si les flags autorisent la lecture.
#[inline]
pub fn can_read(flags: u32) -> bool {
    let rw = flags & 0x0003;
    rw == O_RDONLY || rw == O_RDWR
}

/// Retourne true si les flags autorisent l'écriture.
#[inline]
pub fn can_write(flags: u32) -> bool {
    let rw = flags & 0x0003;
    rw == O_WRONLY || rw == O_RDWR
}

/// Valide les flags d'ouverture (seuls les bits connus sont acceptés).
#[inline]
pub fn validate(flags: u32) -> bool {
    flags & !0x07FF == 0
}
