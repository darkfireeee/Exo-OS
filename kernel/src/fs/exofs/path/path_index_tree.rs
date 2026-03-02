// path/path_index_tree.rs — Radix tree in-memory pour PathIndex
// Ring 0, no_std
//
// Structure : HashMap-like basée sur BTreeMap kernel-safe
// Lookup O(log n) par hash

use crate::fs::exofs::core::{ObjectId, ExofsError};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

/// Radix tree in-memory pour le PathIndex
/// Clé : hash FNV-1a du nom
/// Valeur : ObjectId
///
/// En cas de collision (très rare) : liste de candidats vérifiés par comparaison
pub struct PathIndexTree {
    /// Map hash → liste d'ObjectId (pour gestion collisions)
    inner: BTreeMap<u64, SmallVec>,
}

/// Vecteur inline pour éviter les allocations heap sur cas courant (0-2 éléments)
struct SmallVec {
    /// Première entrée inline (cas dominant)
    first: Option<ObjectId>,
    /// Entrées supplémentaires (collisions — rarissimes)
    overflow: Option<Vec<ObjectId>>,
}

impl SmallVec {
    fn new(oid: ObjectId) -> Self {
        SmallVec { first: Some(oid), overflow: None }
    }

    fn push(&mut self, oid: ObjectId) -> Result<(), ExofsError> {
        if self.first.is_none() {
            self.first = Some(oid);
        } else {
            let ov = self.overflow.get_or_insert_with(Vec::new);
            ov.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            ov.push(oid);
        }
        Ok(())
    }

    fn remove(&mut self, oid: ObjectId) {
        if self.first == Some(oid) {
            if let Some(ov) = &mut self.overflow {
                if !ov.is_empty() {
                    self.first = Some(ov.remove(0));
                    return;
                }
            }
            self.first = None;
        } else if let Some(ov) = &mut self.overflow {
            ov.retain(|o| !o.ct_eq(&oid));
        }
    }

    fn is_empty(&self) -> bool {
        self.first.is_none()
    }
}

impl PathIndexTree {
    pub fn new() -> Self {
        PathIndexTree {
            inner: BTreeMap::new(),
        }
    }

    /// Insère un hash → ObjectId
    pub fn insert(&mut self, hash: u64, oid: ObjectId) {
        self.inner
            .entry(hash)
            .and_modify(|v| { let _ = v.push(oid); })
            .or_insert_with(|| SmallVec::new(oid));
    }

    /// Trouve l'ObjectId correspondant à un hash + nom exact
    /// (pour lever les ambiguïtés de collision)
    pub fn find(
        &self,
        hash: u64,
        name: &[u8],
        entries: &[(u64, ObjectId, Vec<u8>)],
    ) -> Option<ObjectId> {
        // Vérifie d'abord que le hash existe
        if self.inner.get(&hash).is_none() {
            return None;
        }
        // Recherche linéaire sur les entrées (NAME_MAX comparaisons)
        // Garanti rapide car le hash filtre 99.99% des cas
        for (ehash, eoid, ename) in entries {
            if *ehash == hash && ename.as_slice() == name {
                return Some(*eoid);
            }
        }
        None
    }

    /// Supprime un hash du tree
    pub fn remove(&mut self, hash: u64) {
        if let Some(sv) = self.inner.get_mut(&hash) {
            if sv.is_empty() {
                self.inner.remove(&hash);
            }
        }
    }

    /// Nombre de hashes distincts dans le tree
    #[inline]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}
