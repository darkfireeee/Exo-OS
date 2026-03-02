// path/namespace.rs — Namespaces de chemins pour containers
// Ring 0, no_std
//
// Chaque container a son propre namespace (root ObjectId différent)
// Implémente l'isolation entre containers

use crate::fs::exofs::core::{ObjectId, EpochId, ExofsError};
use crate::scheduler::sync::spinlock::SpinLock;
use alloc::vec::Vec;

/// Identifiant de namespace
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(transparent)]
pub struct NamespaceId(pub u32);

impl NamespaceId {
    /// Namespace root (noyau) — ObjectId de la racine globale
    pub const ROOT: Self = NamespaceId(0);
}

/// Namespace de chemins — définit la racine visible d'un groupe de processus
pub struct PathNamespace {
    pub id: NamespaceId,
    /// ObjectId de la racine de ce namespace
    pub root_oid: ObjectId,
    /// Epoch à laquelle ce namespace a été créé
    pub created_epoch: EpochId,
    /// Référence count
    pub ref_count: u32,
}

/// Registre global des namespaces
pub struct NamespaceRegistry {
    inner: SpinLock<NamespaceRegistryInner>,
}

struct NamespaceRegistryInner {
    namespaces: Vec<PathNamespace>,
    next_id: u32,
}

pub static NAMESPACE_REGISTRY: NamespaceRegistry = NamespaceRegistry {
    inner: SpinLock::new(NamespaceRegistryInner {
        namespaces: Vec::new(),
        next_id: 1,
    }),
};

impl NamespaceRegistry {
    /// Crée un nouveau namespace avec une racine donnée
    pub fn create(
        &self,
        root_oid: ObjectId,
        epoch: EpochId,
    ) -> Result<NamespaceId, ExofsError> {
        let mut guard = self.inner.lock();
        let id = NamespaceId(guard.next_id);
        guard.next_id = guard.next_id.checked_add(1)
            .ok_or(ExofsError::OffsetOverflow)?;
        guard.namespaces.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        guard.namespaces.push(PathNamespace {
            id,
            root_oid,
            created_epoch: epoch,
            ref_count: 1,
        });
        Ok(id)
    }

    /// Retourne l'ObjectId racine d'un namespace
    pub fn get_root(&self, id: NamespaceId) -> Option<ObjectId> {
        let guard = self.inner.lock();
        for ns in &guard.namespaces {
            if ns.id == id {
                return Some(ns.root_oid);
            }
        }
        None
    }

    /// Incrémente le ref_count d'un namespace
    pub fn acquire(&self, id: NamespaceId) -> Result<(), ExofsError> {
        let mut guard = self.inner.lock();
        guard.namespaces.iter_mut()
            .find(|ns| ns.id == id)
            .map(|ns| ns.ref_count += 1)
            .ok_or(ExofsError::ObjectNotFound)
    }

    /// Décrémente le ref_count — supprime si 0
    pub fn release(&self, id: NamespaceId) {
        let mut guard = self.inner.lock();
        if let Some(pos) = guard.namespaces.iter().position(|ns| ns.id == id) {
            guard.namespaces[pos].ref_count -= 1;
            if guard.namespaces[pos].ref_count == 0 {
                guard.namespaces.remove(pos);
            }
        }
    }
}
