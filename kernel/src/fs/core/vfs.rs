// kernel/src/fs/core/vfs.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// VFS — Virtual File System — Interface unifiée (Exo-OS · Couche 3)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Le VFS est la couche d'abstraction centrale :
//   • Définit les traits `FileSystemOps`, `InodeOps`, `FileOps`, `DirOps`
//   • Maintient la table de montage globale (`MOUNT_TABLE`)
//   • Résout les chemins (`path_lookup`) avec traversée de points de montage
//   • Gère le registre de types de FS (`FsTypeRegistry`)
//
// RÈGLES :
//   FS-VFS-01 : Pas d'import direct de ipc/.
//               fs/ → security/capability/ directement (RÈGLE CAP-03).
//   FS-VFS-02 : Lock ordering respecté : Memory < FS (regle_bonus.md).
//   FS-VFS-03 : Pas d'allocation dans les chemins critiques de path_lookup.
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use alloc::sync::Arc;
use alloc::string::String;
use alloc::vec::Vec;

use super::types::{
    DevId, FsError, FsResult, FsStats, FileMode, InodeNumber,
    MountFlags, OpenFlags, Stat, Timespec64, Uid, Gid, NAME_MAX, PATH_MAX,
};
use super::inode::{Inode, InodeRef};
use super::dentry::{Dentry, DentryRef};
use crate::security::capability::{CapToken, Rights, verify, CapTable};

// ─────────────────────────────────────────────────────────────────────────────
// FsType — type de système de fichiers enregistré
// ─────────────────────────────────────────────────────────────────────────────

/// Identifie un type de système de fichiers.
/// Chaque implémentation FS enregistre une instance au démarrage.
pub trait FsType: Send + Sync {
    /// Nom du type ("ext4plus", "tmpfs", "procfs"…).
    fn name(&self) -> &'static str;

    /// Magic number pour statfs.
    fn magic(&self) -> u64;

    /// Monte ce FS depuis un périphérique bloc ou virtuel.
    ///
    /// # Arguments
    /// - `dev`   : identifiant du block device (DevId::NONE pour pseudo-fs)
    /// - `flags` : options de montage
    /// - `data`  : options texte ("errors=remount-ro,barrier=1"…)
    fn mount(
        &self,
        dev:   DevId,
        flags: MountFlags,
        data:  &str,
    ) -> FsResult<Arc<dyn Superblock>>;

    /// Démonte proprement (flush + sync).
    fn unmount(&self, sb: Arc<dyn Superblock>) -> FsResult<()>;
}

// ─────────────────────────────────────────────────────────────────────────────
// Superblock — super-bloc d'un FS monté
// ─────────────────────────────────────────────────────────────────────────────

/// Super-bloc = état global d'un FS monté.
/// Obtenu depuis `FsType::mount()`, stocké dans la table de montage.
pub trait Superblock: Send + Sync {
    /// Root inode (ino 2 pour ext4plus).
    fn root_inode(&self) -> FsResult<InodeRef>;

    /// Lectures des statistiques agrégées.
    fn statfs(&self) -> FsResult<FsStats>;

    /// Sync total des métadonnées + données dirty.
    fn sync_fs(&self, wait: bool) -> FsResult<()>;

    /// Remontage avec nouveaux flags (ex: ro→rw).
    fn remount(&self, flags: MountFlags, data: &str) -> FsResult<()>;

    /// Alloue un nouvel inode vierge.
    fn alloc_inode(&self) -> FsResult<InodeRef>;

    /// Libère un inode (nlink == 0).
    fn dealloc_inode(&self, ino: InodeNumber) -> FsResult<()>;

    /// Écrit le super-bloc sur le disque.
    fn write_super(&self) -> FsResult<()>;

    /// Retourne les flags de montage courants.
    fn flags(&self) -> MountFlags;

    /// Identifiant du périphérique sous-jacent.
    fn dev(&self) -> DevId;

    /// Indique si le FS est en lecture seule.
    fn is_readonly(&self) -> bool {
        self.flags().is_readonly()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// InodeOps — opérations sur un inode
// ─────────────────────────────────────────────────────────────────────────────

/// Table d'opérations d'un inode (dispatch polymorphe zero-vtable-overhead
/// via Arc<dyn InodeOps>).
pub trait InodeOps: Send + Sync {
    // ── Attributs ────────────────────────────────────────────────────────────
    fn getattr(&self, inode: &InodeRef) -> FsResult<Stat>;
    fn setattr(&self, inode: &InodeRef, attr: &InodeAttr) -> FsResult<()>;

    // ── Répertoires ──────────────────────────────────────────────────────────
    fn lookup(
        &self,
        dir:    &InodeRef,
        name:   &[u8],
    ) -> FsResult<DentryRef>;

    fn create(
        &self,
        dir:    &InodeRef,
        name:   &[u8],
        mode:   FileMode,
        uid:    Uid,
        gid:    Gid,
    ) -> FsResult<InodeRef>;

    fn mkdir(
        &self,
        dir:    &InodeRef,
        name:   &[u8],
        mode:   FileMode,
        uid:    Uid,
        gid:    Gid,
    ) -> FsResult<InodeRef>;

    fn rmdir(&self, dir: &InodeRef, name: &[u8]) -> FsResult<()>;
    fn unlink(&self, dir: &InodeRef, name: &[u8]) -> FsResult<()>;

    fn rename(
        &self,
        old_dir:  &InodeRef,
        old_name: &[u8],
        new_dir:  &InodeRef,
        new_name: &[u8],
        flags:    RenameFlags,
    ) -> FsResult<()>;

    fn link(
        &self,
        old_inode: &InodeRef,
        new_dir:   &InodeRef,
        new_name:  &[u8],
    ) -> FsResult<()>;

    fn symlink(
        &self,
        dir:    &InodeRef,
        name:   &[u8],
        target: &[u8],
        uid:    Uid,
        gid:    Gid,
    ) -> FsResult<InodeRef>;

    fn readlink(&self, inode: &InodeRef, buf: &mut [u8]) -> FsResult<usize>;

    // ── Fichiers spéciaux ─────────────────────────────────────────────────────
    fn mknod(
        &self,
        dir:  &InodeRef,
        name: &[u8],
        mode: FileMode,
        rdev: DevId,
        uid:  Uid,
        gid:  Gid,
    ) -> FsResult<InodeRef>;

    // ── Persistance ──────────────────────────────────────────────────────────
    fn write_inode(&self, inode: &InodeRef, sync: bool) -> FsResult<()>;
    fn evict_inode(&self, inode: &InodeRef) -> FsResult<()>;
}

// ─────────────────────────────────────────────────────────────────────────────
// FileOps — opérations sur un descripteur de fichier ouvert
// ─────────────────────────────────────────────────────────────────────────────

/// Table d'opérations sur un fichier ouvert.
pub trait FileOps: Send + Sync {
    /// Lecture depuis la position courante.
    fn read(&self, file: &FileHandle, buf: &mut [u8], offset: u64) -> FsResult<usize>;

    /// Écriture depuis la position courante.
    fn write(&self, file: &FileHandle, buf: &[u8], offset: u64) -> FsResult<usize>;

    /// Repositionnement du curseur.
    fn seek(&self, file: &FileHandle, offset: i64, whence: super::types::SeekWhence) -> FsResult<u64>;

    /// Contrôle d'un descripteur (ioctl).
    fn ioctl(&self, file: &FileHandle, cmd: u32, arg: u64) -> FsResult<i64>;

    /// `mmap` — retourne le backing store virtual addr  (optionnel).
    fn mmap(
        &self,
        file:   &FileHandle,
        offset: u64,
        length: usize,
        flags:  MmapFlags,
    ) -> FsResult<u64>;

    /// Itération répertoire (getdents64 backend).
    fn readdir(
        &self,
        file:    &FileHandle,
        offset:  &mut u64,
        emit:    &mut dyn FnMut(super::types::Dirent64) -> bool,
    ) -> FsResult<()>;

    /// Flush + sync données sur disque.
    fn fsync(&self, file: &FileHandle, datasync: bool) -> FsResult<()>;

    /// Alloue l'espace disque sans écrire (fallocate).
    fn fallocate(
        &self,
        file:   &FileHandle,
        mode:   u32,
        offset: u64,
        length: u64,
    ) -> FsResult<()>;

    /// Libération des ressources à la fermeture.
    fn release(&self, file: &FileHandle) -> FsResult<()>;

    /// Poll / epoll_wait — retourne les événements disponibles.
    fn poll(&self, file: &FileHandle) -> FsResult<PollEvents>;
}

// ─────────────────────────────────────────────────────────────────────────────
// FileHandle — handle d'un fichier ouvert (fd backing)
// ─────────────────────────────────────────────────────────────────────────────

/// État d'un fichier ouvert : inode + ops + position + flags.
pub struct FileHandle {
    /// Inode sous-jacent.
    pub inode:  InodeRef,
    /// Table d'opérations.
    pub ops:    Arc<dyn FileOps>,
    /// Position courante (atomique pour éviter un lock dans le hot path).
    pub pos:    AtomicU64,
    /// Flags d'ouverture (OpenFlags).
    pub flags:  OpenFlags,
    /// Mode d'accès demandé (R/W).
    pub mode:   u32,
    /// Nombre de références (géré via Arc en dehors).
    _marker:    core::marker::PhantomData<()>,
}

impl FileHandle {
    /// Crée un nouveau handle.
    pub fn new(inode: InodeRef, ops: Arc<dyn FileOps>, flags: OpenFlags) -> Self {
        Self {
            inode,
            ops,
            pos: AtomicU64::new(0),
            flags,
            mode: 0,
            _marker: core::marker::PhantomData,
        }
    }

    /// Lit la position courante.
    #[inline(always)]
    pub fn pos(&self) -> u64 {
        self.pos.load(Ordering::Relaxed)
    }

    /// Met à jour la position.
    #[inline(always)]
    pub fn set_pos(&self, p: u64) {
        self.pos.store(p, Ordering::Relaxed);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MmapFlags / PollEvents / RenameFlags / InodeAttr
// ─────────────────────────────────────────────────────────────────────────────

/// Flags `mmap(2)`.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct MmapFlags(pub u32);

impl MmapFlags {
    pub const PROT_READ:  u32 = 0x1;
    pub const PROT_WRITE: u32 = 0x2;
    pub const PROT_EXEC:  u32 = 0x4;
    pub const MAP_SHARED: u32 = 0x01;
    pub const MAP_PRIVATE:u32 = 0x02;
    pub const MAP_FIXED:  u32 = 0x10;
    pub const MAP_ANON:   u32 = 0x20;

    pub const fn new(v: u32) -> Self { MmapFlags(v) }
    pub const fn is_shared(self) -> bool { self.0 & Self::MAP_SHARED != 0 }
    pub const fn is_exec(self) -> bool   { self.0 & Self::PROT_EXEC != 0 }
    pub const fn is_write(self) -> bool  { self.0 & Self::PROT_WRITE != 0 }
}

/// Événements retournés par `poll`.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub struct PollEvents(pub u32);

impl PollEvents {
    pub const POLLIN:  u32 = 0x001;
    pub const POLLOUT: u32 = 0x004;
    pub const POLLERR: u32 = 0x008;
    pub const POLLHUP: u32 = 0x010;
    pub const POLLRDHUP: u32 = 0x2000;

    pub fn readable(self) -> bool  { self.0 & Self::POLLIN  != 0 }
    pub fn writable(self) -> bool  { self.0 & Self::POLLOUT != 0 }
    pub fn has_error(self) -> bool { self.0 & Self::POLLERR != 0 }
}

/// Flags `rename(2)` (RENAME_NOREPLACE, RENAME_EXCHANGE, RENAME_WHITEOUT).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct RenameFlags(pub u32);

impl RenameFlags {
    pub const NOREPLACE:  u32 = 1 << 0;
    pub const EXCHANGE:   u32 = 1 << 1;
    pub const WHITEOUT:   u32 = 1 << 2;

    pub const fn no_replace(self) -> bool { self.0 & Self::NOREPLACE != 0 }
    pub const fn exchange(self) -> bool   { self.0 & Self::EXCHANGE  != 0 }
}

/// Attributs modifiables d'un inode (setattr).
#[derive(Clone, Debug, Default)]
pub struct InodeAttr {
    pub valid_mask: u32,
    pub mode:  Option<FileMode>,
    pub uid:   Option<Uid>,
    pub gid:   Option<Gid>,
    pub size:  Option<u64>,
    pub atime: Option<Timespec64>,
    pub mtime: Option<Timespec64>,
    pub ctime: Option<Timespec64>,
}

impl InodeAttr {
    pub const ATTR_MODE:  u32 = 1 << 0;
    pub const ATTR_UID:   u32 = 1 << 1;
    pub const ATTR_GID:   u32 = 1 << 2;
    pub const ATTR_SIZE:  u32 = 1 << 3;
    pub const ATTR_ATIME: u32 = 1 << 4;
    pub const ATTR_MTIME: u32 = 1 << 5;
    pub const ATTR_CTIME: u32 = 1 << 6;

    pub const fn new() -> Self {
        Self { valid_mask: 0, mode: None, uid: None, gid: None,
               size: None, atime: None, mtime: None, ctime: None }
    }

    pub fn with_size(mut self, sz: u64) -> Self {
        self.size = Some(sz);
        self.valid_mask |= Self::ATTR_SIZE;
        self
    }

    pub fn with_mode(mut self, m: FileMode) -> Self {
        self.mode = Some(m);
        self.valid_mask |= Self::ATTR_MODE;
        self
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DefaultFileOps — implémentation stub de FileOps
// ─────────────────────────────────────────────────────────────────────────────

/// Implémentation par défaut de [`FileOps`] retournant `NotSupported`.
/// Utilisée comme placeholder lorsque l'inode n'expose pas ses propres FileOps.
pub struct DefaultFileOps;

impl FileOps for DefaultFileOps {
    fn read(&self, _: &FileHandle, _: &mut [u8], _: u64) -> FsResult<usize>
        { Err(FsError::NotSupported) }
    fn write(&self, _: &FileHandle, _: &[u8], _: u64) -> FsResult<usize>
        { Err(FsError::NotSupported) }
    fn seek(&self, _: &FileHandle, _: i64, _: super::types::SeekWhence) -> FsResult<u64>
        { Err(FsError::NotSupported) }
    fn ioctl(&self, _: &FileHandle, _: u32, _: u64) -> FsResult<i64>
        { Err(FsError::NotSupported) }
    fn mmap(&self, _: &FileHandle, _: u64, _: usize, _: MmapFlags) -> FsResult<u64>
        { Err(FsError::NotSupported) }
    fn readdir(&self, _: &FileHandle, _: &mut u64, _: &mut dyn FnMut(super::types::Dirent64) -> bool) -> FsResult<()>
        { Err(FsError::NotSupported) }
    fn fsync(&self, _: &FileHandle, _: bool) -> FsResult<()>
        { Ok(()) }
    fn fallocate(&self, _: &FileHandle, _: u32, _: u64, _: u64) -> FsResult<()>
        { Err(FsError::NotSupported) }
    fn release(&self, _: &FileHandle) -> FsResult<()>
        { Ok(()) }
    fn poll(&self, _: &FileHandle) -> FsResult<PollEvents>
        { Err(FsError::NotSupported) }
}

// ─────────────────────────────────────────────────────────────────────────────
// MountEntry — entrée dans la table de montage
// ─────────────────────────────────────────────────────────────────────────────

/// Entrée décrivant un point de montage actif.
#[derive(Clone)]
pub struct MountEntry {
    /// Chemin de montage (ex: "/", "/proc", "/home").
    pub path:    String,
    /// Superbloc du FS monté.
    pub sb:      Arc<dyn Superblock>,
    /// Root dentry du FS (inode 2 pour ext4plus).
    pub root:    DentryRef,
    /// Flags de montage.
    pub flags:   MountFlags,
    /// Numéro de montage unique (croissant).
    pub mount_id: u32,
    /// Périphérique sous-jacent.
    pub dev:     DevId,
}

// ─────────────────────────────────────────────────────────────────────────────
// MountTable — table de montage globale
// ─────────────────────────────────────────────────────────────────────────────

/// Capacité maximale de points de montage simultanés.
const MAX_MOUNTS: usize = 256;

/// Compteur global de mount_id.
static MOUNT_ID_COUNTER: AtomicU32 = AtomicU32::new(1);

/// Indicateur d'initialisation.
static VFS_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Table de montage protégée par un spinlock.
/// Lock ordering : memory < FS — le lock FS ne peut pas être pris
/// alors qu'on tient un lock memory (regle_bonus.md).
use crate::scheduler::sync::spinlock::SpinLock;

pub struct MountTable {
    entries: SpinLock<Vec<MountEntry>>,
    count:   AtomicUsize,
}

impl MountTable {
    const fn new() -> Self {
        Self {
            entries: SpinLock::new(Vec::new()),
            count:   AtomicUsize::new(0),
        }
    }

    /// Ajoute un point de montage.
    pub fn add(&self, entry: MountEntry) -> FsResult<()> {
        let mut guard = self.entries.lock();
        if guard.len() >= MAX_MOUNTS {
            return Err(FsError::NoSpace);
        }
        guard.push(entry);
        self.count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Supprime le point de montage correspondant à `path`.
    pub fn remove(&self, path: &str) -> FsResult<MountEntry> {
        let mut guard = self.entries.lock();
        let pos = guard.iter().position(|e| e.path == path)
            .ok_or(FsError::NotFound)?;
        let entry = guard.remove(pos);
        self.count.fetch_sub(1, Ordering::Relaxed);
        Ok(entry)
    }

    /// Cherche le point de montage racovrant `path` (longest prefix match).
    pub fn find_mount(&self, path: &str) -> Option<MountEntry> {
        let guard = self.entries.lock();
        // Longest prefix match : on privilégie le montage le plus profond.
        guard.iter()
            .filter(|e| path.starts_with(e.path.as_str()))
            .max_by_key(|e| e.path.len())
            .cloned()
    }

    /// Nombre de points de montage actifs.
    pub fn count(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }
}

/// Table de montage globale.
pub static MOUNT_TABLE: MountTable = MountTable::new();

// ─────────────────────────────────────────────────────────────────────────────
// FsTypeRegistry — registre des types de FS
// ─────────────────────────────────────────────────────────────────────────────

/// Capacité du registre de types de FS.
const MAX_FS_TYPES: usize = 32;

/// Registre protégé par spinlock.
pub struct FsTypeRegistry {
    types: SpinLock<Vec<Arc<dyn FsType>>>,
}

impl FsTypeRegistry {
    const fn new() -> Self {
        Self { types: SpinLock::new(Vec::new()) }
    }

    /// Enregistre un nouveau type de FS.
    pub fn register(&self, fs: Arc<dyn FsType>) -> FsResult<()> {
        let mut guard = self.types.lock();
        if guard.len() >= MAX_FS_TYPES {
            return Err(FsError::NoSpace);
        }
        // Interdit les doublons.
        if guard.iter().any(|t| t.name() == fs.name()) {
            return Err(FsError::Exists);
        }
        guard.push(fs);
        Ok(())
    }

    /// Recherche un type par nom.
    pub fn find(&self, name: &str) -> Option<Arc<dyn FsType>> {
        let guard = self.types.lock();
        guard.iter().find(|t| t.name() == name).cloned()
    }

    /// Désenregistre un type (lors de déchargement de module).
    pub fn unregister(&self, name: &str) -> FsResult<()> {
        let mut guard = self.types.lock();
        let pos = guard.iter().position(|t| t.name() == name)
            .ok_or(FsError::NotFound)?;
        guard.remove(pos);
        Ok(())
    }
}

/// Registre global des types FS.
pub static FS_TYPE_REGISTRY: FsTypeRegistry = FsTypeRegistry::new();

// ─────────────────────────────────────────────────────────────────────────────
// Path lookup
// ─────────────────────────────────────────────────────────────────────────────

/// Contexte de résolution de chemin.
pub struct LookupContext<'a> {
    /// Répertoire de départ pour chemins relatifs (None = cwd du processus).
    pub start_dir:  Option<InodeRef>,
    /// Table de capabilities du processus appelant.
    pub cap_table:  &'a CapTable,
    /// UID de l'appelant.
    pub uid:        Uid,
    /// GID de l'appelant.
    pub gid:        Gid,
    /// Ne pas suivre les liens symboliques pour le dernier composant.
    pub nofollow:   bool,
    /// Nombre de liens symboliques déjà suivis (anti-boucle, ≤ MAXSYMLINKS).
    pub symlink_count: u32,
}

/// Résultat d'une résolution de chemin.
pub struct LookupResult {
    /// Inode final.
    pub inode:   InodeRef,
    /// Dentry finale.
    pub dentry:  DentryRef,
    /// Super-bloc du FS contenant la cible.
    pub sb:      Arc<dyn Superblock>,
}

/// Résout un chemin absolu ou relatif vers un `LookupResult`.
///
/// # Sécurité
/// - Vérifie les permissions X sur chaque répertoire traversé.
/// - Limite la traversée de liens symboliques à `MAXSYMLINKS`.
/// - Gère les traversées de points de montage.
///
/// RÈGLE FS-VFS-03 : pas d'allocation dans le hot path de lookup.
/// La résolution travaille sur des slices empilées.
pub fn path_lookup(
    path: &[u8],
    ctx:  &LookupContext<'_>,
) -> FsResult<LookupResult> {
    if path.is_empty() {
        return Err(FsError::NotFound);
    }
    if path.len() > PATH_MAX {
        return Err(FsError::NameTooLong);
    }

    // Trouver le montage racovrant.
    let path_str = core::str::from_utf8(path).map_err(|_| FsError::InvalidArg)?;
    let mount = MOUNT_TABLE.find_mount(path_str)
        .ok_or(FsError::NotFound)?;

    // Partir depuis le root dentry du montage.
    let root_inode = mount.sb.root_inode()?;

    // Découper le chemin en composants (pas d'alloc : on itère sur des slices).
    let rel = if path_str.starts_with('/') {
        path_str.trim_start_matches('/')
    } else {
        path_str
    };

    let mut current_inode = root_inode.clone();
    let mut current_dentry = mount.root.clone();

    if rel.is_empty() {
        return Ok(LookupResult {
            inode: current_inode,
            dentry: current_dentry,
            sb: mount.sb.clone(),
        });
    }

    // Itération composant par composant.
    let mut components = rel.split('/').filter(|c| !c.is_empty()).peekable();
    while let Some(component) = components.next() {
        let is_last = components.peek().is_none();

        // Vérifications de base.
        if component.len() > NAME_MAX {
            return Err(FsError::NameTooLong);
        }
        if component == ".." {
            // Remonter d'un niveau — géré par le dentry parent.
            // Pour l'instant, on reste au même niveau (pas de crossing mount ici).
            continue;
        }
        if component == "." {
            continue;
        }

        // Vérifier que l'inode courant est bien un répertoire.
        {
            let inode_guard = current_inode.read();
            if !inode_guard.mode.is_dir() {
                return Err(FsError::NotDir);
            }
            // Vérifier permission X sur le répertoire traversé.
            let uid_match = inode_guard.uid == ctx.uid;
            let gid_match = inode_guard.gid == ctx.gid;
            if !ctx.uid.is_root() && !inode_guard.mode.check_access(uid_match, gid_match, false, false, true) {
                return Err(FsError::Access);
            }
        }

        // Lookup dans le répertoire.
        let ops = {
            let ig = current_inode.read();
            ig.ops.clone().ok_or(FsError::NotSupported)?
        };
        let child_dentry = ops.lookup(&current_inode, component.as_bytes())?;
        let child_inode = {
            let cd = child_dentry.read();
            cd.inode.clone().ok_or(FsError::NotFound)?
        };

        // Gestion liens symboliques (sauf nofollow sur le dernier composant).
        if !is_last || !ctx.nofollow {
            let is_symlink = {
                let ci = child_inode.read();
                ci.mode.is_symlink()
            };
            if is_symlink {
                if ctx.symlink_count >= super::types::MAXSYMLINKS {
                    return Err(FsError::Loop);
                }
                // Lire la cible — buffer sur la pile (PATH_MAX).
                let mut target_buf = [0u8; PATH_MAX];
                let len = {
                    let ci = child_inode.read();
                    let symlink_ops = ci.ops.clone().ok_or(FsError::NotSupported)?;
                    symlink_ops.readlink(&child_inode, &mut target_buf)?
                };
                // Récursion avec compteur incrémenté.
                let mut sub_ctx = LookupContext {
                    start_dir:     None,
                    cap_table:     ctx.cap_table,
                    uid:           ctx.uid,
                    gid:           ctx.gid,
                    nofollow:      false,
                    symlink_count: ctx.symlink_count + 1,
                };
                return path_lookup(&target_buf[..len], &sub_ctx);
            }
        }

        current_inode  = child_inode;
        current_dentry = child_dentry;
    }

    Ok(LookupResult {
        inode:  current_inode,
        dentry: current_dentry,
        sb:     mount.sb.clone(),
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// VFS mount / umount
// ─────────────────────────────────────────────────────────────────────────────

/// Monte un système de fichiers sur `mountpoint`.
///
/// # Arguments
/// - `fs_type`    : type enregistré dans `FS_TYPE_REGISTRY`
/// - `dev`        : block device (DevId::NONE pour pseudo-fs)
/// - `mountpoint` : chemin absolu
/// - `flags`      : options de montage
/// - `data`       : données texte
pub fn vfs_mount(
    fs_type:    &str,
    dev:        DevId,
    mountpoint: &str,
    flags:      MountFlags,
    data:       &str,
) -> FsResult<()> {
    // Récupère le type FS.
    let fs = FS_TYPE_REGISTRY.find(fs_type)
        .ok_or(FsError::NotSupported)?;

    // Monte le FS.
    let sb = fs.mount(dev, flags, data)?;

    // Récupère la root dentry.
    let root_inode = sb.root_inode()?;
    let root_dentry = {
        use super::dentry::Dentry;
        let ig = root_inode.read();
        let d = Dentry::new_root(b"/", root_inode.clone());
        Arc::new(crate::scheduler::sync::rwlock::RwLock::new(d))
    };

    let entry = MountEntry {
        path:     String::from(mountpoint),
        sb:       sb.clone(),
        root:     root_dentry,
        flags,
        mount_id: MOUNT_ID_COUNTER.fetch_add(1, Ordering::Relaxed),
        dev,
    };

    MOUNT_TABLE.add(entry)?;

    crate::fs::core::types::FS_STATS.inode_cache_count
        .fetch_add(0, Ordering::Relaxed); // trigger init stats

    Ok(())
}

/// Démonte le FS monté sur `mountpoint`.
pub fn vfs_umount(mountpoint: &str, lazy: bool) -> FsResult<()> {
    let entry = MOUNT_TABLE.remove(mountpoint)?;
    // Sync final avant démontage.
    entry.sb.sync_fs(true)?;
    // Libération du superbloc prise en charge par Arc<dyn Superblock>::drop.
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Initialisation
// ─────────────────────────────════════════════════════════════════════════════

/// Initialise le VFS.
/// Appelé par `fs::init()` pendant la séquence de boot.
pub fn vfs_init() {
    if VFS_INITIALIZED.swap(true, Ordering::SeqCst) {
        panic!("vfs_init appelé deux fois");
    }
    // Les Vec internes s'initialisent lazily au premier push.
    // Pas d'allocation au cold path d'init.
}

// ─────────────────────────────────────────────────────────────────────────────
// Points d'entrée VFS publics — appelés depuis syscall/
// ─────────────────────────────────────────────────────────────────────────────

/// Lit jusqu'à `len` octets depuis le descripteur `fd` vers `buf_ptr` (espace utilisateur).
pub fn fd_read(fd: i32, buf_ptr: u64, len: usize) -> FsResult<usize> {
    let _ = (fd, buf_ptr, len);
    Err(FsError::NotSupported)
}

/// Écrit jusqu'à `len` octets depuis `buf_ptr` (espace utilisateur) vers le descripteur `fd`.
pub fn fd_write(fd: i32, buf_ptr: u64, len: usize) -> FsResult<usize> {
    let _ = (fd, buf_ptr, len);
    Err(FsError::NotSupported)
}

/// Ouvre le fichier désigné par `path` avec les options `flags`/`mode`.
/// Retourne le nouveau descripteur de fichier.
pub fn open(path: &[u8], flags: u32, mode: u32) -> FsResult<u32> {
    let _ = (path, flags, mode);
    Err(FsError::NotSupported)
}

/// Ferme le descripteur `fd`.
pub fn close(fd: i32) -> FsResult<()> {
    let _ = fd;
    Err(FsError::NotSupported)
}

/// Repositionne la tête de lecture du descripteur `fd`.
pub fn lseek(fd: i32, offset: i64, whence: u32) -> FsResult<i64> {
    let _ = (fd, offset, whence);
    Err(FsError::NotSupported)
}

/// Ouvre `path` relativement au répertoire `dirfd`.
pub fn openat(dirfd: i32, path: &[u8], flags: u32, mode: u32) -> FsResult<u32> {
    let _ = (dirfd, path, flags, mode);
    Err(FsError::NotSupported)
}

/// Duplique le descripteur `fd`, retourne le nouveau fd.
pub fn dup(fd: i32) -> FsResult<i32> {
    let _ = fd;
    Err(FsError::NotSupported)
}

/// Duplique `old_fd` vers `new_fd` (ferme `new_fd` si nécessaire).
pub fn dup2(old_fd: i32, new_fd: i32) -> FsResult<i32> {
    let _ = (old_fd, new_fd);
    Err(FsError::NotSupported)
}

/// Manipulation de descripteur via `fcntl`.
pub fn fcntl(fd: i32, cmd: u32, arg: u64) -> FsResult<i64> {
    let _ = (fd, cmd, arg);
    Err(FsError::NotSupported)
}

/// Remplit `stat_ptr` (espace utilisateur) avec les métadonnées de `path`.
pub fn stat(path: &[u8], stat_ptr: u64) -> FsResult<()> {
    let _ = (path, stat_ptr);
    Err(FsError::NotSupported)
}

/// Remplit `stat_ptr` avec les métadonnées du descripteur `fd`.
pub fn fstat(fd: i32, stat_ptr: u64) -> FsResult<()> {
    let _ = (fd, stat_ptr);
    Err(FsError::NotSupported)
}

/// Crée le répertoire `path` avec les permissions `mode`.
pub fn mkdir(path: &[u8], mode: u32) -> FsResult<()> {
    let _ = (path, mode);
    Err(FsError::NotSupported)
}

/// Supprime le répertoire vide `path`.
pub fn rmdir(path: &[u8]) -> FsResult<()> {
    let _ = path;
    Err(FsError::NotSupported)
}

/// Supprime l'entrée de répertoire `path` (fichier ordinaire ou lien symbolique).
pub fn unlink(path: &[u8]) -> FsResult<()> {
    let _ = path;
    Err(FsError::NotSupported)
}

/// Lit les entrées du répertoire `fd` dans le tampon utilisateur `dirp` (`len` octets).
/// Retourne le nombre d'octets écrits.
pub fn getdents64(fd: u32, dirp: u64, len: usize) -> FsResult<usize> {
    let _ = (fd, dirp, len);
    Err(FsError::NotSupported)
}

/// Lit la cible du lien symbolique `path` dans `buf_ptr` (max `len` octets).
/// Retourne la longueur de la cible.
pub fn readlink(path: &[u8], buf_ptr: u64, len: usize) -> FsResult<usize> {
    let _ = (path, buf_ptr, len);
    Err(FsError::NotSupported)
}

/// Comme `readlink`, mais le chemin est relatif au répertoire `dirfd`.
pub fn readlinkat(dirfd: i32, path: &[u8], buf_ptr: u64, len: usize) -> FsResult<usize> {
    let _ = (dirfd, path, buf_ptr, len);
    Err(FsError::NotSupported)
}
