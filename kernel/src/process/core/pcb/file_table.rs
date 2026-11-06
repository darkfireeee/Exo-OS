//! # process/core/pcb/file_table.rs — Per-process open file table
//!
//! Manages allocation and tracking of file descriptors for a process.
//! Opaque with respect to FS: descriptors hold integer handles without direct fs/ imports.

extern crate alloc;

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

/// A file descriptor: opaque (managed by fs/ via integer handle).
#[derive(Copy, Clone, Debug)]
pub struct FileDescriptor {
    /// File descriptor number (0=stdin, 1=stdout, 2=stderr, 3+...).
    pub fd: i32,
    /// Opaque handle to fs/ entry (index in vfs table).
    pub handle: u64,
    /// O_CLOEXEC, O_NONBLOCK flags...
    pub flags: u32,
}

/// Open file table of a process (shared between threads via fork+CLONE_FILES).
pub struct OpenFileTable {
    /// Per-process fd_limit (configurable via rlimit RLIMIT_NOFILE).
    fd_limit: usize,
    /// Active descriptors. Index = fd number.
    descriptors: Vec<Option<FileDescriptor>>,
    /// Next fd to try first (hint, not guaranteed).
    next_hint: usize,
    /// Cumulative open counter.
    open_count: AtomicU64,
    /// Cumulative close counter.
    close_count: AtomicU64,
}

impl OpenFileTable {
    /// Creates an empty table with standard fds (0, 1, 2) pre-reserved (but empty).
    pub fn new(fd_limit: usize) -> Self {
        Self::try_new(fd_limit).expect("OpenFileTable::new: allocation échouée")
    }

    /// Fallible variant used by kernel paths that must remain OOM-safe.
    pub fn try_new(fd_limit: usize) -> Option<Self> {
        let mut descriptors = Vec::new();
        descriptors
            .try_reserve_exact(32.min(fd_limit).max(3))
            .ok()?;
        // Allocate slots 0, 1, 2 for stdin/stdout/stderr (initially None).
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

    /// Installs the stdin/stdout/stderr triplet.
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

    /// Installs or replaces a specific fd with an opaque handle.
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

    /// Allocates the next available fd and associates the handle with it.
    /// Returns the allocated fd number, or -1 if the table is full.
    pub fn install(&mut self, handle: u64, flags: u32) -> i32 {
        let start = self.next_hint;
        let limit = self.fd_limit;

        // Scan starting from hint.
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
                // Extend vector.
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
        // Rescan from 0 in case there are gaps before start.
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

    /// Closes the given fd. Returns associated handle so fs/ can close the file.
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

    /// Reads handle associated with an fd (without closing).
    #[inline(always)]
    pub fn get(&self, fd: i32) -> Option<&FileDescriptor> {
        self.descriptors.get(fd as usize)?.as_ref()
    }

    /// Updates descriptor flags (FD_CLOEXEC, O_NONBLOCK mirror...).
    pub fn set_flags(&mut self, fd: i32, flags: u32) -> bool {
        let Some(Some(entry)) = self.descriptors.get_mut(fd as usize) else {
            return false;
        };
        entry.flags = flags;
        true
    }

    /// Closes all fds marked O_CLOEXEC (called during execve).
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

    /// Closes all open descriptors of the process.
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

    /// Closes all descriptors without allocation, for exit/reap paths.
    pub fn close_all_noalloc(&mut self) {
        for slot in &mut self.descriptors {
            if slot.take().is_some() {
                self.close_count.fetch_add(1, Ordering::Relaxed);
            }
        }
        self.next_hint = 3;
    }

    /// Clones table for fork() (handles are duplicated).
    pub fn clone_for_fork(&self) -> Self {
        self.try_clone_for_fork()
            .expect("OpenFileTable::clone_for_fork: allocation échouée")
    }

    /// Fallible clone of the table for fork().
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

    /// Number of currently open fds.
    pub fn open_fd_count(&self) -> usize {
        self.descriptors.iter().filter(|s| s.is_some()).count()
    }
}
