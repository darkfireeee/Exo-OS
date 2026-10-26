//! # process/core/pcb/file_table.rs — Table des fichiers ouverts par processus
//!
//! Gère l'attribution et le suivi des descripteurs de fichiers pour un processus.
//! Opaque vis-à-vis de FS : les descripteurs portent des handles entiers sans import direct fs/.

extern crate alloc;

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

/// Un descripteur de fichier : opaque (géré par fs/ via handle entier).
#[derive(Copy, Clone, Debug)]
pub struct FileDescriptor {
    /// Numéro fd (0=stdin, 1=stdout, 2=stderr, 3+...).
    pub fd: i32,
    /// Handle opaque vers l'entrée fs/ (index dans la table vfs).
    pub handle: u64,
    /// Flags O_CLOEXEC, O_NONBLOCK...
    pub flags: u32,
}

/// Table des fichiers ouverts d'un processus (partagée entre threads via fork+CLONE_FILES).
pub struct OpenFileTable {
    /// fd_limit par processus (configurable via rlimit RLIMIT_NOFILE).
    fd_limit: usize,
    /// Descripteurs actifs. Index = numéro fd.
    descriptors: Vec<Option<FileDescriptor>>,
    /// Prochain fd à essayer en premier (hint, pas garanti).
    next_hint: usize,
    /// Compteur d'ouvertures cumulées.
    open_count: AtomicU64,
    /// Compteur de fermetures cumulées.
    close_count: AtomicU64,
}

impl OpenFileTable {
    /// Crée une table vide avec les 3 fds standards pré-réservés (mais vides).
    pub fn new(fd_limit: usize) -> Self {
        Self::try_new(fd_limit).expect("OpenFileTable::new: allocation échouée")
    }

    /// Variante fallible utilisée par les chemins noyau qui doivent rester OOM-safe.
    pub fn try_new(fd_limit: usize) -> Option<Self> {
        let mut descriptors = Vec::new();
        descriptors
            .try_reserve_exact(32.min(fd_limit).max(3))
            .ok()?;
        // Allouer slots 0,1,2 pour stdin/stdout/stderr (initialement None).
        descriptors.push(None); // stdin
        descriptors.push(None); // stdout
        descriptors.push(None); // stderr
        Some(Self {
            fd_limit,
            descriptors,
            next_hint: 3,
            open_count: AtomicU64::new(0),
            close_count: AtomicU64::new(0),
        })
    }

    /// Installe le triplet stdin/stdout/stderr.
    pub fn install_std_fds(&mut self, stdin: u64, stdout: u64, stderr: u64) {
        self.descriptors[0] = Some(FileDescriptor {
            fd: 0,
            handle: stdin,
            flags: 0,
        });
        self.descriptors[1] = Some(FileDescriptor {
            fd: 1,
            handle: stdout,
            flags: 1,
        });
        self.descriptors[2] = Some(FileDescriptor {
            fd: 2,
            handle: stderr,
            flags: 1,
        });
        self.open_count
            .store(self.open_fd_count() as u64, Ordering::Relaxed);
    }

    /// Installe ou remplace un fd précis par un handle opaque.
    pub fn install_at(&mut self, fd: i32, handle: u64, flags: u32) -> bool {
        if fd < 0 || fd as usize >= self.fd_limit {
            return false;
        }
        let idx = fd as usize;
        while self.descriptors.len() <= idx {
            if self.descriptors.try_reserve(1).is_err() {
                return false;
            }
            self.descriptors.push(None);
        }

        if self.descriptors[idx].is_none() {
            self.open_count.fetch_add(1, Ordering::Relaxed);
        }
        self.descriptors[idx] = Some(FileDescriptor { fd, handle, flags });
        if idx == self.next_hint {
            while self.next_hint < self.descriptors.len()
                && self.descriptors[self.next_hint].is_some()
            {
                self.next_hint += 1;
            }
        }
        true
    }

    /// Alloue le prochain fd disponible et y associe le handle.
    /// Retourne le numéro fd alloué ou -1 si la table est pleine.
    pub fn install(&mut self, handle: u64, flags: u32) -> i32 {
        let start = self.next_hint;
        let limit = self.fd_limit;

        // Scanner à partir du hint.
        for idx in start..limit {
            if idx < self.descriptors.len() {
                if self.descriptors[idx].is_none() {
                    self.descriptors[idx] = Some(FileDescriptor {
                        fd: idx as i32,
                        handle,
                        flags,
                    });
                    self.next_hint = idx + 1;
                    self.open_count.fetch_add(1, Ordering::Relaxed);
                    return idx as i32;
                }
            } else {
                // Étendre le vecteur.
                if self.descriptors.try_reserve(1).is_err() {
                    return -1;
                }
                self.descriptors.push(Some(FileDescriptor {
                    fd: idx as i32,
                    handle,
                    flags,
                }));
                self.next_hint = idx + 1;
                self.open_count.fetch_add(1, Ordering::Relaxed);
                return idx as i32;
            }
        }
        // Rescan depuis 0 au cas où il y a des trous avant start.
        for idx in 3..start {
            if idx < self.descriptors.len() && self.descriptors[idx].is_none() {
                self.descriptors[idx] = Some(FileDescriptor {
                    fd: idx as i32,
                    handle,
                    flags,
                });
                self.next_hint = idx + 1;
                self.open_count.fetch_add(1, Ordering::Relaxed);
                return idx as i32;
            }
        }
        -1 // EMFILE
    }

    /// Ferme le fd donné. Retourne le handle associé pour que fs/ puisse fermer le fichier.
    pub fn close(&mut self, fd: i32) -> Option<u64> {
        if fd < 0 || fd as usize >= self.descriptors.len() {
            return None;
        }
        let entry = self.descriptors[fd as usize].take()?;
        if (fd as usize) < self.next_hint {
            self.next_hint = fd as usize;
        }
        self.close_count.fetch_add(1, Ordering::Relaxed);
        Some(entry.handle)
    }

    /// Lit le handle associé à un fd (sans fermer).
    #[inline(always)]
    pub fn get(&self, fd: i32) -> Option<&FileDescriptor> {
        self.descriptors.get(fd as usize)?.as_ref()
    }

    /// Met à jour les flags de descripteur (FD_CLOEXEC, O_NONBLOCK miroir...).
    pub fn set_flags(&mut self, fd: i32, flags: u32) -> bool {
        let Some(Some(entry)) = self.descriptors.get_mut(fd as usize) else {
            return false;
        };
        entry.flags = flags;
        true
    }

    /// Ferme tous les fds marqués O_CLOEXEC (appelé lors de execve).
    #[allow(clippy::result_unit_err)]
    pub fn close_on_exec(&mut self) -> Result<Vec<u64>, ()> {
        const O_CLOEXEC: u32 = 0x80000;
        let mut closed_handles = Vec::new();
        let close_count = self
            .descriptors
            .iter()
            .filter(|slot| slot.as_ref().is_some_and(|fd| fd.flags & O_CLOEXEC != 0))
            .count();
        closed_handles
            .try_reserve_exact(close_count)
            .map_err(|_| ())?;
        for slot in &mut self.descriptors {
            if let Some(fd_entry) = slot {
                if fd_entry.flags & O_CLOEXEC != 0 {
                    closed_handles.push(fd_entry.handle);
                    *slot = None;
                    self.close_count.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        self.next_hint = 3;
        Ok(closed_handles)
    }

    /// Ferme tous les descripteurs ouverts du processus.
    #[allow(clippy::result_unit_err)]
    pub fn close_all(&mut self) -> Result<Vec<u64>, ()> {
        let mut closed_handles = Vec::new();
        let close_count = self
            .descriptors
            .iter()
            .filter(|slot| slot.is_some())
            .count();
        closed_handles
            .try_reserve_exact(close_count)
            .map_err(|_| ())?;
        for slot in &mut self.descriptors {
            if let Some(fd_entry) = slot.take() {
                closed_handles.push(fd_entry.handle);
                self.close_count.fetch_add(1, Ordering::Relaxed);
            }
        }
        self.next_hint = 3;
        Ok(closed_handles)
    }

    /// Ferme tous les descripteurs sans allocation, pour les chemins exit/reap.
    pub fn close_all_noalloc(&mut self) {
        for slot in &mut self.descriptors {
            if slot.take().is_some() {
                self.close_count.fetch_add(1, Ordering::Relaxed);
            }
        }
        self.next_hint = 3;
    }

    /// Clone la table pour fork() (les handles sont dupliqués).
    pub fn clone_for_fork(&self) -> Self {
        self.try_clone_for_fork()
            .expect("OpenFileTable::clone_for_fork: allocation échouée")
    }

    /// Clone fallible de la table pour fork().
    pub fn try_clone_for_fork(&self) -> Option<Self> {
        let mut descriptors = Vec::new();
        descriptors.try_reserve_exact(self.descriptors.len()).ok()?;
        let mut idx = 0usize;
        while idx < self.descriptors.len() {
            descriptors.push(self.descriptors[idx]);
            idx += 1;
        }
        Some(Self {
            fd_limit: self.fd_limit,
            descriptors,
            next_hint: self.next_hint,
            open_count: AtomicU64::new(self.open_count.load(Ordering::Relaxed)),
            close_count: AtomicU64::new(0),
        })
    }

    /// Nombre de fds ouverts actuellement.
    pub fn open_fd_count(&self) -> usize {
        self.descriptors.iter().filter(|s| s.is_some()).count()
    }
}
