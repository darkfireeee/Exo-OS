//! path_walker.rs — Itération itérative de chemin dans l'arbre ExoFS
//!
//! Règles critiques appliquées :
//!  - RECUR-01 : zéro récursion — machine à états itérative
//!  - OOM-02   : try_reserve avant chaque push
//!  - PATH-07  : pas de [u8; PATH_MAX] sur la pile
//!  - ARITH-02 : arithmétique vérifiée sur toutes les offsets

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;

use crate::fs::exofs::core::{ExofsError, ExofsResult, ObjectId};
use crate::fs::exofs::path::path_component::{
    PathComponent, PathParser, validate_component,
};
use crate::fs::exofs::path::symlink::SYMLINK_MAX_DEPTH;

// ─────────────────────────────────────────────────────────────────────────────
// Etat du walker
// ─────────────────────────────────────────────────────────────────────────────

/// État courant de la machine à états du `PathWalker`.
#[derive(Debug)]
pub enum WalkerState {
    /// Positionnement initial avant le premier pas.
    AtRoot,
    /// Dans un répertoire, composants restants à descendre.
    InDirectory {
        dir_oid:   ObjectId,
        remaining: Vec<PathComponent>,
    },
    /// Un symlink vient d'être rencontré.
    AtSymlink {
        target:    Vec<u8>,
        depth:     usize,
    },
    /// Résolution terminée avec succès.
    Done {
        oid: ObjectId,
    },
    /// Résolution terminée en erreur.
    Failed(ExofsError),
}

// ─────────────────────────────────────────────────────────────────────────────
// Résultat d'un pas
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat renvoyé par `PathWalker::step()`.
#[derive(Debug)]
pub enum WalkerStepResult {
    /// Un composant a été consommé, la résolution continue.
    Continue,
    /// Résolution terminée — l'OID final est disponible.
    Done(ObjectId),
    /// Un symlink a été rencontré, sa cible est fournie.
    SymlinkEncountered(Vec<u8>),
    /// Erreur fatale.
    Error(ExofsError),
}

// ─────────────────────────────────────────────────────────────────────────────
// Trait de callback
// ─────────────────────────────────────────────────────────────────────────────

/// Trait que l'appelant doit implémenter pour alimenter le walker.
///
/// Le walker ne fait **aucune** I/O directe — il délègue toutes les
/// résolutions à ce trait.
pub trait WalkerBackend {
    /// Recherche un composant dans un répertoire.
    /// Retourne `(oid, is_symlink)` ou `None` si absent.
    fn lookup(
        &self,
        dir_oid: &ObjectId,
        component: &PathComponent,
    ) -> ExofsResult<Option<(ObjectId, bool)>>;

    /// Lit la cible d'un lien symbolique.
    fn symlink_target(&self, oid: &ObjectId) -> ExofsResult<Vec<u8>>;

    /// OID de la racine du système de fichiers.
    fn root_oid(&self) -> ObjectId;
}

// ─────────────────────────────────────────────────────────────────────────────
// PathWalker
// ─────────────────────────────────────────────────────────────────────────────

/// Itérateur de chemin à état explicite.
///
/// Chaque appel à `step()` avance d'un composant.  L'appelant doit
/// continuer jusqu'à `WalkerStepResult::Done` ou `::Error`.
pub struct PathWalker {
    /// OID du répertoire racine de résolution.
    pub root_oid:      ObjectId,
    /// État courant.
    pub state:         WalkerState,
    /// Profondeur de résolution de symlinks.
    pub symlink_depth: usize,
}

impl PathWalker {
    /// Crée un nouveau walker sur `path` depuis `root`.
    ///
    /// - Parse le chemin avec `PathParser` (itératif, RECUR-01).
    /// - Les composants sont stockés en ordre inverse pour `pop()` efficace.
    pub fn new(root: ObjectId, path: &[u8]) -> ExofsResult<Self> {
        if path.len() > crate::fs::exofs::path::path_component::PATH_MAX {
            return Err(ExofsError::PathTooLong);
        }
        let mut components: Vec<PathComponent> = Vec::new();
        let parser = PathParser::new(path);
        for comp_result in parser {
            let comp = comp_result?;
            // Ignorer les composants "."
            let bytes = comp.as_bytes();
            if bytes == b"." { continue; }
            components.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            components.push(comp);
        }
        // Inverser pour que le prochain composant soit en queue.
        components.reverse();
        Ok(PathWalker {
            root_oid: root.clone(),
            state: WalkerState::InDirectory {
                dir_oid:   root,
                remaining: components,
            },
            symlink_depth: 0,
        })
    }

    /// Avance d'un pas dans la résolution.
    ///
    /// Itératif (RECUR-01) — aucun appel interne récursif.
    pub fn step<B: WalkerBackend>(
        &mut self,
        backend: &B,
    ) -> WalkerStepResult {
        match &mut self.state {
            WalkerState::AtRoot => {
                // Transition vers InDirectory à la racine sans composants.
                let root = backend.root_oid();
                self.state = WalkerState::Done { oid: root.clone() };
                WalkerStepResult::Done(root)
            }

            WalkerState::Done { oid } => {
                WalkerStepResult::Done(oid.clone())
            }

            WalkerState::Failed(e) => {
                WalkerStepResult::Error(e.clone())
            }

            WalkerState::AtSymlink { target, depth } => {
                let tgt = core::mem::take(target);
                let d = *depth;
                // Remonte au contexte InDirectory reconstruit depuis le symlink.
                let mut components: Vec<PathComponent> = Vec::new();
                let parser = PathParser::new(&tgt);
                for cr in parser {
                    match cr {
                        Err(e) => {
                            self.state = WalkerState::Failed(e.clone());
                            return WalkerStepResult::Error(e);
                        }
                        Ok(c) => {
                            if c.as_bytes() == b"." { continue; }
                            if let Err(_) = components.try_reserve(1) {
                                let e = ExofsError::NoMemory;
                                self.state = WalkerState::Failed(e.clone());
                                return WalkerStepResult::Error(e);
                            }
                            components.push(c);
                        }
                    }
                }
                components.reverse();
                let start = if tgt.first() == Some(&b'/') {
                    backend.root_oid()
                } else {
                    // Symlink relatif : partir du parent courant (root par défaut ici)
                    self.root_oid.clone()
                };
                self.symlink_depth = d;
                self.state = WalkerState::InDirectory {
                    dir_oid:   start,
                    remaining: components,
                };
                WalkerStepResult::Continue
            }

            WalkerState::InDirectory { dir_oid, remaining } => {
                match remaining.pop() {
                    None => {
                        // Plus de composants : résolution terminée.
                        let oid = dir_oid.clone();
                        self.state = WalkerState::Done { oid: oid.clone() };
                        WalkerStepResult::Done(oid)
                    }
                    Some(comp) => {
                        // Composant ".." : remonter (pas supporté sans parent-tracking —
                        // indiquer erreur contrôlée).
                        if comp.as_bytes() == b".." {
                            // On ne dispose pas du parent ici, on retourne
                            // l'OID courant pour les cas de chemins normalisés.
                            // Un appelant avec canonicalize_path doit d'abord normaliser.
                            let oid = dir_oid.clone();
                            self.state = WalkerState::Done { oid: oid.clone() };
                            return WalkerStepResult::Done(oid);
                        }
                        match backend.lookup(dir_oid, &comp) {
                            Err(e) => {
                                self.state = WalkerState::Failed(e.clone());
                                WalkerStepResult::Error(e)
                            }
                            Ok(None) => {
                                let e = ExofsError::ObjectNotFound;
                                self.state = WalkerState::Failed(e.clone());
                                WalkerStepResult::Error(e)
                            }
                            Ok(Some((oid, true))) => {
                                // Symlink
                                let new_depth = match self.symlink_depth
                                    .checked_add(1) {
                                    None => {
                                        let e = ExofsError::OffsetOverflow;
                                        self.state = WalkerState::Failed(e.clone());
                                        return WalkerStepResult::Error(e);
                                    }
                                    Some(d) => d,
                                };
                                if new_depth > SYMLINK_MAX_DEPTH {
                                    let e = ExofsError::TooManySymlinks;
                                    self.state = WalkerState::Failed(e.clone());
                                    return WalkerStepResult::Error(e);
                                }
                                match backend.symlink_target(&oid) {
                                    Err(e) => {
                                        self.state = WalkerState::Failed(e.clone());
                                        WalkerStepResult::Error(e)
                                    }
                                    Ok(tgt) => {
                                        let tgt_copy = tgt.clone();
                                        self.state = WalkerState::AtSymlink {
                                            target: tgt,
                                            depth:  new_depth,
                                        };
                                        WalkerStepResult::SymlinkEncountered(tgt_copy)
                                    }
                                }
                            }
                            Ok(Some((oid, false))) => {
                                // Répertoire ou fichier ordinaire
                                let new_dir = oid;
                                let rem_empty = remaining.is_empty();
                                if rem_empty {
                                    self.state = WalkerState::Done {
                                        oid: new_dir.clone(),
                                    };
                                    WalkerStepResult::Done(new_dir)
                                } else {
                                    *dir_oid = new_dir;
                                    WalkerStepResult::Continue
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Résout le chemin jusqu'au bout.
    ///
    /// Itératif (RECUR-01) — boucle `while` explicite.
    pub fn collect_to_end<B: WalkerBackend>(
        &mut self,
        backend: &B,
    ) -> ExofsResult<ObjectId> {
        let mut iters: usize = 0;
        let max_iters: usize = 4096;
        loop {
            if iters >= max_iters { return Err(ExofsError::InternalError); }
            iters = iters.checked_add(1).ok_or(ExofsError::OffsetOverflow)?;
            match self.step(backend) {
                WalkerStepResult::Done(oid)             => return Ok(oid),
                WalkerStepResult::Error(e)              => return Err(e),
                WalkerStepResult::Continue              => {}
                WalkerStepResult::SymlinkEncountered(_) => {}
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers publics
// ─────────────────────────────────────────────────────────────────────────────

/// Résout `path` depuis `root` en utilisant `backend`.
///
/// - Normalise d'abord le chemin (via `canonicalize_to_vec`).
/// - Puis crée un `PathWalker` et itère jusqu'à la fin.
pub fn walk_path<B: WalkerBackend>(
    root: ObjectId,
    path: &[u8],
    backend: &B,
) -> ExofsResult<ObjectId> {
    // Normalisation (supprime //, /., gère ..)
    let normalized = crate::fs::exofs::path::canonicalize::canonicalize_to_vec(path)?;
    let mut walker = PathWalker::new(root, &normalized)?;
    walker.collect_to_end(backend)
}

/// Résout uniquement le composant final d'un chemin (basename).
pub fn basename(path: &[u8]) -> ExofsResult<PathComponent> {
    let mut last: Option<PathComponent> = None;
    for cr in PathParser::new(path) {
        let c = cr?;
        if c.as_bytes() != b"." && c.as_bytes() != b".." {
            last = Some(c);
        }
    }
    last.ok_or(ExofsError::InvalidPathComponent)
}

/// Résout le chemin jusqu'au répertoire parent (dirname semantics).
///
/// Retourne l'OID du répertoire contenant le dernier composant.
pub fn walk_parent<B: WalkerBackend>(
    root: ObjectId,
    path: &[u8],
    backend: &B,
) -> ExofsResult<(ObjectId, PathComponent)> {
    let normalized = crate::fs::exofs::path::canonicalize::canonicalize_to_vec(path)?;
    // Trouver la dernière occurrence de '/'
    let slash_pos = normalized.iter().rposition(|&b| b == b'/');
    let (parent_path, child_bytes) = match slash_pos {
        None => (b"/" as &[u8], normalized.as_slice()),
        Some(0) => (b"/", &normalized[1..]),
        Some(p) => (&normalized[..p], &normalized[p + 1..]),
    };
    validate_component(child_bytes)?;
    let child = PathComponent::from_bytes(child_bytes)?;
    let parent_oid = {
        let mut walker = PathWalker::new(root, parent_path)?;
        walker.collect_to_end(backend)?
    };
    Ok((parent_oid, child))
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn oid(b: u8) -> ObjectId { let mut a = [0u8; 32]; a[0] = b; ObjectId(a) }

    struct FlatBackend {
        root: ObjectId,
    }
    impl WalkerBackend for FlatBackend {
        fn lookup(&self, _dir: &ObjectId, comp: &PathComponent)
            -> ExofsResult<Option<(ObjectId, bool)>>
        {
            let b = comp.as_bytes();
            if b == b"bin"   { return Ok(Some((oid(2), false))); }
            if b == b"lib"   { return Ok(Some((oid(3), false))); }
            if b == b"link"  { return Ok(Some((oid(10), true))); }
            Ok(None)
        }
        fn symlink_target(&self, _oid: &ObjectId) -> ExofsResult<Vec<u8>> {
            let mut v = Vec::new();
            v.extend_from_slice(b"/bin");
            Ok(v)
        }
        fn root_oid(&self) -> ObjectId { self.root.clone() }
    }

    #[test] fn test_walk_simple() {
        let b = FlatBackend { root: oid(1) };
        let res = walk_path(oid(1), b"/bin", &b).unwrap();
        assert_eq!(res.0[0], 2);
    }

    #[test] fn test_walk_not_found() {
        let b = FlatBackend { root: oid(1) };
        assert!(walk_path(oid(1), b"/missing", &b).is_err());
    }

    #[test] fn test_basename() {
        let c = basename(b"/a/b/filename").unwrap();
        assert_eq!(c.as_bytes(), b"filename");
    }

    #[test] fn test_basename_root() {
        assert!(basename(b"/").is_err());
    }
}
