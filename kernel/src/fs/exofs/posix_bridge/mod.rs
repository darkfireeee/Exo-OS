//! mod.rs — Pont POSIX pour ExoFS (posix_bridge)
//!
//! Agrège les quatre sous-modules du pont POSIX :
//!   - `fcntl_lock`     : verrous de plage d'octets POSIX (F_SETLK / F_GETLK)
//!   - `inode_emulation`: mapping bidirectionnel object_id ↔ ino_t
//!   - `mmap`           : gestion des mappings mémoire (mmap/munmap/msync/mprotect)
//!   - `vfs_compat`     : couche VFS (lookup/open/read/write/getattr/readdir/…)
//!
//! Fournit également `posix_bridge_init()`, `posix_bridge_shutdown()` et
//! `posix_bridge_stats()` pour initialisation, arrêt et diagnostic.
//!
//! RECUR-01 / OOM-02 / ARITH-02 — ExofsError exclusivement.

pub mod fcntl_lock;
pub mod inode_emulation;
pub mod mmap;
pub mod vfs_compat;

// ─────────────────────────────────────────────────────────────────────────────
// Ré-exports ergonomiques
// ─────────────────────────────────────────────────────────────────────────────

pub use fcntl_lock::{
    FCNTL_LOCK_TABLE,
    LockKind,
    FcntlCmd,
    ByteRangeLock,
    LockInfo,
    MAX_LOCKS_PER_OBJECT,
    MAX_OBJECTS_LOCKED,
};

pub use inode_emulation::{
    INODE_EMULATION,
    ObjectIno,
    InodeEntry,
    INO_ROOT,
    INO_RESERVED,
    INO_MAX_CACHE,
    encode_inode_entry,
    decode_inode_entry,
    inode_flags,
};

pub use mmap::{
    MMAP_TABLE,
    MmapEntry,
    MappingState,
    map_flags,
    map_prot,
    MMAP_PAGE_SIZE,
    MMAP_MAX_MAPPINGS,
    align_up,
    pages_for,
    ranges_overlap,
    validate_page_aligned,
};

pub use vfs_compat::{
    VfsInode,
    VfsDirent,
    VfsFd,
    VFS_ROOT_INO,
    VFS_NAME_MAX,
    file_mode,
    open_flags,
    register_exofs_vfs_ops,
    vfs_is_registered,
    root_inode,
    vfs_lookup,
    vfs_create,
    vfs_open,
    vfs_close,
    vfs_read,
    vfs_write,
    vfs_getattr,
    vfs_mkdir,
    vfs_unlink,
    vfs_rmdir,
    vfs_rename,
    vfs_readdir,
    vfs_truncate,
    vfs_symlink,
    vfs_close_all_pid,
    vfs_open_count,
};

use crate::fs::exofs::core::ExofsResult;

// ─────────────────────────────────────────────────────────────────────────────
// État global du pont
// ─────────────────────────────────────────────────────────────────────────────

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static BRIDGE_INITIALIZED: AtomicBool = AtomicBool::new(false);
static BRIDGE_INIT_COUNT:  AtomicU64  = AtomicU64::new(0);
static BRIDGE_SHUTDOWN_COUNT: AtomicU64 = AtomicU64::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// Statistiques
// ─────────────────────────────────────────────────────────────────────────────

/// Snapshot des métriques du pont POSIX.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct PosixBridgeStats {
    /// Nombre de verrous actifs (toutes tables confondues).
    pub lock_count:         usize,
    /// Nombre d'objets verrouillés distincts.
    pub locked_objects:     usize,
    /// Nombre d'inodes en cache.
    pub inode_count:        usize,
    /// Prochain ino qui sera alloué.
    pub next_ino:           ObjectIno,
    /// Nombre de mappings mémoire actifs.
    pub mmap_count:         usize,
    /// Nombre d'octets totaux mappés.
    pub mmap_total_bytes:   u64,
    /// Nombre de descripteurs de fichiers ouverts.
    pub open_fd_count:      usize,
    /// Le VFS ExoFS est-il enregistré ?
    pub vfs_registered:     bool,
    /// Le pont a-t-il été initialisé ?
    pub initialized:        bool,
    /// Nombre d'initialisations depuis le démarrage.
    pub init_count:         u64,
    /// Nombre d'arrêts depuis le démarrage.
    pub shutdown_count:     u64,
}

/// Collecte les statistiques de tous les sous-modules.
pub fn posix_bridge_stats() -> PosixBridgeStats {
    PosixBridgeStats {
        lock_count:       FCNTL_LOCK_TABLE.total_lock_count(),
        locked_objects:   FCNTL_LOCK_TABLE.locked_object_count(),
        inode_count:      INODE_EMULATION.count(),
        next_ino:         INODE_EMULATION.peek_next_ino(),
        mmap_count:       MMAP_TABLE.mapping_count(),
        mmap_total_bytes: MMAP_TABLE.total_mapped_bytes(),
        open_fd_count:    vfs_open_count(),
        vfs_registered:   vfs_is_registered(),
        initialized:      BRIDGE_INITIALIZED.load(Ordering::Acquire),
        init_count:       BRIDGE_INIT_COUNT.load(Ordering::Relaxed),
        shutdown_count:   BRIDGE_SHUTDOWN_COUNT.load(Ordering::Relaxed),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Cycle de vie
// ─────────────────────────────────────────────────────────────────────────────

/// Initialise l'ensemble du pont POSIX.
///
/// - Garantit la présence de l'inode racine.
/// - Enregistre ExoFS comme opérateur VFS (idem no-op si déjà fait).
/// - Idempotent : plusieurs appels sont sans danger.
pub fn posix_bridge_init() -> ExofsResult<()> {
    if BRIDGE_INITIALIZED.compare_exchange(false, true, Ordering::Release, Ordering::Relaxed).is_err() {
        // Déjà initialisé — incrément du compteur mais pas d'erreur.
        BRIDGE_INIT_COUNT.fetch_add(1, Ordering::Relaxed);
        return Ok(());
    }
    // S'assure que l'inode racine est présent.
    INODE_EMULATION.ensure_root()?;
    // Enregistre les ops VFS.
    let _ = register_exofs_vfs_ops(); // Peut retourner ObjectAlreadyExists si déjà enregistré — on ignore.
    BRIDGE_INIT_COUNT.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// Arrête le pont POSIX : libère toutes les ressources.
///
/// À appeler lors du démontage du système de fichiers.
pub fn posix_bridge_shutdown() {
    // Libère les tables dans l'ordre inverse de leur initialisation.
    MMAP_TABLE.clear();
    FCNTL_LOCK_TABLE.clear();
    INODE_EMULATION.clear();
    BRIDGE_INITIALIZED.store(false, Ordering::Release);
    BRIDGE_SHUTDOWN_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Libère toutes les ressources d'un processus (appel à exit/kill).
///
/// - Supprime tous les mappings du pid.
/// - Supprime tous les verrous du pid.
/// - Ferme tous les fds du pid.
pub fn posix_bridge_process_exit(pid: u32) {
    MMAP_TABLE.munmap_all_pid(pid);
    FCNTL_LOCK_TABLE.release_all_pid(pid as u64);
    vfs_close_all_pid(pid);
}

// ─────────────────────────────────────────────────────────────────────────────
// Validations transversales
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie la cohérence interne du pont (assertion de sanité).
/// Retourne le nombre d'anomalies détectées.
/// RECUR-01 : while.
pub fn posix_bridge_check() -> usize {
    let mut anomalies = 0usize;
    // Vérifie que la racine est enregistrée si le pont est initialisé.
    if BRIDGE_INITIALIZED.load(Ordering::Acquire) {
        if !INODE_EMULATION.contains_ino(INO_ROOT) { anomalies = anomalies.wrapping_add(1); }
        if !vfs_is_registered() { anomalies = anomalies.wrapping_add(1); }
    }
    anomalies
}

/// Retourne le nombre total de verrous actifs dans la table globale.
/// Parcours RECUR-01 : while.
pub fn total_lock_count() -> usize {
    FCNTL_LOCK_TABLE.total_lock_count()
}

// ─────────────────────────────────────────────────────────────────────────────
// Constantes de version
// ─────────────────────────────────────────────────────────────────────────────

pub const POSIX_BRIDGE_MAJOR: u8  = 1;
pub const POSIX_BRIDGE_MINOR: u8  = 0;
pub const POSIX_BRIDGE_PATCH: u8  = 0;
pub const POSIX_BRIDGE_MAGIC: u32 = 0x5042_5247; // "PBRG"

/// Retourne la version du pont sous forme d'u32 encodé.
/// ARITH-02 : wrapping_shl.
pub fn posix_bridge_version() -> u32 {
    ((POSIX_BRIDGE_MAJOR as u32).wrapping_shl(16))
        | ((POSIX_BRIDGE_MINOR as u32).wrapping_shl(8))
        | (POSIX_BRIDGE_PATCH as u32)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bridge_stats_type() {
        let s = PosixBridgeStats::default();
        assert_eq!(s.lock_count, 0);
        assert!(!s.initialized);
    }

    #[test]
    fn test_bridge_version() {
        let v = posix_bridge_version();
        assert_eq!(v >> 16, POSIX_BRIDGE_MAJOR as u32);
        assert_eq!((v >> 8) & 0xFF, POSIX_BRIDGE_MINOR as u32);
        assert_eq!(v & 0xFF, POSIX_BRIDGE_PATCH as u32);
    }

    #[test]
    fn test_bridge_magic() {
        assert_eq!(POSIX_BRIDGE_MAGIC, 0x5042_5247);
    }

    #[test]
    fn test_bridge_init_idempotent() {
        let b = AtomicBool::new(false);
        // Vérifie que compare_exchange retourne Err si déjà true.
        b.store(true, Ordering::Relaxed);
        assert!(b.compare_exchange(false, true, Ordering::Release, Ordering::Relaxed).is_err());
    }

    #[test]
    fn test_align_up_reexport() {
        // align_up est ré-exporté depuis mmap.
        assert_eq!(align_up(1, 4096), 4096);
    }

    #[test]
    fn test_pages_for_reexport() {
        assert_eq!(pages_for(4097), 2);
    }

    #[test]
    fn test_ranges_overlap_reexport() {
        assert!(ranges_overlap(0, 10, 5, 10));
        assert!(!ranges_overlap(0, 5, 5, 5));
    }

    #[test]
    fn test_vfs_root_ino() {
        assert_eq!(root_inode(), 1);
        assert_eq!(VFS_ROOT_INO, 1);
    }

    #[test]
    fn test_ino_root_constant() {
        assert_eq!(INO_ROOT, 1);
        assert_eq!(INO_RESERVED, 2);
    }

    #[test]
    fn test_mmap_page_size() {
        assert_eq!(MMAP_PAGE_SIZE, 4096);
    }

    #[test]
    fn test_validate_page_aligned_reexport() {
        assert!(validate_page_aligned(0).is_ok());
        assert!(validate_page_aligned(1).is_err());
    }

    #[test]
    fn test_inode_flags_values() {
        assert_eq!(inode_flags::DIRECTORY, 0x0001);
        assert_eq!(inode_flags::SYMLINK,   0x0002);
        assert_eq!(inode_flags::REGULAR,   0x0004);
    }

    #[test]
    fn test_file_mode_ifdir() {
        assert_eq!(file_mode::S_IFDIR, 0o040000);
    }

    #[test]
    fn test_open_flags_rdonly() {
        assert_eq!(open_flags::O_RDONLY, 0);
        assert_ne!(open_flags::O_WRONLY, 0);
    }

    #[test]
    fn test_posix_bridge_stats_size() {
        // Sanity : la structure doit être de taille raisonnable.
        let sz = core::mem::size_of::<PosixBridgeStats>();
        assert!(sz > 0 && sz < 256);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Configuration du pont
// ─────────────────────────────────────────────────────────────────────────────

/// Paramètres de configuration du pont POSIX.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PosixBridgeConfig {
    /// Nombre maximal de verrous par objet (défaut: MAX_LOCKS_PER_OBJECT).
    pub max_locks_per_object: usize,
    /// Nombre maximal d'objets verrouillés simultanément.
    pub max_locked_objects: usize,
    /// Taille maximale du cache inode.
    pub max_inode_cache: usize,
    /// Nombre maximal de mappings mémoire actifs.
    pub max_mmap_entries: usize,
    /// Nombre maximal de fds ouverts.
    pub max_open_fds: usize,
    /// Taille de page (doit être puissance de 2).
    pub page_size: u64,
    /// Version de configuration.
    pub version: u8,
}

impl Default for PosixBridgeConfig {
    fn default() -> Self {
        Self {
            max_locks_per_object: MAX_LOCKS_PER_OBJECT,
            max_locked_objects:   MAX_OBJECTS_LOCKED,
            max_inode_cache:      INO_MAX_CACHE,
            max_mmap_entries:     MMAP_MAX_MAPPINGS,
            max_open_fds:         vfs_compat::VFS_OPEN_MAX,
            page_size:            MMAP_PAGE_SIZE,
            version:              POSIX_BRIDGE_MAJOR,
        }
    }
}

impl PosixBridgeConfig {
    /// Valide la configuration. Retourne le nombre d'erreurs.
    /// RECUR-01 : pas de boucle ici (contrôles indépendants).
    pub fn validate(&self) -> usize {
        let mut errors = 0usize;
        if self.max_locks_per_object == 0   { errors = errors.wrapping_add(1); }
        if self.max_locked_objects == 0     { errors = errors.wrapping_add(1); }
        if self.max_inode_cache == 0        { errors = errors.wrapping_add(1); }
        if self.max_mmap_entries == 0       { errors = errors.wrapping_add(1); }
        if self.max_open_fds == 0           { errors = errors.wrapping_add(1); }
        if !self.page_size.is_power_of_two() { errors = errors.wrapping_add(1); }
        errors
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Diagnostics formatés
// ─────────────────────────────────────────────────────────────────────────────

/// Rapport de diagnostic compact (serialisable en zone log).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct PosixBridgeDiag {
    pub magic:         u32,
    pub version:       u32,
    pub stats:         PosixBridgeStats,
    pub config_errors: usize,
    pub check_fail:    usize,
}

/// Construit un rapport de diagnostic complet.
pub fn posix_bridge_diag() -> PosixBridgeDiag {
    let cfg = PosixBridgeConfig::default();
    PosixBridgeDiag {
        magic:         POSIX_BRIDGE_MAGIC,
        version:       posix_bridge_version(),
        stats:         posix_bridge_stats(),
        config_errors: cfg.validate(),
        check_fail:    posix_bridge_check(),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Utilitaires d'encodage/décodage de noms
// ─────────────────────────────────────────────────────────────────────────────

/// Copie `name` dans un tableau de taille fixe N. Retourne la longueur copiée.
/// OOM-02 n/a (tableau statique). ARITH-02 : min().
pub fn copy_name_to_buf<const N: usize>(name: &[u8], out: &mut [u8; N]) -> usize {
    let len = name.len().min(N).min(VFS_NAME_MAX);
    let mut i = 0usize;
    while i < len { out[i] = name[i]; i = i.wrapping_add(1); }
    len
}

/// Compare deux noms de façon constante-time (évite les oracles temporels).
/// ARITH-02 : wrapping additions.
pub fn names_equal(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() { return false; }
    let mut diff = 0u8;
    let mut i = 0usize;
    while i < a.len() { diff |= a[i] ^ b[i]; i = i.wrapping_add(1); }
    diff == 0
}

/// Retourne la longueur d'un nom nul-terminé dans un tableau de taille fixe.
/// RECUR-01 : while.
pub fn strnlen(buf: &[u8], max: usize) -> usize {
    let limit = max.min(buf.len());
    let mut i = 0usize;
    while i < limit && buf[i] != 0 { i = i.wrapping_add(1); }
    i
}

// ─────────────────────────────────────────────────────────────────────────────
// Opérations atomiques utilitaires
// ─────────────────────────────────────────────────────────────────────────────

/// Incrémente un compteur atomique avec saturation à `max`.
/// ARITH-02 : saturating_add.
pub fn atomic_saturating_inc(counter: &core::sync::atomic::AtomicU64, max: u64) {
    let mut cur = counter.load(Ordering::Relaxed);
    while cur < max {
        match counter.compare_exchange_weak(cur, cur.saturating_add(1), Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(actual) => cur = actual,
        }
    }
}

/// Décrémente un compteur atomique avec plancher à 0.
/// ARITH-02 : saturating_sub.
pub fn atomic_saturating_dec(counter: &core::sync::atomic::AtomicU64) {
    let mut cur = counter.load(Ordering::Relaxed);
    while cur > 0 {
        match counter.compare_exchange_weak(cur, cur.saturating_sub(1), Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(actual) => cur = actual,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Fonctions de conversion mode/flags
// ─────────────────────────────────────────────────────────────────────────────

/// Convertit un mode POSIX en flags inode ExoFS.
pub fn mode_to_inode_flags(mode: u32) -> u32 {
    let ty = mode & file_mode::S_IFMT;
    if ty == file_mode::S_IFDIR  { return inode_flags::DIRECTORY; }
    if ty == file_mode::S_IFLNK  { return inode_flags::SYMLINK;   }
    inode_flags::REGULAR
}

/// Convertit des flags inode ExoFS en bits de type mode POSIX.
pub fn inode_flags_to_mode_type(flags: u32) -> u32 {
    if flags & inode_flags::DIRECTORY != 0 { return file_mode::S_IFDIR; }
    if flags & inode_flags::SYMLINK   != 0 { return file_mode::S_IFLNK; }
    file_mode::S_IFREG
}

/// Retourne vrai si les open_flags nécessitent un accès en écriture.
pub fn flags_require_write(flags: u32) -> bool {
    flags & open_flags::O_WRONLY != 0 || flags & open_flags::O_RDWR != 0 || flags & open_flags::O_TRUNC != 0
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests supplémentaires
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests_extra {
    use super::*;

    #[test]
    fn test_config_default_valid() {
        let cfg = PosixBridgeConfig::default();
        assert_eq!(cfg.validate(), 0);
    }

    #[test]
    fn test_config_invalid_page_size() {
        let mut cfg = PosixBridgeConfig::default();
        cfg.page_size = 3000; // Pas une puissance de 2.
        assert!(cfg.validate() > 0);
    }

    #[test]
    fn test_config_zero_locks() {
        let mut cfg = PosixBridgeConfig::default();
        cfg.max_locks_per_object = 0;
        assert!(cfg.validate() > 0);
    }

    #[test]
    fn test_diag_magic() {
        let d = posix_bridge_diag();
        assert_eq!(d.magic, POSIX_BRIDGE_MAGIC);
    }

    #[test]
    fn test_copy_name_to_buf() {
        let src = b"hello";
        let mut buf = [0u8; 16];
        let len = copy_name_to_buf(src, &mut buf);
        assert_eq!(len, 5);
        assert_eq!(&buf[..5], b"hello");
    }

    #[test]
    fn test_copy_name_truncation() {
        let src = b"toolongname";
        let mut buf = [0u8; 4];
        let len = copy_name_to_buf(src, &mut buf);
        assert_eq!(len, 4);
    }

    #[test]
    fn test_names_equal() {
        assert!(names_equal(b"abc", b"abc"));
        assert!(!names_equal(b"abc", b"abd"));
        assert!(!names_equal(b"ab", b"abc"));
    }

    #[test]
    fn test_strnlen() {
        let buf = b"hello\x00world";
        assert_eq!(strnlen(buf, 64), 5);
        assert_eq!(strnlen(b"nul", 64), 3);
        assert_eq!(strnlen(b"", 64), 0);
    }

    #[test]
    fn test_mode_to_inode_flags() {
        assert_eq!(mode_to_inode_flags(file_mode::S_IFDIR), inode_flags::DIRECTORY);
        assert_eq!(mode_to_inode_flags(file_mode::S_IFLNK), inode_flags::SYMLINK);
        assert_eq!(mode_to_inode_flags(file_mode::S_IFREG), inode_flags::REGULAR);
    }

    #[test]
    fn test_inode_flags_to_mode_type() {
        assert_eq!(inode_flags_to_mode_type(inode_flags::DIRECTORY), file_mode::S_IFDIR);
        assert_eq!(inode_flags_to_mode_type(inode_flags::SYMLINK),   file_mode::S_IFLNK);
        assert_eq!(inode_flags_to_mode_type(inode_flags::REGULAR),   file_mode::S_IFREG);
    }

    #[test]
    fn test_flags_require_write() {
        assert!( flags_require_write(open_flags::O_WRONLY));
        assert!( flags_require_write(open_flags::O_RDWR));
        assert!( flags_require_write(open_flags::O_TRUNC));
        assert!(!flags_require_write(open_flags::O_RDONLY));
    }

    #[test]
    fn test_atomic_saturating_inc() {
        let c = core::sync::atomic::AtomicU64::new(u64::MAX - 1);
        atomic_saturating_inc(&c, u64::MAX);
        assert_eq!(c.load(Ordering::Relaxed), u64::MAX);
        atomic_saturating_inc(&c, u64::MAX); // ne dépasse pas
        assert_eq!(c.load(Ordering::Relaxed), u64::MAX);
    }

    #[test]
    fn test_atomic_saturating_dec() {
        let c = core::sync::atomic::AtomicU64::new(1);
        atomic_saturating_dec(&c);
        assert_eq!(c.load(Ordering::Relaxed), 0);
        atomic_saturating_dec(&c); // ne passe pas sous 0
        assert_eq!(c.load(Ordering::Relaxed), 0);
    }
}
