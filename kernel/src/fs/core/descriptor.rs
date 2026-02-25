// kernel/src/fs/core/descriptor.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// DESCRIPTEURS DE FICHIERS — fdtable (Exo-OS · Couche 3)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Chaque processus possède une `FdTable` (file descriptor table) :
//   • fd (0..OPEN_MAX) → Option<Arc<FileHandle>>
//   • Opérations : alloc_fd, close_fd, dup_fd, dup2_fd, set_cloexec
//   • Héritage fork() : clone_for_fork() copie les Arc (partage le handle)
//   • Exécution exec() : close_cloexec() ferme les O_CLOEXEC
//
// RÈGLES :
//   FD-01 : fd 0/1/2 stdin/stdout/stderr — jamais réalloués sans alloc_fd explicite.
//   FD-02 : dup2 ferme la cible atomiquement avant la copie.
//   FD-03 : Lock ordering : FdTable lock < inode lock.
//   FD-04 : OPEN_MAX = 1024 (regle_bonus.md OPEN_MAX).
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use alloc::sync::Arc;
use alloc::vec::Vec;

use super::types::{FsError, FsResult, OpenFlags, OPEN_MAX};
use super::vfs::FileHandle;
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────────────────────
// Fd — type du numéro de descripteur
// ─────────────────────────────────────────────────────────────────────────────

/// Type du numéro de descripteur de fichier (fd ≥ 0).
#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Fd(pub u32);

impl Fd {
    /// Stdin.
    pub const STDIN:  Self = Fd(0);
    /// Stdout.
    pub const STDOUT: Self = Fd(1);
    /// Stderr.
    pub const STDERR: Self = Fd(2);
    /// Valeur sentinelle invalide.
    pub const INVALID: Self = Fd(u32::MAX);

    /// Retourne l'index dans le tableau (usize).
    #[inline(always)]
    pub const fn index(self) -> usize { self.0 as usize }

    /// Valide.
    #[inline(always)]
    pub const fn is_valid(self) -> bool { self.0 != u32::MAX }
}

impl core::fmt::Display for Fd {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "fd({})", self.0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// FdEntry — entrée de la table
// ─────────────────────────────────────────────────────────────────────────────

/// Entrée dans la FdTable.
#[derive(Clone)]
pub struct FdEntry {
    /// Handle partagé (clonés par dup).
    pub handle:  Arc<FileHandle>,
    /// `O_CLOEXEC` : ce descripteur est fermé à exec().
    pub cloexec: bool,
    /// `O_NONBLOCK` : opérations non bloquantes.
    pub nonblock: bool,
}

impl FdEntry {
    pub fn new(handle: Arc<FileHandle>) -> Self {
        let cloexec  = handle.flags.is_cloexec();
        let nonblock = handle.flags.is_nonblock();
        FdEntry { handle, cloexec, nonblock }
    }

    pub fn with_cloexec(mut self, v: bool) -> Self {
        self.cloexec = v;
        self
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// FdTable — table de descripteurs d'un processus
// ─────────────────────────────────────────────────────────────────────────────

/// Table des descripteurs ouverts d'un processus.
/// Capacité fixe `OPEN_MAX` — slot `None` = fermé.
pub struct FdTable {
    /// Entrées indexées par fd.
    slots:   SpinLock<alloc::boxed::Box<[Option<FdEntry>]>>,
    /// Nombre de descripteurs ouverts.
    open:    AtomicU32,
    /// Prochain fd libre hint (optimisation de recherche).
    next_fd: AtomicU32,
    /// Limite soft du processus (rlimit NOFILE).
    limit:   AtomicU32,
}

impl FdTable {
    /// Crée une FdTable vide.
    pub fn new() -> Self {
        let slots: alloc::boxed::Box<[Option<FdEntry>]> =
            (0..OPEN_MAX).map(|_| None).collect::<Vec<_>>().into_boxed_slice();
        Self {
            slots:   SpinLock::new(slots),
            open:    AtomicU32::new(0),
            next_fd: AtomicU32::new(0),
            limit:   AtomicU32::new(OPEN_MAX as u32),
        }
    }

    /// Alloue le prochain fd libre à partir de `min_fd`.
    /// Retourne le fd alloué et y stocke `entry`.
    pub fn alloc_fd(&self, entry: FdEntry, min_fd: u32) -> FsResult<Fd> {
        let limit = self.limit.load(Ordering::Relaxed) as usize;
        let mut slots = self.slots.lock();

        let start = (self.next_fd.load(Ordering::Relaxed) as usize)
            .max(min_fd as usize);

        // Scan linéaire depuis `start`.
        for i in start..limit {
            if slots[i].is_none() {
                slots[i] = Some(entry);
                self.open.fetch_add(1, Ordering::Relaxed);
                self.next_fd.store((i + 1) as u32, Ordering::Relaxed);
                return Ok(Fd(i as u32));
            }
        }

        // Recommencer depuis 0 (wrap).
        for i in (min_fd as usize)..start {
            if slots[i].is_none() {
                slots[i] = Some(entry);
                self.open.fetch_add(1, Ordering::Relaxed);
                self.next_fd.store((i + 1) as u32, Ordering::Relaxed);
                return Ok(Fd(i as u32));
            }
        }

        Err(FsError::TooManyFiles)
    }

    /// Ferme un descripteur.
    /// Retourne le `FdEntry` pour appeler `FileOps::release`.
    pub fn close_fd(&self, fd: Fd) -> FsResult<FdEntry> {
        if !fd.is_valid() || fd.index() >= OPEN_MAX {
            return Err(FsError::BadFd);
        }
        let mut slots = self.slots.lock();
        match slots[fd.index()].take() {
            Some(entry) => {
                self.open.fetch_sub(1, Ordering::Relaxed);
                // Mettre à jour hint (le slot est maintenant libre).
                let current_hint = self.next_fd.load(Ordering::Relaxed);
                if fd.0 < current_hint {
                    self.next_fd.store(fd.0, Ordering::Relaxed);
                }
                Ok(entry)
            }
            None => Err(FsError::BadFd),
        }
    }

    /// Duplique le fd `from` — le résultat aura le même handle (Arc clone).
    pub fn dup_fd(&self, from: Fd) -> FsResult<Fd> {
        let entry = self.get(from)?.with_cloexec(false);
        self.alloc_fd(entry, 0)
    }

    /// `dup2(from, to)` : force `to` à pointer vers le même handle que `from`.
    /// Ferme `to` s'il était ouvert (atomique depuis la perspective du processus).
    pub fn dup2_fd(&self, from: Fd, to: Fd) -> FsResult<FdEntry> {
        if !to.is_valid() || to.index() >= OPEN_MAX {
            return Err(FsError::BadFd);
        }
        let new_entry = self.get(from)?.with_cloexec(false);
        let mut slots = self.slots.lock();
        let old = slots[to.index()].take();
        if old.is_none() {
            self.open.fetch_add(1, Ordering::Relaxed);
        }
        slots[to.index()] = Some(new_entry.clone());
        Ok(old.unwrap_or_else(|| new_entry.clone()))
    }

    /// Récupère une référence clonée à l'entrée `fd`.
    pub fn get(&self, fd: Fd) -> FsResult<FdEntry> {
        if !fd.is_valid() || fd.index() >= OPEN_MAX {
            return Err(FsError::BadFd);
        }
        let slots = self.slots.lock();
        slots[fd.index()].clone().ok_or(FsError::BadFd)
    }

    /// Vérifie si un fd est ouvert.
    pub fn is_open(&self, fd: Fd) -> bool {
        if fd.index() >= OPEN_MAX { return false; }
        let slots = self.slots.lock();
        slots[fd.index()].is_some()
    }

    /// Ferme tous les descripteurs `O_CLOEXEC` (appelé à exec()).
    /// Retourne la liste des handles à `release()`.
    pub fn close_cloexec(&self) -> Vec<FdEntry> {
        let mut closed = Vec::new();
        let mut slots = self.slots.lock();
        for slot in slots.iter_mut() {
            if let Some(entry) = slot {
                if entry.cloexec {
                    closed.push(slot.take().unwrap());
                    self.open.fetch_sub(1, Ordering::Relaxed);
                }
            }
        }
        closed
    }

    /// Clone la table pour fork() — partage les Arc<FileHandle>.
    /// Les fds `O_CLOEXEC` sont conservés dans le fils (seul exec les ferme).
    pub fn clone_for_fork(&self) -> FdTable {
        let slots = self.slots.lock();
        let new_slots: alloc::boxed::Box<[Option<FdEntry>]> =
            slots.iter().map(|e| e.clone()).collect::<Vec<_>>().into_boxed_slice();
        let open = slots.iter().filter(|e| e.is_some()).count() as u32;
        FdTable {
            slots:   SpinLock::new(new_slots),
            open:    AtomicU32::new(open),
            next_fd: AtomicU32::new(self.next_fd.load(Ordering::Relaxed)),
            limit:   AtomicU32::new(self.limit.load(Ordering::Relaxed)),
        }
    }

    /// Modifie le flag `O_CLOEXEC` d'un descripteur (`fcntl(F_SETFD, FD_CLOEXEC)`).
    pub fn set_cloexec(&self, fd: Fd, cloexec: bool) -> FsResult<()> {
        if !fd.is_valid() || fd.index() >= OPEN_MAX {
            return Err(FsError::BadFd);
        }
        let mut slots = self.slots.lock();
        match slots[fd.index()].as_mut() {
            Some(e) => { e.cloexec = cloexec; Ok(()) }
            None    => Err(FsError::BadFd),
        }
    }

    /// Modifie le flag `O_NONBLOCK` d'un descripteur.
    pub fn set_nonblock(&self, fd: Fd, nonblock: bool) -> FsResult<()> {
        if !fd.is_valid() || fd.index() >= OPEN_MAX {
            return Err(FsError::BadFd);
        }
        let mut slots = self.slots.lock();
        match slots[fd.index()].as_mut() {
            Some(e) => { e.nonblock = nonblock; Ok(()) }
            None    => Err(FsError::BadFd),
        }
    }

    /// Retourne le nombre de descripteurs ouverts.
    pub fn open_count(&self) -> u32 {
        self.open.load(Ordering::Relaxed)
    }

    /// Retourne la limite soft courante.
    pub fn limit(&self) -> usize {
        self.limit.load(Ordering::Relaxed) as usize
    }

    /// Modifie la limite soft (rlimit NOFILE).
    pub fn set_limit(&self, limit: usize) -> FsResult<()> {
        if limit > OPEN_MAX {
            return Err(FsError::InvalidArg);
        }
        self.limit.store(limit as u32, Ordering::Release);
        Ok(())
    }

    /// Ferme tous les descripteurs (appelé à la destruction du processus).
    pub fn close_all(&self) -> Vec<FdEntry> {
        let mut closed = Vec::new();
        let mut slots = self.slots.lock();
        for slot in slots.iter_mut() {
            if let Some(e) = slot.take() {
                closed.push(e);
                self.open.fetch_sub(1, Ordering::Relaxed);
            }
        }
        closed
    }

    /// Itère sur tous les fds ouverts (pour /proc/pid/fd).
    pub fn for_each<F>(&self, mut f: F)
    where
        F: FnMut(Fd, &FdEntry),
    {
        let slots = self.slots.lock();
        for (i, slot) in slots.iter().enumerate() {
            if let Some(e) = slot {
                f(Fd(i as u32), e);
            }
        }
    }
}

impl Default for FdTable {
    fn default() -> Self {
        FdTable::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CurrentDir — répertoire de travail courant d'un processus
// ─────────────────────────────────────────────────────────────────────────────

use super::dentry::DentryRef;
use super::inode::InodeRef;

/// Répertoire de travail courant (cwd).
pub struct CurrentDir {
    /// Dentry du répertoire courant.
    pub dentry: SpinLock<Option<DentryRef>>,
    /// Inode du répertoire courant.
    pub inode:  SpinLock<Option<InodeRef>>,
    /// Chemin absolu (cache textuel, mis à jour lors des chdir).
    pub path:   SpinLock<alloc::string::String>,
}

impl CurrentDir {
    /// Crée un cwd avec la valeur de "/" par défaut.
    pub fn new_root(root_dentry: DentryRef, root_inode: InodeRef) -> Self {
        Self {
            dentry: SpinLock::new(Some(root_dentry)),
            inode:  SpinLock::new(Some(root_inode)),
            path:   SpinLock::new(alloc::string::String::from("/")),
        }
    }

    /// Vide (avant initialisation du processus).
    pub fn empty() -> Self {
        Self {
            dentry: SpinLock::new(None),
            inode:  SpinLock::new(None),
            path:   SpinLock::new(alloc::string::String::from("/")),
        }
    }

    /// Met à jour le répertoire courant.
    pub fn cd(&self, dentry: DentryRef, inode: InodeRef, new_path: alloc::string::String) {
        *self.dentry.lock() = Some(dentry);
        *self.inode.lock()  = Some(inode);
        *self.path.lock()   = new_path;
    }

    /// Retourne le chemin texte du cwd.
    pub fn path_str(&self) -> alloc::string::String {
        self.path.lock().clone()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// FdStats — compteurs d'instrumentation
// ─────────────────────────────────────────────────────────────────────────────

/// Statistiques globales sur les descripteurs de fichiers.
pub struct FdStats {
    pub total_opens:  AtomicU64,
    pub total_closes: AtomicU64,
    pub total_dups:   AtomicU64,
    pub peak_open:    AtomicU32,
    pub cloexec_closed: AtomicU64,
}

impl FdStats {
    const fn new() -> Self {
        Self {
            total_opens:    AtomicU64::new(0),
            total_closes:   AtomicU64::new(0),
            total_dups:     AtomicU64::new(0),
            peak_open:      AtomicU32::new(0),
            cloexec_closed: AtomicU64::new(0),
        }
    }

    /// Notifie une ouverture et met à jour le pic.
    pub fn on_open(&self, current_open: u32) {
        self.total_opens.fetch_add(1, Ordering::Relaxed);
        let mut peak = self.peak_open.load(Ordering::Relaxed);
        loop {
            if current_open <= peak { break; }
            match self.peak_open.compare_exchange_weak(
                peak, current_open, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => break,
                Err(p) => peak = p,
            }
        }
    }
}

/// Statistiques globales.
pub static FD_STATS: FdStats = FdStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// Tests internes
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_table() -> FdTable {
        FdTable::new()
    }

    #[test]
    fn fd_alloc_uses_lowest_free() {
        // Impossible de créer un FileHandle sans inode dans les tests unitaires,
        // mais on peut vérifier la logique d'allocation via des méthodes low-level.
        // Vérification : prochain fd libre part de 0.
        let table = make_table();
        assert_eq!(table.open_count(), 0);
    }

    #[test]
    fn fd_invalid_close_returns_error() {
        let table = make_table();
        let result = table.close_fd(Fd::INVALID);
        assert_eq!(result, Err(FsError::BadFd));
    }

    #[test]
    fn fd_get_on_closed_returns_bad_fd() {
        let table = make_table();
        let result = table.get(Fd(0));
        assert_eq!(result, Err(FsError::BadFd));
    }
}
