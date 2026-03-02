//! InodeEmulation — mapping ObjectId ↔ ino_t pour le VFS existant (no_std).

use alloc::collections::BTreeMap;
use crate::scheduler::sync::spinlock::SpinLock;
use crate::fs::exofs::core::FsError;

/// Type ino_t compatible POSIX.
pub type ObjectIno = u64;

/// Mapping bidirectionnel ObjectId (u64) ↔ ino_t.
pub struct InodeEmulation {
    obj_to_ino: SpinLock<BTreeMap<u64, ObjectIno>>,
    ino_to_obj: SpinLock<BTreeMap<ObjectIno, u64>>,
    next_ino:   core::sync::atomic::AtomicU64,
}

pub static INODE_EMULATION: InodeEmulation = InodeEmulation::new_const();

impl InodeEmulation {
    pub const fn new_const() -> Self {
        Self {
            obj_to_ino: SpinLock::new(BTreeMap::new()),
            ino_to_obj: SpinLock::new(BTreeMap::new()),
            next_ino:   core::sync::atomic::AtomicU64::new(2), // 1 est root
        }
    }

    /// Retourne ou alloue un ino_t stable pour un object_id.
    pub fn get_or_alloc(&self, object_id: u64) -> Result<ObjectIno, FsError> {
        {
            if let Some(&ino) = self.obj_to_ino.lock().get(&object_id) {
                return Ok(ino);
            }
        }
        let ino = self.next_ino.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        let mut forward = self.obj_to_ino.lock();
        forward.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        forward.insert(object_id, ino);
        drop(forward);

        let mut reverse = self.ino_to_obj.lock();
        reverse.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        reverse.insert(ino, object_id);
        Ok(ino)
    }

    pub fn ino_to_object(&self, ino: ObjectIno) -> Option<u64> {
        self.ino_to_obj.lock().get(&ino).copied()
    }

    pub fn release(&self, object_id: u64) {
        let mut forward = self.obj_to_ino.lock();
        if let Some(ino) = forward.remove(&object_id) {
            self.ino_to_obj.lock().remove(&ino);
        }
    }
}
