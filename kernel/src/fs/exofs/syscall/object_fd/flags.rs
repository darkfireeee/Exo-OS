//! # fs/exofs/syscall/object_fd/flags.rs — Drapeaux d'ouverture de descripteur d'objet
//!
//! Définit les constantes POSIX-compatibles pour `open` (O_RDONLY, O_WRONLY, O_RDWR, etc.)
//! et les méthodes d'analyse et de validation.

pub const O_RDONLY: u32 = 0x0000;
pub const O_WRONLY: u32 = 0x0001;
pub const O_RDWR: u32 = 0x0002;
pub const O_CREAT: u32 = 0x0040;
pub const O_EXCL: u32 = 0x0080;
pub const O_NOCTTY: u32 = 0x0100;
pub const O_TRUNC: u32 = 0x0200;
pub const O_APPEND: u32 = 0x0400;
pub const O_NONBLOCK: u32 = 0x0800;
pub const O_DSYNC: u32 = 0x1000;
pub const O_DIRECT: u32 = 0x4000;
pub const O_LARGEFILE: u32 = 0x8000;
pub const O_DIRECTORY: u32 = 0x0001_0000;
pub const O_NOFOLLOW: u32 = 0x0002_0000;
pub const O_NOATIME: u32 = 0x0004_0000;
pub const O_CLOEXEC: u32 = 0x0008_0000;
pub const O_SYNC: u32 = 0x0010_1000;
pub const O_PATH: u32 = 0x0020_0000;
pub const O_TMPFILE: u32 = 0x0040_0000;

/// Masque des drapeaux valides reconnus par ExoFS (aligné sur Linux / POSIX)
pub const VALID_FLAGS: u32 = 0x007F_FFFF;

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
    flags & !VALID_FLAGS == 0
}
