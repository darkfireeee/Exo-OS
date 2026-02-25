// kernel/src/fs/core/types.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// TYPES FONDAMENTAUX FS — Exo-OS · Couche 3
// ═══════════════════════════════════════════════════════════════════════════════
//
// FileMode, Permissions, Stat, Dirent, InodeNumber, DevId…
// Zéro dépendance vers d'autres modules kernel sauf memory::core::types.
// Compatibilité POSIX 2024 garantie (syscall/ABI stables).
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::fmt;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes fondamentales
// ─────────────────────────────────────────────────────────────────────────────

/// Taille d'un bloc FS par défaut (4 KiB = page size).
pub const FS_BLOCK_SIZE: u32 = 4096;
/// Masque d'alignement bloc.
pub const FS_BLOCK_MASK: u32 = FS_BLOCK_SIZE - 1;
/// Décalage bloc (log2(4096) = 12).
pub const FS_BLOCK_SHIFT: u32 = 12;
/// Numéro d'inode "vide" / invalide.
pub const INVALID_INO: u64 = 0;
/// Numéro d'inode racine du système de fichiers.
pub const ROOT_INO: u64 = 2;
/// Longueur maximale d'un nom de fichier (POSIX).
pub const NAME_MAX: usize = 255;
/// Longueur maximale d'un chemin absolu (POSIX).
pub const PATH_MAX: usize = 4096;
/// Nombre maximum de liens symboliques à résoudre (anti-boucle).
pub const MAXSYMLINKS: u32 = 40;
/// Nombre max de descripteurs de fichier par processus.
pub const OPEN_MAX: usize = 1024;
/// Alignement cache-line (64 octets).
pub const CACHE_LINE: usize = 64;

// ─────────────────────────────────────────────────────────────────────────────
// InodeNumber — numéro d'inode 64 bits
// ─────────────────────────────────────────────────────────────────────────────

/// Identifiant unique d'un inode dans un système de fichiers.
/// Opaque — ne pas construire directement (passer par allocateurs dédiés).
#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct InodeNumber(pub u64);

impl Default for InodeNumber {
    #[inline(always)]
    fn default() -> Self { InodeNumber::INVALID }
}

impl InodeNumber {
    /// Inode invalide (valeur sentinelle).
    pub const INVALID: Self = InodeNumber(0);
    /// Inode racine ext4 standard.
    pub const ROOT: Self = InodeNumber(2);

    #[inline(always)]
    pub const fn new(n: u64) -> Self {
        InodeNumber(n)
    }

    #[inline(always)]
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    #[inline(always)]
    pub const fn is_valid(self) -> bool {
        self.0 != 0
    }
}

impl fmt::Display for InodeNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ino#{}", self.0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DevId — identifiant de périphérique (majeur + mineur)
// ─────────────────────────────────────────────────────────────────────────────

/// Identifiant de périphérique (dev_t POSIX).
/// Encodage : bits [63:20] = majeur, bits [19:0] = mineur.
#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct DevId(pub u64);

impl Default for DevId {
    #[inline(always)]
    fn default() -> Self { DevId::NONE }
}

impl DevId {
    pub const NONE: Self = DevId(0);

    #[inline(always)]
    pub const fn new(major: u32, minor: u32) -> Self {
        DevId(((major as u64) << 20) | (minor as u64 & 0xF_FFFF))
    }

    #[inline(always)]
    pub const fn major(self) -> u32 {
        (self.0 >> 20) as u32
    }

    #[inline(always)]
    pub const fn minor(self) -> u32 {
        (self.0 & 0xF_FFFF) as u32
    }
}

impl fmt::Display for DevId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.major(), self.minor())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// FileType — type de fichier (inode)
// ─────────────────────────────────────────────────────────────────────────────

/// Type du fichier encodé dans le champ `mode` (bits [15:12]).
#[repr(u8)]
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum FileType {
    /// Fichier régulier.
    Regular   = 0x8,
    /// Répertoire.
    Directory = 0x4,
    /// Lien symbolique.
    Symlink   = 0xA,
    /// Fichier spécial bloc (disque).
    Block     = 0x6,
    /// Fichier spécial caractère (tty, urandom…).
    Char      = 0x2,
    /// FIFO (named pipe).
    Fifo      = 0x1,
    /// Socket UNIX domain.
    Socket    = 0xC,
    /// Inconnu / non initialisé.
    Unknown   = 0x0,
}

impl FileType {
    /// Extrait le type depuis les bits haut d'un mode POSIX.
    #[inline(always)]
    pub const fn from_mode(mode: u16) -> Self {
        match (mode >> 12) & 0xF {
            0x8 => FileType::Regular,
            0x4 => FileType::Directory,
            0xA => FileType::Symlink,
            0x6 => FileType::Block,
            0x2 => FileType::Char,
            0x1 => FileType::Fifo,
            0xC => FileType::Socket,
            _   => FileType::Unknown,
        }
    }

    /// Retourne les bits de type masqués pour le champ mode.
    #[inline(always)]
    pub const fn mode_bits(self) -> u16 {
        (self as u16) << 12
    }

    #[inline(always)]
    pub const fn is_dir(self) -> bool {
        matches!(self, FileType::Directory)
    }

    #[inline(always)]
    pub const fn is_regular(self) -> bool {
        matches!(self, FileType::Regular)
    }

    #[inline(always)]
    pub const fn is_symlink(self) -> bool {
        matches!(self, FileType::Symlink)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// FileMode — mode complet (type + permissions)
// ─────────────────────────────────────────────────────────────────────────────

/// Mode POSIX complet : type (4 bits) + setuid/setgid/sticky (3 bits) +
/// permissions rwx pour user/group/other (9 bits) = 16 bits.
#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct FileMode(pub u16);

impl Default for FileMode {
    #[inline(always)]
    fn default() -> Self { FileMode(0) }
}

impl FileMode {
    // ── Constantes de permission ─────────────────────────────────────────────
    pub const S_ISUID: u16 = 0o4000; // Set-user-ID
    pub const S_ISGID: u16 = 0o2000; // Set-group-ID
    pub const S_ISVTX: u16 = 0o1000; // Sticky bit

    pub const S_IRUSR: u16 = 0o0400;
    pub const S_IWUSR: u16 = 0o0200;
    pub const S_IXUSR: u16 = 0o0100;
    pub const S_IRGRP: u16 = 0o0040;
    pub const S_IWGRP: u16 = 0o0020;
    pub const S_IXGRP: u16 = 0o0010;
    pub const S_IROTH: u16 = 0o0004;
    pub const S_IWOTH: u16 = 0o0002;
    pub const S_IXOTH: u16 = 0o0001;

    pub const PERM_MASK: u16 = 0o7777;
    pub const TYPE_MASK: u16 = 0xF000;

    // ── Constructeurs communs ────────────────────────────────────────────────
    pub const fn new(raw: u16) -> Self { FileMode(raw) }

    pub const fn regular(perm: u16) -> Self {
        FileMode(FileType::Regular.mode_bits() | (perm & Self::PERM_MASK))
    }
    pub const fn directory(perm: u16) -> Self {
        FileMode(FileType::Directory.mode_bits() | (perm & Self::PERM_MASK))
    }
    pub const fn symlink() -> Self {
        FileMode(FileType::Symlink.mode_bits() | 0o0777)
    }

    #[inline(always)]
    pub const fn file_type(self) -> FileType {
        FileType::from_mode(self.0)
    }

    #[inline(always)]
    pub const fn permissions(self) -> u16 {
        self.0 & Self::PERM_MASK
    }

    #[inline(always)]
    pub const fn is_dir(self) -> bool {
        self.file_type().is_dir()
    }

    #[inline(always)]
    pub const fn is_regular(self) -> bool {
        self.file_type().is_regular()
    }

    #[inline(always)]
    pub const fn is_symlink(self) -> bool {
        matches!(self.file_type(), FileType::Symlink)
    }

    #[inline(always)]
    pub const fn is_setuid(self) -> bool {
        (self.0 & Self::S_ISUID) != 0
    }

    #[inline(always)]
    pub const fn is_setgid(self) -> bool {
        (self.0 & Self::S_ISGID) != 0
    }

    /// Vérifie que les permissions accordent l'accès demandé.
    /// `uid_match` / `gid_match` : est-ce que l'appelant correspond à owner/group.
    pub fn check_access(self, uid_match: bool, gid_match: bool, want_read: bool, want_write: bool, want_exec: bool) -> bool {
        let perms = self.permissions();
        let shift = if uid_match { 6 } else if gid_match { 3 } else { 0 };
        let bits = (perms >> shift) & 0o7;
        let r_ok = !want_read  || (bits & 0o4 != 0);
        let w_ok = !want_write || (bits & 0o2 != 0);
        let x_ok = !want_exec  || (bits & 0o1 != 0);
        r_ok && w_ok && x_ok
    }
}

impl fmt::Debug for FileMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let p = self.permissions();
        write!(f, "{:?}({:04o})", self.file_type(), p)
    }
}

impl fmt::Display for FileMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let p = self.permissions();
        let t = match self.file_type() {
            FileType::Regular   => '-',
            FileType::Directory => 'd',
            FileType::Symlink   => 'l',
            FileType::Block     => 'b',
            FileType::Char      => 'c',
            FileType::Fifo      => 'p',
            FileType::Socket    => 's',
            FileType::Unknown   => '?',
        };
        let rwx = |v: u16, r: u16, w: u16, x: u16| {
            let rc = if v & r != 0 { 'r' } else { '-' };
            let wc = if v & w != 0 { 'w' } else { '-' };
            let xc = if v & x != 0 { 'x' } else { '-' };
            (rc, wc, xc)
        };
        let (ur, uw, ux) = rwx(p, 0o400, 0o200, 0o100);
        let (gr, gw, gx) = rwx(p, 0o040, 0o020, 0o010);
        let (or2, ow, ox) = rwx(p, 0o004, 0o002, 0o001);
        write!(f, "{}{}{}{}{}{}{}{}{}{}", t, ur, uw, ux, gr, gw, gx, or2, ow, ox)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// OpenFlags — flags open(2)
// ─────────────────────────────────────────────────────────────────────────────

/// Flags transmis à `open(2)` / `openat(2)`.
/// Compatible Linux ABI x86_64.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct OpenFlags(pub u32);

impl OpenFlags {
    pub const O_RDONLY:    u32 = 0o0000000;
    pub const O_WRONLY:    u32 = 0o0000001;
    pub const O_RDWR:      u32 = 0o0000002;
    pub const O_CREAT:     u32 = 0o0000100;
    pub const O_EXCL:      u32 = 0o0000200;
    pub const O_NOCTTY:    u32 = 0o0000400;
    pub const O_TRUNC:     u32 = 0o0001000;
    pub const O_APPEND:    u32 = 0o0002000;
    pub const O_NONBLOCK:  u32 = 0o0004000;
    pub const O_DSYNC:     u32 = 0o0010000;
    pub const O_DIRECT:    u32 = 0o0040000;
    pub const O_LARGEFILE: u32 = 0o0100000;
    pub const O_DIRECTORY: u32 = 0o0200000;
    pub const O_NOFOLLOW:  u32 = 0o0400000;
    pub const O_CLOEXEC:   u32 = 0o2000000;
    pub const O_SYNC:      u32 = 0o4010000;
    pub const O_PATH:      u32 = 0o10000000;

    pub const fn new(raw: u32) -> Self { OpenFlags(raw) }

    #[inline] pub const fn is_read(self) -> bool  { (self.0 & 0o3) != Self::O_WRONLY }
    #[inline] pub const fn is_write(self) -> bool { (self.0 & 0o3) == Self::O_WRONLY || (self.0 & 0o3) == Self::O_RDWR }
    #[inline] pub const fn is_append(self) -> bool { self.0 & Self::O_APPEND != 0 }
    #[inline] pub const fn is_nonblock(self) -> bool { self.0 & Self::O_NONBLOCK != 0 }
    #[inline] pub const fn is_direct(self) -> bool { self.0 & Self::O_DIRECT != 0 }
    #[inline] pub const fn is_cloexec(self) -> bool { self.0 & Self::O_CLOEXEC != 0 }
    #[inline] pub const fn create_on_missing(self) -> bool { self.0 & Self::O_CREAT != 0 }
    #[inline] pub const fn excl(self) -> bool { self.0 & Self::O_EXCL != 0 }
    #[inline] pub const fn truncate(self) -> bool { self.0 & Self::O_TRUNC != 0 }
    #[inline] pub const fn nofollow(self) -> bool { self.0 & Self::O_NOFOLLOW != 0 }
    #[inline] pub const fn directory_only(self) -> bool { self.0 & Self::O_DIRECTORY != 0 }
    #[inline] pub const fn sync(self) -> bool { self.0 & Self::O_SYNC != 0 }
}

// ─────────────────────────────────────────────────────────────────────────────
// SeekWhence — positionnement lseek(2)
// ─────────────────────────────────────────────────────────────────────────────

#[repr(u32)]
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum SeekWhence {
    /// Depuis le début du fichier.
    Set     = 0,
    /// Depuis la position courante.
    Current = 1,
    /// Depuis la fin du fichier.
    End     = 2,
    /// Prochain trou (SEEK_HOLE/SEEK_DATA POSIX).
    Data    = 3,
    /// Prochaine donnée.
    Hole    = 4,
}

impl SeekWhence {
    pub fn from_u32(v: u32) -> Option<Self> {
        match v {
            0 => Some(SeekWhence::Set),
            1 => Some(SeekWhence::Current),
            2 => Some(SeekWhence::End),
            3 => Some(SeekWhence::Data),
            4 => Some(SeekWhence::Hole),
            _ => None,
        }
    }

    /// Alias de `Current` (compatibilité POSIX SEEK_CUR).
    pub const Cur: SeekWhence = SeekWhence::Current;
}

// ─────────────────────────────────────────────────────────────────────────────
// Timespec64 — horodatage nanoseconde
// ─────────────────────────────────────────────────────────────────────────────

/// Horodatage POSIX — secondes + nanosecondes.
#[repr(C)]
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct Timespec64 {
    /// Secondes depuis l'epoch Unix.
    pub tv_sec:  i64,
    /// Nanosecondes (0..999_999_999).
    pub tv_nsec: i32,
    pub _pad:    i32,
}

impl Timespec64 {
    pub const ZERO: Self = Self { tv_sec: 0, tv_nsec: 0, _pad: 0 };

    #[inline(always)]
    pub const fn new(sec: i64, nsec: i32) -> Self {
        Self { tv_sec: sec, tv_nsec: nsec, _pad: 0 }
    }

    /// Retourne l'horodatage en nanosecondes absolues.
    #[inline(always)]
    pub const fn as_ns(self) -> i128 {
        self.tv_sec as i128 * 1_000_000_000 + self.tv_nsec as i128
    }

    /// Construit depuis des nanosecondes monotoniques.
    #[inline(always)]
    pub const fn from_ns(ns: u64) -> Self {
        let sec = (ns / 1_000_000_000) as i64;
        let nsec = (ns % 1_000_000_000) as i32;
        Self { tv_sec: sec, tv_nsec: nsec, _pad: 0 }
    }
}

impl fmt::Display for Timespec64 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{:09}", self.tv_sec, self.tv_nsec)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Stat — structure stat(2) POSIX
// ─────────────────────────────────────────────────────────────────────────────

/// Structure retournée par `stat(2)` / `fstat(2)` / `lstat(2)`.
/// Compatible Linux `struct stat` x86_64.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct Stat {
    /// Identifiant du périphérique contenant l'inode.
    pub st_dev:     DevId,
    /// Numéro d'inode.
    pub st_ino:     InodeNumber,
    /// Mode (type + permissions).
    pub st_mode:    FileMode,
    /// Nombre de liens durs.
    pub st_nlink:   u32,
    /// UID propriétaire.
    pub st_uid:     u32,
    /// GID groupe.
    pub st_gid:     u32,
    /// DevId si fichier spécial.
    pub st_rdev:    DevId,
    /// Taille en octets.
    pub st_size:    i64,
    /// Taille d'un bloc I/O.
    pub st_blksize: u32,
    pub _pad:       u32,
    /// Nombre de blocs alloués (en unités 512 octets).
    pub st_blocks:  i64,
    /// Dernier accès.
    pub st_atim:    Timespec64,
    /// Dernière modification.
    pub st_mtim:    Timespec64,
    /// Dernier changement de métadonnées.
    pub st_ctim:    Timespec64,
}

impl Stat {
    pub const fn zeroed() -> Self {
        Self {
            st_dev: DevId::NONE, st_ino: InodeNumber::INVALID,
            st_mode: FileMode(0), st_nlink: 0, st_uid: 0, st_gid: 0,
            st_rdev: DevId::NONE, st_size: 0, st_blksize: FS_BLOCK_SIZE, _pad: 0,
            st_blocks: 0,
            st_atim: Timespec64::ZERO, st_mtim: Timespec64::ZERO, st_ctim: Timespec64::ZERO,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DirEntry — entrée lue par getdents64(2)
// ─────────────────────────────────────────────────────────────────────────────

/// Entrée de répertoire retournée par `getdents64(2)`.
/// Taille variable — `d_reclen` indique la taille totale de l'entrée.
#[repr(C)]
#[derive(Clone, Debug)]
pub struct Dirent64 {
    /// Numéro d'inode.
    pub d_ino:    u64,
    /// Offset vers l'entrée suivante dans le répertoire.
    pub d_off:    i64,
    /// Longueur totale de cette structure (alignée 8 octets).
    pub d_reclen: u16,
    /// Type d'entrée (DT_* constants).
    pub d_type:   DirEntryType,
    pub _pad:     u8,
    /// Nom null-terminé (longueur variable).
    pub d_name:   [u8; NAME_MAX + 1],
}

/// Type d'une entrée de répertoire (`d_type` dans Dirent64).
#[repr(u8)]
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum DirEntryType {
    Unknown   = 0,
    Fifo      = 1,
    Char      = 2,
    Directory = 4,
    Block     = 6,
    Regular   = 8,
    Symlink   = 10,
    Socket    = 12,
    Whiteout  = 14,
}

impl DirEntryType {
    pub const fn from_file_type(ft: FileType) -> Self {
        match ft {
            FileType::Regular   => DirEntryType::Regular,
            FileType::Directory => DirEntryType::Directory,
            FileType::Symlink   => DirEntryType::Symlink,
            FileType::Block     => DirEntryType::Block,
            FileType::Char      => DirEntryType::Char,
            FileType::Fifo      => DirEntryType::Fifo,
            FileType::Socket    => DirEntryType::Socket,
            FileType::Unknown   => DirEntryType::Unknown,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// FsStats — statistiques système de fichiers (statfs(2))
// ─────────────────────────────────────────────────────────────────────────────

/// Structure retournée par `statfs(2)` / `fstatfs(2)`.
/// Compatible Linux `struct statfs64`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct FsStats {
    /// Magic number identifiant le FS (ext4 = 0xEF53).
    pub f_type:   u64,
    /// Taille d'un bloc FS.
    pub f_bsize:  u64,
    /// Nombre total de blocs.
    pub f_blocks: u64,
    /// Nombre de blocs libres.
    pub f_bfree:  u64,
    /// Nombre de blocs libres pour non-root.
    pub f_bavail: u64,
    /// Nombre total d'inodes.
    pub f_files:  u64,
    /// Nombre d'inodes libres.
    pub f_ffree:  u64,
    /// Identifiant du FS.
    pub f_fsid:   [u32; 2],
    /// Longueur maximale des noms.
    pub f_namelen: u64,
    /// Unité de fragmentation.
    pub f_frsize:  u64,
    /// Flags de montage.
    pub f_flags:   u64,
    pub f_spare:   [u64; 4],
}

// ─────────────────────────────────────────────────────────────────────────────
// FsError — erreurs FS (errno compatible)
// ─────────────────────────────────────────────────────────────────────────────

/// Erreurs retournées par les opérations FS.
/// Valeurs compatibles errno POSIX/Linux.
#[repr(i32)]
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum FsError {
    /// Opération réussie.
    Ok            =  0,
    /// Argument invalide.
    InvalidArg    = -22,  // EINVAL
    /// Fichier non trouvé.
    NotFound      = -2,   // ENOENT
    /// Accès refusé.
    Access        = -13,  // EACCES
    /// Permission refusée.
    Permission    = -1,   // EPERM
    /// Trop de fichiers ouverts.
    TooManyFiles  = -24,  // EMFILE
    /// Espace insuffisant.
    NoSpace       = -28,  // ENOSPC
    /// Impossible de créer (déjà existant).
    Exists        = -17,  // EEXIST
    /// N'est pas un répertoire.
    NotDir        = -20,  // ENOTDIR
    /// Est un répertoire.
    IsDir         = -21,  // EISDIR
    /// Répertoire non vide.
    DirNotEmpty   = -39,  // ENOTEMPTY
    /// Système de fichiers en lecture seule.
    ReadOnly      = -30,  // EROFS
    /// Dépassement de capacité.
    Overflow      = -75,  // EOVERFLOW
    /// Corruption détectée.
    Corrupt       = -117, // EUCLEAN (remapped)
    /// I/O error.
    Io            = -5,   // EIO
    /// Buffer trop petit.
    Range         = -34,  // ERANGE
    /// Lien symbolique en boucle.
    Loop          = -40,  // ELOOP
    /// Cross-device link.
    CrossDevice   = -18,  // EXDEV
    /// Ressource temporairement indisponible.
    WouldBlock    = -11,  // EAGAIN / EWOULDBLOCK
    /// Délai expiré.
    TimedOut      = -110, // ETIMEDOUT
    /// Interruption par signal.
    Interrupted   = -4,   // EINTR
    /// Capacité insuffisante (journalisation).
    JournalFull   = -105, // ENOBUFS (journal full — distinct de ENOSPC)
    /// Inode corrompu.
    BadInode      = -14,  // EFAULT (remapped)
    /// Fonctionnalité non implémentée.
    NotSupported  = -95,  // EOPNOTSUPP
    /// Nom trop long.
    NameTooLong   = -36,  // ENAMETOOLONG
    /// Trop de liens.
    TooManyLinks  = -31,  // EMLINK
    /// Descripteur invalide.
    BadFd         = -9,   // EBADF
    /// Offset invalide (hors fichier).
    Seek          = -29,  // ESPIPE
    /// Pipe cassé.
    BrokenPipe    = -32,  // EPIPE
    /// Handle périmé (NFS stale).
    Stale         = -116,  // ESTALE
    /// Numéro magique invalide.
    BadMagic      = -84,  // EILSEQ (remapped pour numéro magique invalide)
    /// Connexion refusée.
    ConnectionRefused = -111, // ECONNREFUSED
    /// Non connecté.
    NotConnected  = -107, // ENOTCONN
    /// Données corrompues (alias sémantique de Corrupt).
    DataCorrupted = -74,  // EBADMSG
}

impl FsError {
    /// Lit la valeur errno (absolue).
    #[inline(always)]
    pub const fn errno(self) -> i32 {
        -(self as i32)
    }

    #[inline(always)]
    pub fn to_errno(self) -> i32 { self.errno() }

    // ── Aliases sémantiques ──────────────────────────────────────────────────
    // Ces constantes pointent vers les variants canoniques.

    /// Alias : EAGAIN / EWOULDBLOCK.
    pub const Again: FsError = FsError::WouldBlock;
    /// Alias : EAGAIN non-bloquant (identique à `Again`).
    pub const TryAgain: FsError = FsError::WouldBlock;
    /// Alias : argument invalide (= `InvalidArg`).
    pub const InvalArg: FsError = FsError::InvalidArg;
    /// Alias : mauvaise adresse (= `BadInode` / EFAULT).
    pub const BadAddress: FsError = FsError::BadInode;
    /// Alias : déjà existant (= `Exists`).
    pub const AlreadyExists: FsError = FsError::Exists;
    /// Alias : argument invalide (= `InvalidArg`).
    pub const InvalidArgument: FsError = FsError::InvalidArg;
    /// Alias : est un répertoire (= `IsDir`).
    pub const IsDirectory: FsError = FsError::IsDir;
    /// Alias : permission refusée (= `Access`).
    pub const PermissionDenied: FsError = FsError::Access;
}

impl fmt::Display for FsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FsError::{:?}(errno={})", self, self.errno())
    }
}

/// Type résultat standard des opérations FS.
pub type FsResult<T> = Result<T, FsError>;

// ─────────────────────────────────────────────────────────────────────────────
// MountFlags — flags de montage
// ─────────────────────────────────────────────────────────────────────────────

/// Flags passés à `mount(2)`.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct MountFlags(pub u64);

impl MountFlags {
    pub const MS_RDONLY:      u64 = 1 << 0;
    pub const MS_NOSUID:      u64 = 1 << 1;
    pub const MS_NODEV:       u64 = 1 << 2;
    pub const MS_NOEXEC:      u64 = 1 << 3;
    pub const MS_SYNCHRONOUS: u64 = 1 << 4;
    pub const MS_REMOUNT:     u64 = 1 << 5;
    pub const MS_MANDLOCK:    u64 = 1 << 6;
    pub const MS_DIRSYNC:     u64 = 1 << 7;
    pub const MS_NOATIME:     u64 = 1 << 10;
    pub const MS_NODIRATIME:  u64 = 1 << 11;
    pub const MS_BIND:        u64 = 1 << 12;
    pub const MS_MOVE:        u64 = 1 << 13;
    pub const MS_REC:         u64 = 1 << 14;
    pub const MS_STRICTATIME: u64 = 1 << 24;
    pub const MS_LAZYTIME:    u64 = 1 << 25;

    pub const fn new(raw: u64) -> Self { MountFlags(raw) }
    pub const fn is_readonly(self) -> bool  { self.0 & Self::MS_RDONLY != 0 }
    pub const fn is_nosuid(self) -> bool    { self.0 & Self::MS_NOSUID != 0 }
    pub const fn is_noatime(self) -> bool   { self.0 & Self::MS_NOATIME != 0 }
    pub const fn is_sync(self) -> bool      { self.0 & Self::MS_SYNCHRONOUS != 0 }
}

// ─────────────────────────────────────────────────────────────────────────────
// FsGeneration — compteur de génération anti-ABA atomique
// ─────────────────────────────────────────────────────────────────────────────

/// Compteur de génération pour détecter les invalidations concurrentes.
/// Utilisé par le dentry cache et l'inode cache.
pub struct FsGeneration(AtomicU64);

impl FsGeneration {
    pub const fn new(v: u64) -> Self {
        FsGeneration(AtomicU64::new(v))
    }

    /// Lit la génération actuelle (lecture non bloquante).
    #[inline(always)]
    pub fn load(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }

    /// Incrémente la génération (invalidation).
    #[inline(always)]
    pub fn bump(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Release)
    }

    /// Vérifie si la génération correspond (valide).
    #[inline(always)]
    pub fn matches(&self, expected: u64) -> bool {
        self.0.load(Ordering::Acquire) == expected
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Uid / Gid
// ─────────────────────────────────────────────────────────────────────────────

/// Identifiant utilisateur 32 bits.
#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct Uid(pub u32);

/// Identifiant groupe 32 bits.
#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct Gid(pub u32);

impl Uid {
    pub const ROOT: Self = Uid(0);
    pub const NOBODY: Self = Uid(65534);
    pub const fn new(v: u32) -> Self { Uid(v) }
    pub const fn is_root(self) -> bool { self.0 == 0 }
}

impl Gid {
    pub const ROOT: Self = Gid(0);
    pub const NOBODY: Self = Gid(65534);
    pub const fn new(v: u32) -> Self { Gid(v) }
    pub const fn is_root(self) -> bool { self.0 == 0 }
}

// ─────────────────────────────────────────────────────────────────────────────
// InodeFlags — flags etendus d'un inode
// ─────────────────────────────────────────────────────────────────────────────

/// Flags étendus d'un inode ext4 (champ i_flags).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct InodeFlags(pub u32);

impl InodeFlags {
    pub const SECRM_FL:        u32 = 0x00000001; // Secure deletion
    pub const UNRM_FL:         u32 = 0x00000002; // Undelete
    pub const COMPR_FL:        u32 = 0x00000004; // Compress file
    pub const SYNC_FL:         u32 = 0x00000008; // Synchronous updates
    pub const IMMUTABLE_FL:    u32 = 0x00000010; // Immutable file
    pub const APPEND_FL:       u32 = 0x00000020; // writes to file may only append
    pub const NODUMP_FL:       u32 = 0x00000040; // do not dump file
    pub const NOATIME_FL:      u32 = 0x00000080; // do not update atime
    pub const DIRTY_FL:        u32 = 0x00000100;
    pub const COMPRBLK_FL:     u32 = 0x00000200;
    pub const NOCOMPR_FL:      u32 = 0x00000400;
    pub const ENCRYPT_FL:      u32 = 0x00000800; // Encrypted inode
    pub const BTREE_FL:        u32 = 0x00001000; // b-tree format dir
    pub const INDEX_FL:        u32 = 0x00001000; // hash-indexed directory
    pub const IMAGIC_FL:       u32 = 0x00002000;
    pub const JOURNAL_DATA_FL: u32 = 0x00004000; // file data should be journaled
    pub const NOTAIL_FL:       u32 = 0x00008000;
    pub const DIRSYNC_FL:      u32 = 0x00010000; // dirsync behaviour
    pub const TOPDIR_FL:       u32 = 0x00020000; // Top of directory hierarchies
    pub const HUGE_FILE_FL:    u32 = 0x00040000;
    pub const EXTENTS_FL:      u32 = 0x00080000; // Inode uses extents
    pub const VERITY_FL:       u32 = 0x00100000; // Verity protected inode
    pub const EA_INODE_FL:     u32 = 0x00200000; // Large extended attribute value
    pub const EOFBLOCKS_FL:    u32 = 0x00400000;
    pub const SNAPFILE_FL:     u32 = 0x01000000;
    pub const INLINE_DATA_FL:  u32 = 0x10000000; // Inode has inline data
    pub const PROJINHERIT_FL:  u32 = 0x20000000; // Create with parents projid
    pub const CASEFOLD_FL:     u32 = 0x40000000; // Casefolded directory

    pub const fn new(v: u32) -> Self { InodeFlags(v) }
    pub const fn has_extents(self) -> bool { self.0 & Self::EXTENTS_FL != 0 }
    pub const fn is_inline(self) -> bool   { self.0 & Self::INLINE_DATA_FL != 0 }
    pub const fn is_immutable(self) -> bool { self.0 & Self::IMMUTABLE_FL != 0 }
    pub const fn is_append_only(self) -> bool { self.0 & Self::APPEND_FL != 0 }
    pub const fn is_encrypted(self) -> bool { self.0 & Self::ENCRYPT_FL != 0 }
    pub const fn has_htree(self) -> bool { self.0 & Self::INDEX_FL != 0 }
    pub const fn journal_data(self) -> bool { self.0 & Self::JOURNAL_DATA_FL != 0 }
}

// ─────────────────────────────────────────────────────────────────────────────
// FsAtomics — compteurs atomiques globaux pour instrumentation
// ─────────────────────────────────────────────────────────────────────────────

/// Compteurs globaux d'instrumentation du sous-système FS.
pub struct FsAtomics {
    /// Nombre d'inodes en cache.
    pub inode_cache_count: AtomicU64,
    /// Nombre de dentries en cache.
    pub dentry_cache_count: AtomicU64,
    /// Nombre de pages en page cache.
    pub page_cache_count: AtomicU64,
    /// Alias : nombre de pages en page cache.
    pub page_cache_pages: AtomicU64,
    /// Lectures FS totales (en octets).
    pub reads_bytes: AtomicU64,
    /// Alias : octets lus (même compteur, nom POSIX).
    pub bytes_read: AtomicU64,
    /// Écritures FS totales (en octets).
    pub writes_bytes: AtomicU64,
    /// Alias : octets écrits (même compteur, nom POSIX).
    pub bytes_written: AtomicU64,
    /// Nombre de pages sales (non synchronisées).
    pub dirty_pages: AtomicU64,
    /// Nombre de fichiers/descripteurs actuellement ouverts.
    pub open_files: AtomicU64,
    /// Nombre de défauts de cache (page cache miss).
    pub cache_misses: AtomicU64,
    /// Nombre d'accès au cache (page cache hit).
    pub cache_hits: AtomicU64,
    /// Nombre d'opérations d'éviction.
    pub evictions: AtomicU64,
    /// Erreurs I/O.
    pub io_errors: AtomicU64,
    /// Transactions journal committées.
    pub journal_commits: AtomicU64,
    /// Transactions journal avortées.
    pub journal_aborts: AtomicU64,
}

impl FsAtomics {
    const fn zeroed() -> Self {
        Self {
            inode_cache_count:  AtomicU64::new(0),
            dentry_cache_count: AtomicU64::new(0),
            page_cache_count:   AtomicU64::new(0),
            page_cache_pages:   AtomicU64::new(0),
            reads_bytes:        AtomicU64::new(0),
            bytes_read:         AtomicU64::new(0),
            writes_bytes:       AtomicU64::new(0),
            bytes_written:      AtomicU64::new(0),
            dirty_pages:        AtomicU64::new(0),
            open_files:         AtomicU64::new(0),
            cache_misses:       AtomicU64::new(0),
            cache_hits:         AtomicU64::new(0),
            evictions:          AtomicU64::new(0),
            io_errors:          AtomicU64::new(0),
            journal_commits:    AtomicU64::new(0),
            journal_aborts:     AtomicU64::new(0),
        }
    }
}

/// Instance globale des compteurs d'instrumentation FS.
pub static FS_STATS: FsAtomics = FsAtomics::zeroed();
