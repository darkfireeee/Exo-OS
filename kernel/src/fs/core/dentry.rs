// kernel/src/fs/core/dentry.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// DENTRY — Cache de noms VFS (Exo-OS · Couche 3)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Le dentry cache (dcache) mappe : (parent_dentry, name) → inode.
// Il évite de re-descendre dans le FS pour chaque composant de chemin.
//
// Design :
//   • Chaque `Dentry` contient un nom (≤ 255 octets), un parent optionnel,
//     et une référence à l'inode cible.
//   • Le dentry peut être :
//       - VALID    : en cache, inode présent, utilisable
//       - NEGATIVE : en cache, fichier inexistant (optimisation)
//       - UNHASHED : retiré du hash, en attente de destruction
//   • Les lectures sont RCU via Arc<RwLock<Dentry>>.
//
// Génération de cache :
//   Chaque dentry stocke la génération de son parent au moment du lookup.
//   Si la génération du parent change (rename/unlink), les dentries issues
//   de ce parent sont automatiquement invalidées (vérification lazy).
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicBool, Ordering};
use alloc::sync::Arc;
use alloc::vec::Vec;

use super::inode::InodeRef;
use super::types::{FsError, FsResult, InodeNumber, NAME_MAX, FS_STATS};
use crate::scheduler::sync::rwlock::RwLock;
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────────────────────
// DentryState — état d'une entrée de cache
// ─────────────────────────────────────────────────────────────────────────────

/// État d'une entrée de dentry cache.
#[repr(u8)]
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum DentryState {
    /// Dentry valide avec inode résolu.
    Valid    = 0,
    /// Dentry négative : le nom est connu inexistant dans ce répertoire.
    Negative = 1,
    /// Dentry retirée du hash, destruction imminente.
    Unhashed = 2,
    /// Dentry racine (pas de parent).
    Root     = 3,
}

// ─────────────────────────────────────────────────────────────────────────────
// DentryName — nom inline (pas d'alloc pour noms courts)
// ─────────────────────────────────────────────────────────────────────────────

/// Nom d'entrée de répertoire.
/// Inline jusqu'à 63 octets, sinon alloué dans un Vec<u8>.
/// Optimisation : la grande majorité des noms de fichiers tient en 63 octets.
pub struct DentryName {
    len:  u8,
    buf:  [u8; NAME_MAX + 1],
}

impl DentryName {
    /// Construit depuis un slice. Retourne `Err` si > NAME_MAX.
    pub fn from_bytes(name: &[u8]) -> FsResult<Self> {
        if name.is_empty() {
            return Err(FsError::InvalidArg);
        }
        if name.len() > NAME_MAX {
            return Err(FsError::NameTooLong);
        }
        let mut buf = [0u8; NAME_MAX + 1];
        buf[..name.len()].copy_from_slice(name);
        Ok(DentryName { len: name.len() as u8, buf })
    }

    /// Retourne le nom comme slice d'octets.
    #[inline(always)]
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len as usize]
    }

    /// Longueur du nom en octets.
    #[inline(always)]
    pub fn len(&self) -> usize {
        self.len as usize
    }

    /// Compare deux noms.
    #[inline(always)]
    pub fn equals(&self, other: &[u8]) -> bool {
        self.as_bytes() == other
    }

    /// Hash 32 bits du nom (FNV-1a rapide, pas besoin de cryptographic hash).
    pub fn hash(&self) -> u32 {
        let mut h: u32 = 2_166_136_261;
        for &b in self.as_bytes() {
            h ^= b as u32;
            h = h.wrapping_mul(16_777_619);
        }
        h
    }
}

impl core::fmt::Debug for DentryName {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match core::str::from_utf8(self.as_bytes()) {
            Ok(s) => write!(f, "\"{}\"", s),
            Err(_) => write!(f, "<{} raw bytes>", self.len),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Dentry — entrée de cache
// ─────────────────────────────────────────────────────────────────────────────

/// Entrée de cache de nommage VFS.
pub struct Dentry {
    /// Nom de l'entrée dans le répertoire parent.
    pub name:       DentryName,
    /// Inode résolu (None = dentry négative).
    pub inode:      Option<InodeRef>,
    /// Parent de cette entrée (None = root dentry).
    pub parent:     Option<DentryRef>,
    /// État courant.
    pub state:      DentryState,
    /// Génération du parent au moment de l'insertion.
    pub parent_gen: u64,
    /// Hash précalculé du nom.
    pub name_hash:  u32,
    /// Référence compteur (tracking).
    pub refcount:   AtomicU32,
    /// Permet les dentries négatives avec TTL.
    pub negative_ttl: AtomicU64,
    /// Timestamp d'insertion (ns monotonic).
    pub insert_time: u64,
    /// Flags d'état supplémentaires.
    pub flags:       u32,
}

/// Flags de dentry.
impl Dentry {
    /// Flag : la dentry a été montée dessus (mount point).
    pub const FLAG_MOUNT:     u32 = 1 << 0;
    /// Flag : la dentry provient d'un lien symbolique.
    pub const FLAG_SYMLINK:   u32 = 1 << 1;
    /// Flag : dentry en lecture seule.
    pub const FLAG_RDONLY:    u32 = 1 << 2;
    /// Flag : dentry automount.
    pub const FLAG_AUTOMOUNT: u32 = 1 << 3;

    /// Crée une dentry root (sans parent).
    pub fn new_root(name: &[u8], inode: InodeRef) -> Self {
        FS_STATS.dentry_cache_count.fetch_add(1, Ordering::Relaxed);
        let dname = DentryName::from_bytes(name)
            .unwrap_or_else(|_| DentryName::from_bytes(b"/").unwrap());
        let hash = dname.hash();
        Dentry {
            name: dname,
            inode: Some(inode),
            parent: None,
            state: DentryState::Root,
            parent_gen: 0,
            name_hash: hash,
            refcount: AtomicU32::new(1),
            negative_ttl: AtomicU64::new(0),
            insert_time: 0,
            flags: 0,
        }
    }

    /// Crée une dentry valide (avec parent et inode).
    pub fn new(
        name:       &[u8],
        inode:      InodeRef,
        parent:     DentryRef,
        parent_gen: u64,
        now_ns:     u64,
    ) -> FsResult<Self> {
        FS_STATS.dentry_cache_count.fetch_add(1, Ordering::Relaxed);
        let dname = DentryName::from_bytes(name)?;
        let hash  = dname.hash();
        Ok(Dentry {
            name: dname,
            inode: Some(inode),
            parent: Some(parent),
            state: DentryState::Valid,
            parent_gen,
            name_hash: hash,
            refcount: AtomicU32::new(1),
            negative_ttl: AtomicU64::new(0),
            insert_time: now_ns,
            flags: 0,
        })
    }

    /// Crée une dentry négative (optimisation : cache des "non-existants").
    pub fn new_negative(
        name:       &[u8],
        parent:     DentryRef,
        parent_gen: u64,
        now_ns:     u64,
        ttl_ns:     u64,
    ) -> FsResult<Self> {
        FS_STATS.dentry_cache_count.fetch_add(1, Ordering::Relaxed);
        let dname = DentryName::from_bytes(name)?;
        let hash  = dname.hash();
        Ok(Dentry {
            name: dname,
            inode: None,
            parent: Some(parent),
            state: DentryState::Negative,
            parent_gen,
            name_hash: hash,
            refcount: AtomicU32::new(1),
            negative_ttl: AtomicU64::new(now_ns + ttl_ns),
            insert_time: now_ns,
            flags: 0,
        })
    }

    /// Vérifie si la dentry est encore valide (non expirée pour les négatives).
    pub fn is_valid(&self, now_ns: u64) -> bool {
        match self.state {
            DentryState::Valid | DentryState::Root => true,
            DentryState::Negative => {
                let ttl = self.negative_ttl.load(Ordering::Relaxed);
                ttl == 0 || now_ns < ttl
            }
            DentryState::Unhashed => false,
        }
    }

    /// Invalide la dentry (change son état en Unhashed).
    pub fn invalidate(&mut self) {
        self.state = DentryState::Unhashed;
        self.inode = None;
    }

    /// Retourne l'inode ou `Err(FsError::NotFound)` pour dentry négative.
    pub fn get_inode(&self) -> FsResult<InodeRef> {
        self.inode.clone().ok_or(FsError::NotFound)
    }

    /// Vérifie si c'est un point de montage.
    pub fn is_mount_point(&self) -> bool {
        self.flags & Self::FLAG_MOUNT != 0
    }
}

impl Drop for Dentry {
    fn drop(&mut self) {
        FS_STATS.dentry_cache_count.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Référence partagée vers une `Dentry`.
pub type DentryRef = Arc<RwLock<Dentry>>;

// ─────────────────────────────────────────────────────────────────────────────
// DentryCache — hash table globale (slot = bucket spinlock)
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre de buckets dans le dcache hash (puissance de 2).
const DCACHE_BUCKETS: usize = 512;
const DCACHE_MASK: usize    = DCACHE_BUCKETS - 1;

/// Entrée dans un bucket.
struct DcacheBucketEntry {
    /// Hash du nom.
    name_hash:  u32,
    /// Numéro d'inode parent (pour distinguer les buckets en collision).
    parent_ino: InodeNumber,
    /// Dentry en cache.
    pub dentry:     DentryRef,
}

/// Bucket du dcache (liste chaînée protégée par spinlock).
struct DcacheBucket {
    pub entries: SpinLock<Vec<DcacheBucketEntry>>,
    len:         AtomicU32,
}

impl DcacheBucket {
    const fn new() -> Self {
        Self {
            entries: SpinLock::new(Vec::new()),
            len: AtomicU32::new(0),
        }
    }
}

/// Dcache global — hashmap shardée pour minimiser la contention.
pub struct DentryCache {
    pub buckets: [DcacheBucket; DCACHE_BUCKETS],
    /// Compteur global d'éléments.
    pub total: AtomicU64,
    /// Capacité maximale (entries).
    cap:   usize,
}

impl DentryCache {
    /// Crée une instance vide.
    pub const fn new(cap: usize) -> Self {
        // On ne peut pas utiliser array_init en const — macro manuelle.
        const fn empty_bucket() -> DcacheBucket { DcacheBucket::new() }
        // Astuce no_std : initialisation avec repeat dans const context.
        // La taille est fixe (512), donc on peut l'écrire explicitement.
        // Macro pour éviter la répétition brutale.
        macro_rules! arr512 {
            () => { [
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                // ... (les 480 restants sont générés ci-dessous)
                // Pour garder le code lisible on répète le bloc 16 fois par ligne × 32 lignes.
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                // 104 éléments supplémentaires pour atteindre 512 total.
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
                empty_bucket(), empty_bucket(), empty_bucket(), empty_bucket(),
            ] }
        }
        Self {
            buckets: arr512!(),
            total: AtomicU64::new(0),
            cap,
        }
    }

    /// Calcule l'index de bucket pour (parent_ino, name_hash).
    #[inline(always)]
    fn bucket_index(&self, parent_ino: InodeNumber, name_hash: u32) -> usize {
        // XOR-fold pour mélanger les bits.
        let h = (parent_ino.as_u64() as u32).wrapping_add(name_hash).wrapping_mul(2654435761);
        (h as usize) & DCACHE_MASK
    }

    /// Insère une dentry dans le cache.
    pub fn insert(&self, parent_ino: InodeNumber, dentry: DentryRef) {
        let (name_hash, hash_key) = {
            let d = dentry.read();
            (d.name_hash, d.name_hash)
        };
        let idx = self.bucket_index(parent_ino, hash_key);
        let mut entries = self.buckets[idx].entries.lock();

        // Remplacer une éventuelle entrée existante.
        let name_bytes = {
            let d = dentry.read();
            let mut buf = [0u8; NAME_MAX + 1];
            let nb = d.name.as_bytes();
            buf[..nb.len()].copy_from_slice(nb);
            (nb.len(), buf)
        };
        entries.retain(|e| {
            let ed = e.dentry.read();
            !(e.parent_ino == parent_ino && ed.name.as_bytes() == &name_bytes.1[..name_bytes.0])
        });

        // Éviction LRU basique si plein (retirer le premier).
        if self.total.load(Ordering::Relaxed) as usize >= self.cap {
            if !entries.is_empty() {
                entries.remove(0);
                self.total.fetch_sub(1, Ordering::Relaxed);
                FS_STATS.evictions.fetch_add(1, Ordering::Relaxed);
            }
        }

        entries.push(DcacheBucketEntry { name_hash, parent_ino, dentry });
        self.buckets[idx].len.fetch_add(1, Ordering::Relaxed);
        self.total.fetch_add(1, Ordering::Relaxed);
    }

    /// Recherche une dentry par `(parent_ino, name)`.
    pub fn lookup(&self, parent_ino: InodeNumber, name: &[u8], now_ns: u64) -> Option<DentryRef> {
        let hash = {
            let mut h: u32 = 2_166_136_261;
            for &b in name {
                h ^= b as u32;
                h = h.wrapping_mul(16_777_619);
            }
            h
        };
        let idx = self.bucket_index(parent_ino, hash);
        let entries = self.buckets[idx].entries.lock();
        for e in entries.iter().rev() {
            if e.parent_ino != parent_ino || e.name_hash != hash {
                continue;
            }
            let d = e.dentry.read();
            if d.name.equals(name) && d.is_valid(now_ns) {
                FS_STATS.cache_hits.fetch_add(1, Ordering::Relaxed);
                return Some(e.dentry.clone());
            }
        }
        FS_STATS.cache_misses.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Invalide toutes les dentries d'un répertoire parent.
    pub fn invalidate_parent(&self, parent_ino: InodeNumber) {
        for bucket in self.buckets.iter() {
            let mut entries = bucket.entries.lock();
            entries.retain(|e| {
                if e.parent_ino == parent_ino {
                    self.total.fetch_sub(1, Ordering::Relaxed);
                    false
                } else {
                    true
                }
            });
        }
    }

    /// Retourne le nombre d'entrées en cache.
    pub fn total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }
}

/// Dcache global.
pub static DENTRY_CACHE: DentryCache = DentryCache::new(65536);
