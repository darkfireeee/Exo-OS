//! # fs/exofs/syscall/object_fd/table_inner.rs — Données internes de la table de descripteurs
//!
//! Tableau plat de slots et opérations élémentaires sans allocation dynamique.
//! Conforme à RECUR-01 (boucles `while` uniquement) et ARITH-02 (`saturating_*`/`wrapping_*`).

use crate::fs::exofs::core::types::BlobId;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use super::entry::{ObjectFdEntry, FD_MAX, FD_RESERVED, MAX_FDS};
use super::flags as open_flags;

pub struct ObjectFdTableInner {
    /// Tableau plat de slots (remplace BTreeMap).
    pub slots: [ObjectFdEntry; MAX_FDS],
    /// Nombre de fds ouverts.
    pub open_count: u32,
}

impl ObjectFdTableInner {
    pub const fn new() -> Self {
        Self {
            slots: [const { ObjectFdEntry::empty() }; MAX_FDS],
            open_count: 0,
        }
    }

    /// Trouve un slot libre (RECUR-01 : while).
    pub fn find_free_slot(&self) -> Option<usize> {
        let mut i = 0usize;
        while i < MAX_FDS {
            if self.slots[i].is_free() {
                return Some(i);
            }
            i = i.wrapping_add(1);
        }
        None
    }

    /// Trouve un slot libre à partir d'un fd minimal.
    pub fn find_free_slot_from(&self, min_fd: u32) -> Option<usize> {
        let start_fd = min_fd.max(FD_RESERVED);
        let mut idx = (start_fd - FD_RESERVED) as usize;
        while idx < MAX_FDS {
            if self.slots[idx].is_free() {
                return Some(idx);
            }
            idx = idx.wrapping_add(1);
        }
        None
    }

    /// Trouve le slot correspondant à un fd (RECUR-01 : while).
    pub fn slot_of_fd(&self, fd: u32) -> Option<usize> {
        if fd < FD_RESERVED || fd > FD_MAX {
            return None;
        }
        let idx = (fd - FD_RESERVED) as usize;
        if idx < MAX_FDS && !self.slots[idx].is_free() {
            Some(idx)
        } else {
            None
        }
    }

    pub fn canonical_slot_of_index(&self, idx: usize) -> ExofsResult<usize> {
        let entry = self.slots[idx];
        if entry.is_free() {
            return Err(ExofsError::ObjectNotFound);
        }
        if entry.is_canonical() {
            return Ok(idx);
        }
        self.slot_of_fd(entry.shared_fd)
            .ok_or(ExofsError::ObjectNotFound)
    }

    pub fn canonical_slot_of_fd(&self, fd: u32) -> ExofsResult<usize> {
        let idx = self.slot_of_fd(fd).ok_or(ExofsError::ObjectNotFound)?;
        self.canonical_slot_of_index(idx)
    }

    /// Ouvre un nouveau fd→BlobId.
    pub fn open_slot(
        &mut self,
        blob_id: BlobId,
        flags: u32,
        size: u64,
        epoch_id: u64,
        owner_uid: u64,
        _next_fd: u32,
    ) -> ExofsResult<u32> {
        let idx = self.find_free_slot().ok_or(ExofsError::NoSpace)?;
        let fd = FD_RESERVED.saturating_add(idx as u32);
        self.slots[idx] = ObjectFdEntry {
            fd,
            shared_fd: fd,
            blob_id,
            flags,
            cursor: 0,
            size,
            ref_count: 1,
            epoch_id,
            owner_uid,
        };
        self.open_count = self.open_count.saturating_add(1);
        Ok(fd)
    }

    /// Ferme un fd (retourne true si trouvé et fermé).
    pub fn close_slot(&mut self, fd: u32) -> bool {
        let idx = match self.slot_of_fd(fd) {
            Some(idx) => idx,
            None => return false,
        };

        if !self.slots[idx].is_canonical() {
            if let Some(canon_idx) = self.slot_of_fd(self.slots[idx].shared_fd) {
                let rc = self.slots[canon_idx].ref_count;
                self.slots[canon_idx].ref_count = rc.saturating_sub(1);
            }
            self.slots[idx] = ObjectFdEntry::empty();
            self.open_count = self.open_count.saturating_sub(1);
            return true;
        }

        let rc = self.slots[idx].ref_count;
        if rc <= 1 {
            self.slots[idx] = ObjectFdEntry::empty();
            self.open_count = self.open_count.saturating_sub(1);
            return true;
        }

        let mut promote_idx = None;
        let mut i = 0usize;
        while i < MAX_FDS {
            if i != idx && !self.slots[i].is_free() && self.slots[i].shared_fd == fd {
                promote_idx = Some(i);
                break;
            }
            i = i.wrapping_add(1);
        }

        if let Some(new_idx) = promote_idx {
            let old_entry = self.slots[idx];
            let new_fd = self.slots[new_idx].fd;
            let mut replacement = old_entry;
            replacement.fd = new_fd;
            replacement.shared_fd = new_fd;
            replacement.ref_count = rc.saturating_sub(1);
            self.slots[new_idx] = replacement;

            let mut j = 0usize;
            while j < MAX_FDS {
                if j != idx
                    && j != new_idx
                    && !self.slots[j].is_free()
                    && self.slots[j].shared_fd == fd
                {
                    self.slots[j].shared_fd = new_fd;
                }
                j = j.wrapping_add(1);
            }

            self.slots[idx] = ObjectFdEntry::empty();
            self.open_count = self.open_count.saturating_sub(1);
            return true;
        }

        self.slots[idx].ref_count = rc.saturating_sub(1);
        true
    }

    /// Lit l'entrée d'un fd (clonée).
    pub fn get_entry(&self, fd: u32) -> ExofsResult<ObjectFdEntry> {
        let canon_idx = self.canonical_slot_of_fd(fd)?;
        let mut entry = self.slots[canon_idx];
        entry.fd = fd;
        Ok(entry)
    }

    /// Met à jour le curseur d'un fd.
    pub fn set_cursor(&mut self, fd: u32, cursor: u64) -> ExofsResult<()> {
        let idx = self.canonical_slot_of_fd(fd)?;
        self.slots[idx].cursor = cursor;
        Ok(())
    }

    /// Met à jour la taille connue d'un fd.
    pub fn set_size(&mut self, fd: u32, size: u64) -> ExofsResult<()> {
        let idx = self.canonical_slot_of_fd(fd)?;
        self.slots[idx].size = size;
        Ok(())
    }

    /// Met à jour les flags d'état d'un fd canonique.
    pub fn set_status_flags(&mut self, fd: u32, flags: u32) -> ExofsResult<u32> {
        let idx = self.canonical_slot_of_fd(fd)?;
        let access_mode = self.slots[idx].flags & 0x0003;
        let mutable_mask = open_flags::O_APPEND;
        let retained = self.slots[idx].flags & !(mutable_mask | 0x0003);
        let updated = retained | access_mode | (flags & mutable_mask);
        self.slots[idx].flags = updated;
        Ok(updated)
    }

    /// Duplique un fd (nouveau descripteur partageant le même état).
    pub fn dup_fd(&mut self, fd: u32) -> ExofsResult<u32> {
        let canon_idx = self.canonical_slot_of_fd(fd)?;
        let new_idx = self.find_free_slot().ok_or(ExofsError::NoSpace)?;
        let new_fd = FD_RESERVED.saturating_add(new_idx as u32);

        let mut duplicate = self.slots[canon_idx];
        duplicate.fd = new_fd;
        duplicate.shared_fd = self.slots[canon_idx].fd;
        duplicate.ref_count = 0;
        self.slots[new_idx] = duplicate;
        self.slots[canon_idx].ref_count = self.slots[canon_idx].ref_count.saturating_add(1);
        self.open_count = self.open_count.saturating_add(1);
        Ok(new_fd)
    }

    /// Duplique un fd dans un numéro précis.
    pub fn dup2_fd(&mut self, oldfd: u32, newfd: u32) -> ExofsResult<u32> {
        if newfd < FD_RESERVED || newfd > FD_MAX {
            return Err(ExofsError::InvalidArgument);
        }
        if oldfd == newfd {
            self.canonical_slot_of_fd(oldfd)?;
            return Ok(newfd);
        }

        let canon_idx = self.canonical_slot_of_fd(oldfd)?;
        if let Some(existing_idx) = self.slot_of_fd(newfd) {
            let existing_canon = self.canonical_slot_of_index(existing_idx)?;
            if existing_canon == canon_idx {
                return Ok(newfd);
            }
            let _ = self.close_slot(newfd);
        }

        let new_idx = (newfd - FD_RESERVED) as usize;
        if !self.slots[new_idx].is_free() {
            return Err(ExofsError::NoSpace);
        }

        let mut duplicate = self.slots[canon_idx];
        duplicate.fd = newfd;
        duplicate.shared_fd = self.slots[canon_idx].fd;
        duplicate.ref_count = 0;
        self.slots[new_idx] = duplicate;
        self.slots[canon_idx].ref_count = self.slots[canon_idx].ref_count.saturating_add(1);
        self.open_count = self.open_count.saturating_add(1);
        Ok(newfd)
    }

    /// Duplique un fd en choisissant le premier numéro >= `min_fd`.
    pub fn dup_from_fd(&mut self, fd: u32, min_fd: u32) -> ExofsResult<u32> {
        let slot = self
            .find_free_slot_from(min_fd)
            .ok_or(ExofsError::NoSpace)?;
        self.dup2_fd(fd, FD_RESERVED.saturating_add(slot as u32))
    }

    /// Retourne le nombre de fds ouverts.
    #[inline]
    pub fn open_count(&self) -> u32 {
        self.open_count
    }
}
