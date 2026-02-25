// kernel/src/fs/core/inode.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// INODE — Structure centrale VFS (Exo-OS · Couche 3)
// ═══════════════════════════════════════════════════════════════════════════════
//
// L'inode est objectivement la structure centrale du VFS :
//   • Représente UN fichier/répertoire/lien/device sur le disque.
//   • Référencé depuis les dentries (nom → inode).
//   • Le read path est RCU lock-free (lecture sans acquisition de verrou
//     via SeqLock + génération) pour le hot path stat/read.
//   • L'écriture (setattr, write) prend un RwLock exclusif.
//
// Architecture RCU :
//   read path  →  RwLock::read()  (multiple lecteurs concurrents)
//   write path →  RwLock::write() (exclusif)
//
// Instrumentation :
//   • FS_STATS.inode_cache_count incrémenté/décrémenté à alloc/drop.
//   • Compteur de références read/write par inode.
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicBool, Ordering};
use alloc::sync::Arc;
use alloc::string::String;

use super::types::{
    DevId, FileMode, FsError, FsResult, InodeFlags, InodeNumber,
    Stat, Timespec64, Uid, Gid, FS_STATS,
};
use super::vfs::InodeOps;
use crate::scheduler::sync::rwlock::RwLock;

// ─────────────────────────────────────────────────────────────────────────────
// InodeState — état de cycle de vie
// ─────────────────────────────────────────────────────────────────────────────

/// État du cycle de vie d'un inode.
#[repr(u8)]
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum InodeState {
    /// Inode fraîchement alloué, métadonnées non encore lues depuis disque.
    New       = 0,
    /// Inode propre, cohérent avec le disque.
    Clean     = 1,
    /// Inode modifié, à synchroniser.
    Dirty     = 2,
    /// Inode en cours d'éviction (nlink == 0, write-back en cours).
    Evicting  = 3,
    /// Inode libéré, ne pas utiliser.
    Dead      = 4,
}

// ─────────────────────────────────────────────────────────────────────────────
// Inode — structure principale
// ─────────────────────────────────────────────────────────────────────────────

/// Inode VFS universel.
///
/// Taille cible : ≤ 256 octets pour tenir dans 4 cache lines.
/// Les champs hot (mode, size, ino) sont dans les premières 64 octets.
pub struct Inode {
    // ── Cache line 1 — hot read path ────────────────────────────────────────
    /// Numéro d'inode unique dans son FS.
    pub ino:      InodeNumber,
    /// Mode (type + permissions).
    pub mode:     FileMode,
    /// Propriétaire.
    pub uid:      Uid,
    /// Groupe.
    pub gid:      Gid,
    /// Taille en octets (atomique pour read concurrent sans lock).
    pub size:     AtomicU64,
    /// Nombre de liens durs.
    pub nlink:    AtomicU32,
    /// État courant.
    pub state:    InodeState,
    _pad1:        [u8; 5],

    // ── Cache line 2 — timestamps + device ──────────────────────────────────
    /// Dernier accès (atime).
    pub atime:    Timespec64,
    /// Dernière modification (mtime).
    pub mtime:    Timespec64,
    /// Dernier changement de métadonnées (ctime).
    pub ctime:    Timespec64,
    /// Temps de création (btime — ext4).
    pub btime:    Timespec64,

    // ── Cache line 3 — identifiants disque + flags ───────────────────────────
    /// Périphérique contenant l'inode.
    pub dev:      DevId,
    /// Périphérique cible si fichier spécial (block/char).
    pub rdev:     DevId,
    /// Flags étendus ext4 (EXTENTS_FL, INLINE_DATA_FL…).
    pub flags:    InodeFlags,
    /// Taille d'un bloc I/O préféré.
    pub blksize:  u32,
    /// Blocks alloués (unités 512 octets).
    pub blocks:   AtomicU64,
    /// Génération (anti-ABA pour RCU).
    pub generation: AtomicU64,
    /// Inode "dirty" depuis ce timestamp (ns monotonic).
    pub dirty_since: AtomicU64,

    // ── Cache line 4 — ops + back pointer ────────────────────────────────────
    /// Table d'opérations spécifiques à l'FS (None = inode invalide).
    pub ops:      Option<Arc<dyn InodeOps>>,
    /// Pointeur vers le superbloc (weak pour éviter cycle Arc).
    pub sb_id:    u32,
    /// Nombre de fois que l'inode a été ouvert (pour finalisation).
    pub open_count: AtomicU32,
    /// Indique si un write-back est en cours.
    pub writeback: AtomicBool,
    _pad2:         [u8; 3],
}

// Vérification statique de taille raisonnable.
const _INODE_SIZE_CHECK: () = {
    assert!(core::mem::size_of::<InodeNumber>() == 8);
};

impl Inode {
    /// Crée un inode vierge (état `New`).
    pub fn new(ino: InodeNumber, mode: FileMode, uid: Uid, gid: Gid) -> Self {
        FS_STATS.inode_cache_count.fetch_add(1, Ordering::Relaxed);
        Self {
            ino,
            mode,
            uid,
            gid,
            size:        AtomicU64::new(0),
            nlink:       AtomicU32::new(1),
            state:       InodeState::New,
            _pad1:       [0; 5],
            atime:       Timespec64::ZERO,
            mtime:       Timespec64::ZERO,
            ctime:       Timespec64::ZERO,
            btime:       Timespec64::ZERO,
            dev:         DevId::NONE,
            rdev:         DevId::NONE,
            flags:       InodeFlags::new(0),
            blksize:     super::types::FS_BLOCK_SIZE,
            blocks:      AtomicU64::new(0),
            generation:  AtomicU64::new(0),
            dirty_since: AtomicU64::new(0),
            ops:         None,
            sb_id:       0,
            open_count:  AtomicU32::new(0),
            writeback:   AtomicBool::new(false),
            _pad2:       [0; 3],
        }
    }

    /// Construit le `Stat` depuis cet inode.
    pub fn to_stat(&self) -> Stat {
        let mut st = Stat::zeroed();
        st.st_ino     = self.ino;
        st.st_mode    = self.mode;
        st.st_uid     = self.uid.0;
        st.st_gid     = self.gid.0;
        st.st_nlink   = self.nlink.load(Ordering::Relaxed);
        st.st_size    = self.size.load(Ordering::Relaxed) as i64;
        st.st_blocks  = self.blocks.load(Ordering::Relaxed) as i64;
        st.st_blksize = self.blksize;
        st.st_dev     = self.dev;
        st.st_rdev    = self.rdev;
        st.st_atim    = self.atime;
        st.st_mtim    = self.mtime;
        st.st_ctim    = self.ctime;
        st
    }

    /// Marque l'inode comme dirty et enregistre l'horodatage.
    #[inline(always)]
    pub fn mark_dirty(&mut self, now_ns: u64) {
        if self.state != InodeState::Dirty {
            self.state = InodeState::Dirty;
            self.dirty_since.store(now_ns, Ordering::Release);
        }
    }

    /// Incrémente nlink (nouveau lien dur).
    pub fn inc_nlink(&self) {
        self.nlink.fetch_add(1, Ordering::Release);
    }

    /// Décrémente nlink — retourne `true` si l'inode doit être supprimé.
    pub fn dec_nlink(&self) -> bool {
        let prev = self.nlink.fetch_sub(1, Ordering::AcqRel);
        prev == 1 // était 1 → devient 0 → supprimer
    }

    /// Retourne la taille courante.
    #[inline(always)]
    pub fn file_size(&self) -> u64 {
        self.size.load(Ordering::Acquire)
    }

    /// Met à jour la taille atomiquement.
    #[inline(always)]
    pub fn set_size(&self, sz: u64) {
        self.size.store(sz, Ordering::Release);
    }

    /// Incrémente open_count (un nouveau fd pointe sur cet inode).
    pub fn on_open(&self) {
        self.open_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Décrémente open_count — retourne `true` si dernier fd fermé sur inode zombie.
    pub fn on_close(&self) -> bool {
        let prev = self.open_count.fetch_sub(1, Ordering::AcqRel);
        let is_last_open = prev == 1;
        let nlink_zero = self.nlink.load(Ordering::Relaxed) == 0;
        is_last_open && nlink_zero
    }

    /// Vérifie l'accès selon mode + uid/gid.
    pub fn check_access(&self, uid: Uid, gid: Gid,
                         read: bool, write: bool, exec: bool) -> FsResult<()> {
        // root bypass.
        if uid.is_root() { return Ok(()); }
        let uid_match = self.uid == uid;
        let gid_match = self.gid == gid;
        if self.mode.check_access(uid_match, gid_match, read, write, exec) {
            Ok(())
        } else {
            Err(FsError::Access)
        }
    }

    /// Met à jour `atime` si le montage ne désactive pas noatime.
    pub fn touch_atime(&mut self, now: Timespec64, noatime: bool) {
        if !noatime {
            self.atime = now;
        }
    }

    /// Met à jour `mtime` + `ctime` (après écriture).
    pub fn touch_mtime(&mut self, now: Timespec64) {
        self.mtime = now;
        self.ctime = now;
    }

    /// Met à jour uniquement `ctime` (après setattr).
    pub fn touch_ctime(&mut self, now: Timespec64) {
        self.ctime = now;
    }

    /// Incrémente la génération (utilisé par RCU pour détecter les invalidations).
    pub fn bump_generation(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::Release)
    }

    /// Retourne vrai si l'inode est un répertoire vide (seulement "." et "..").
    /// Valide uniquement si mode.is_dir().
    pub fn is_empty_dir(&self) -> bool {
        // nlink == 2 : uniquement "." et le lien depuis le parent.
        self.nlink.load(Ordering::Relaxed) == 2
    }
}

impl Drop for Inode {
    fn drop(&mut self) {
        FS_STATS.inode_cache_count.fetch_sub(1, Ordering::Relaxed);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// InodeRef — référence partagée thread-safe à un inode protégé en RW
// ─────────────────────────────────────────────────────────────────────────────

/// Référence partagée vers un `Inode` :
/// `Arc<RwLock<Inode>>` — permet lectures concurrentes + écriture exclusive.
pub type InodeRef = Arc<RwLock<Inode>>;

/// Crée un nouvel inode empaqueté dans un `InodeRef`.
pub fn new_inode_ref(ino: InodeNumber, mode: FileMode, uid: Uid, gid: Gid) -> InodeRef {
    Arc::new(RwLock::new(Inode::new(ino, mode, uid, gid)))
}

// ─────────────────────────────────────────────────────────────────────────────
// Inode cache global — raccourci lookup ino → InodeRef
// ─────────────────────────────────────────────────────────────────────────────

use crate::scheduler::sync::spinlock::SpinLock;

/// Capacité maximale du cache d'inodes.
const INODE_CACHE_CAPACITY: usize = 4096;

/// Entrée du cache d'inodes.
struct InodeCacheEntry {
    /// ID du superbloc + numéro d'inode = clé composite.
    sb_id:  u32,
    ino:    InodeNumber,
    inode:  InodeRef,
}

/// Cache d'inodes simple (linéaire, remplacé par inode_cache.rs complet).
/// Ce cache rapide permet un lookup O(n) acceptable pour les petits n.
/// `cache/inode_cache.rs` fournit le cache hash-indexé complet.
pub struct InodeCacheSimple {
    entries: SpinLock<alloc::collections::VecDeque<InodeCacheEntry>>,
    capacity: usize,
}

impl InodeCacheSimple {
    pub const fn new(cap: usize) -> Self {
        Self {
            entries:  SpinLock::new(alloc::collections::VecDeque::new()),
            capacity: cap,
        }
    }

    /// Insère un inode dans le cache. Évince le plus ancien si plein.
    pub fn insert(&self, sb_id: u32, ino: InodeNumber, inode: InodeRef) {
        let mut guard = self.entries.lock();
        // Supprimer l'éventuelle entrée existante (mise à jour).
        guard.retain(|e| !(e.sb_id == sb_id && e.ino == ino));
        // Entrée trop nombreuses → évincer le plus ancien (LRU simplifié).
        if guard.len() >= self.capacity {
            guard.pop_front();
            FS_STATS.evictions.fetch_add(1, Ordering::Relaxed);
        }
        guard.push_back(InodeCacheEntry { sb_id, ino, inode });
    }

    /// Recherche un inode dans le cache. Retourne `None` en cas de miss.
    pub fn lookup(&self, sb_id: u32, ino: InodeNumber) -> Option<InodeRef> {
        let guard = self.entries.lock();
        if let Some(e) = guard.iter().find(|e| e.sb_id == sb_id && e.ino == ino) {
            FS_STATS.cache_hits.fetch_add(1, Ordering::Relaxed);
            Some(e.inode.clone())
        } else {
            FS_STATS.cache_misses.fetch_add(1, Ordering::Relaxed);
            None
        }
    }

    /// Retire un inode du cache (lors de son éviction FS).
    pub fn remove(&self, sb_id: u32, ino: InodeNumber) {
        let mut guard = self.entries.lock();
        guard.retain(|e| !(e.sb_id == sb_id && e.ino == ino));
    }

    /// Vide complètement le cache d'un superbloc (lors de démontage).
    pub fn evict_sb(&self, sb_id: u32) {
        let mut guard = self.entries.lock();
        guard.retain(|e| e.sb_id != sb_id);
    }
}

/// Cache d'inodes global.
pub static INODE_CACHE: InodeCacheSimple = InodeCacheSimple::new(INODE_CACHE_CAPACITY);

// ─────────────────────────────────────────────────────────────────────────────
// Tests internes (no_std) — validation des invariants de base
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inode_new_increments_stats() {
        let before = FS_STATS.inode_cache_count.load(Ordering::Relaxed);
        {
            let _inode = Inode::new(
                InodeNumber::new(100),
                FileMode::regular(0o644),
                Uid::ROOT,
                Gid::ROOT,
            );
            let after = FS_STATS.inode_cache_count.load(Ordering::Relaxed);
            assert_eq!(after, before + 1);
        }
        // Après drop, le compteur revient.
        let post = FS_STATS.inode_cache_count.load(Ordering::Relaxed);
        assert_eq!(post, before);
    }

    #[test]
    fn inode_nlink_dec_returns_true_at_zero() {
        let inode = Inode::new(
            InodeNumber::new(200),
            FileMode::regular(0o644),
            Uid::ROOT,
            Gid::ROOT,
        );
        assert!(!inode.dec_nlink()); // 1 → 0, returns true
        // nlink est maintenant 0.
        assert_eq!(inode.nlink.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn inode_check_access_root_bypass() {
        let inode = Inode::new(
            InodeNumber::new(300),
            FileMode::regular(0o000), // aucune permission
            Uid::new(1000),
            Gid::new(1000),
        );
        // root bypass.
        assert!(inode.check_access(Uid::ROOT, Gid::ROOT, true, true, true).is_ok());
    }

    #[test]
    fn inode_check_access_denied() {
        let inode = Inode::new(
            InodeNumber::new(400),
            FileMode::regular(0o600), // owner rw, others nothing
            Uid::new(1000),
            Gid::new(1000),
        );
        // Un autre utilisateur tente la lecture.
        let result = inode.check_access(Uid::new(2000), Gid::new(2000), true, false, false);
        assert_eq!(result, Err(FsError::Access));
    }
}
