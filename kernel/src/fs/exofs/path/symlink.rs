//! symlink.rs -- Gestion des liens symboliques ExoFS.
//!
//! [PathSymlink] represente la cible d un lien symbolique (chemin en octets).
//! [SymlinkStore] est une table statique de cibles (ring buffer de 128 entrees).
//! La resolution iterative des symlinks respecte SYMLINK_MAX_DEPTH (40 niveaux).
//!
//! ## Regles spec
//! - **RECUR-01** : resolution iterative, jamais recursive.
//! - **OOM-02**   : try_reserve(1) avant push.
//! - **ARITH-02** : checked_add sur tous les calculs d offset.

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;

use crate::fs::exofs::core::{ExofsError, ExofsResult, ObjectId};
use crate::scheduler::sync::spinlock::SpinLock;
use super::path_component::{PathComponent, PathParser, NAME_MAX};

// -- Constantes ---------------------------------------------------------------

/// Profondeur maximale de symlinks suivis avant detection de boucle.
pub const SYMLINK_MAX_DEPTH: usize = 40;
/// Longueur maximale d une cible de symlink.
pub const SYMLINK_TARGET_MAX: usize = 4096;
/// Capacite du store (ring buffer, puissance de 2).
pub const STORE_SIZE: usize = 128;
const STORE_MASK: usize = STORE_SIZE - 1;

// -- SymlinkTarget ------------------------------------------------------------

/// Cible d un lien symbolique : chemin brut en octets (max 4096).
///
/// Stockage inline : pas d allocation heap par cible.
#[derive(Clone)]
pub struct SymlinkTarget {
    bytes: [u8; SYMLINK_TARGET_MAX],
    len:   u16,
    /// OID de l objet symlink.
    pub oid: ObjectId,
    /// `true` si la cible est un chemin absolu.
    pub is_absolute: bool,
}

impl SymlinkTarget {
    /// Cree une cible de symlink depuis un tableau d octets.
    pub fn new(raw: &[u8], oid: ObjectId) -> ExofsResult<Self> {
        if raw.is_empty() { return Err(ExofsError::InvalidPathComponent); }
        if raw.len() > SYMLINK_TARGET_MAX { return Err(ExofsError::PathTooLong); }
        if raw.contains(&0u8) { return Err(ExofsError::InvalidPathComponent); }
        let is_absolute = raw.first() == Some(&b'/');
        let mut storage = [0u8; SYMLINK_TARGET_MAX];
        storage[..raw.len()].copy_from_slice(raw);
        Ok(SymlinkTarget { bytes: storage, len: raw.len() as u16, oid, is_absolute })
    }

    /// Retourne les octets de la cible.
    pub fn as_bytes(&self) -> &[u8] { &self.bytes[..self.len as usize] }
    /// Longueur de la cible.
    pub fn len(&self) -> usize { self.len as usize }
    /// `true` si la cible est vide (ne devrait pas arriver).
    pub fn is_empty(&self) -> bool { self.len == 0 }
}

impl core::fmt::Debug for SymlinkTarget {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "SymlinkTarget({:?})",
            core::str::from_utf8(self.as_bytes()).unwrap_or("?"))
    }
}

// -- SymlinkStoreEntry --------------------------------------------------------

struct SymlinkStoreEntry {
    oid:   ObjectId,
    target: [u8; SYMLINK_TARGET_MAX],
    len:    u16,
    valid:  bool,
}

impl SymlinkStoreEntry {
    const fn empty() -> Self {
        SymlinkStoreEntry {
            oid:    ObjectId::INVALID,
            target: [0u8; SYMLINK_TARGET_MAX],
            len:    0,
            valid:  false,
        }
    }

    fn target_bytes(&self) -> &[u8] { &self.target[..self.len as usize] }
}

// -- SymlinkStoreInner --------------------------------------------------------

struct SymlinkStoreInner {
    entries: [SymlinkStoreEntry; STORE_SIZE],
    head:    usize,
    count:   usize,
}

impl SymlinkStoreInner {
    const fn new() -> Self {
        SymlinkStoreInner {
            entries: [const { SymlinkStoreEntry::empty() }; STORE_SIZE],
            head:    0,
            count:   0,
        }
    }

    fn store(&mut self, oid: &ObjectId, target: &[u8]) -> ExofsResult<()> {
        if target.len() > SYMLINK_TARGET_MAX { return Err(ExofsError::PathTooLong); }
        let slot = self.head & STORE_MASK;
        let e = &mut self.entries[slot];
        e.oid   = oid.clone();
        e.len   = target.len() as u16;
        e.target[..target.len()].copy_from_slice(target);
        e.valid = true;
        self.head = self.head.wrapping_add(1);
        if self.count < STORE_SIZE { self.count = self.count.saturating_add(1); }
        Ok(())
    }

    fn lookup(&self, oid: &ObjectId) -> Option<&[u8]> {
        for e in &self.entries {
            if e.valid && e.oid.as_bytes() == oid.as_bytes() {
                return Some(e.target_bytes());
            }
        }
        None
    }

    fn invalidate(&mut self, oid: &ObjectId) {
        for e in &mut self.entries {
            if e.valid && e.oid.as_bytes() == oid.as_bytes() {
                e.valid = false;
            }
        }
    }

    fn flush(&mut self) {
        for e in &mut self.entries { e.valid = false; }
        self.count = 0;
    }

    fn count(&self) -> usize { self.count }
}

// -- SymlinkStore -------------------------------------------------------------

/// Store de cibles de symlinks (thread-safe, ring buffer).
pub struct SymlinkStore {
    inner: SpinLock<SymlinkStoreInner>,
}

impl SymlinkStore {
    pub const fn new_const() -> Self {
        SymlinkStore { inner: SpinLock::new(SymlinkStoreInner::new()) }
    }

    /// Enregistre la cible d un symlink.
    pub fn store(&self, oid: &ObjectId, target: &[u8]) -> ExofsResult<()> {
        self.inner.lock().store(oid, target)
    }

    /// Recherche la cible d un OID symlink. Retourne une copie.
    pub fn lookup(&self, oid: &ObjectId) -> Option<Vec<u8>> {
        let g = self.inner.lock();
        g.lookup(oid).map(|b| {
            let mut v = Vec::new();
            if v.try_reserve(b.len()).is_ok() { v.extend_from_slice(b); }
            v
        })
    }

    /// Invalide l entree pour un OID.
    pub fn invalidate(&self, oid: &ObjectId) { self.inner.lock().invalidate(oid); }
    /// Vide le store.
    pub fn flush(&self) { self.inner.lock().flush(); }
    /// Nombre d entrees valides.
    pub fn count(&self) -> usize { self.inner.lock().count() }
}

/// Store global des symlinks.
pub static SYMLINK_STORE: SymlinkStore = SymlinkStore::new_const();

// -- SymlinkChain : resolution iterative de chaine (RECUR-01) ─────────────────

/// Resultat de la resolution d une chaine de symlinks.
#[derive(Debug)]
pub struct SymlinkResolution {
    /// Composants resolus.
    pub components: Vec<PathComponent>,
    /// Nombre de niveaux d indirection suivis.
    pub depth:      usize,
    /// `true` si le chemin final est absolu.
    pub is_absolute: bool,
}

/// Resout iterativement une chaine de symlinks.
///
/// - `initial_target` : cible du premier symlink (octets bruts).
/// - `max_depth`       : limite SYMLINK_MAX_DEPTH.
/// - `resolver`        : callback pour obtenir la cible d un composant.
///
/// # Regles spec
/// - **RECUR-01** : boucle iterative, pas de recursion.
/// - **OOM-02**   : try_reserve(1) avant push.
///
/// # Errors
/// - `TooManySymlinks` si depth > max_depth.
/// - `InvalidPathComponent` / `PathTooLong` si chemin invalide.
pub fn resolve_symlink_chain<F>(
    initial_target: &[u8],
    max_depth:      usize,
    resolver:       F,
) -> ExofsResult<SymlinkResolution>
where
    F: Fn(&[u8]) -> Option<Vec<u8>>,
{
    let mut depth:       usize  = 0;
    let mut current:     Vec<u8>;
    let mut components:  Vec<PathComponent> = Vec::new();
    let mut is_absolute: bool;

    // Copie initiale.
    let mut buf: Vec<u8> = Vec::new();
    buf.try_reserve(initial_target.len()).map_err(|_| ExofsError::NoMemory)?;
    buf.extend_from_slice(initial_target);
    current = buf;

    // Boucle de resolution iterative.
    loop {
        if depth > max_depth { return Err(ExofsError::TooManySymlinks); }

        is_absolute = current.first() == Some(&b'/');
        let mut parser = PathParser::new(&current)?;
        components.clear();

        let mut found_symlink = false;
        loop {
            match parser.next_component()? {
                None       => break,
                Some(comp) => {
                    // Verifier si ce composant est lui-meme un symlink.
                    if let Some(next_target) = resolver(comp.as_bytes()) {
                        // Substituer.
                        current = next_target;
                        depth   = depth.checked_add(1).ok_or(ExofsError::OffsetOverflow)?;
                        if depth > max_depth { return Err(ExofsError::TooManySymlinks); }
                        found_symlink = true;
                        break;
                    } else {
                        components.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                        components.push(comp);
                    }
                }
            }
        }

        if !found_symlink { break; }
    }

    Ok(SymlinkResolution { components, depth, is_absolute })
}

// -- Utilitaires ──────────────────────────────────────────────────────────────

/// Verifie si `raw` est une cible de symlink valide.
pub fn is_valid_symlink_target(raw: &[u8]) -> bool {
    !raw.is_empty() && raw.len() <= SYMLINK_TARGET_MAX && !raw.contains(&0u8)
}

/// Enregistre une cible de symlink dans le store global.
pub fn register_symlink(oid: &ObjectId, target: &[u8]) -> ExofsResult<()> {
    SYMLINK_STORE.store(oid, target)
}

/// Invalide un symlink dans le store global.
pub fn invalidate_symlink(oid: &ObjectId) { SYMLINK_STORE.invalidate(oid); }

// -- Tests ────────────────────────────────────────────────────────────────────

// -- SymlinkResolveContext : contexte de resolution ───────────────────────────

/// Contexte de résolution d une chaine de symlinks.
///
/// Maintient l état complet pour une résolution itérative multi-étape.
pub struct SymlinkResolveContext {
    /// Chemin de travail courant.
    current_path: Vec<u8>,
    /// Profondeur de résolution atteinte.
    pub depth:    usize,
    /// Limite de profondeur.
    pub limit:    usize,
}

impl SymlinkResolveContext {
    /// Crée un contexte pour le chemin initial.
    pub fn new(path: &[u8], limit: usize) -> ExofsResult<Self> {
        if path.len() > SYMLINK_TARGET_MAX { return Err(ExofsError::PathTooLong); }
        let mut current_path: Vec<u8> = Vec::new();
        current_path.try_reserve(path.len()).map_err(|_| ExofsError::NoMemory)?;
        current_path.extend_from_slice(path);
        Ok(SymlinkResolveContext { current_path, depth: 0, limit })
    }

    /// Suit un symlink : met à jour le chemin courant.
    pub fn follow(&mut self, target: &[u8]) -> ExofsResult<()> {
        self.depth = self.depth.checked_add(1).ok_or(ExofsError::OffsetOverflow)?;
        if self.depth > self.limit { return Err(ExofsError::TooManySymlinks); }
        if target.len() > SYMLINK_TARGET_MAX { return Err(ExofsError::PathTooLong); }
        self.current_path.clear();
        self.current_path.try_reserve(target.len()).map_err(|_| ExofsError::NoMemory)?;
        self.current_path.extend_from_slice(target);
        Ok(())
    }

    /// Chemin courant.
    pub fn current(&self) -> &[u8] { &self.current_path }

    /// `true` si la limite de profondeur est atteinte.
    pub fn is_exhausted(&self) -> bool { self.depth >= self.limit }
}

// -- SymlinkMetadata ----------------------------------------------------------

/// Métadonnées rattachées à un lien symbolique.
#[derive(Clone, Debug)]
pub struct SymlinkMetadata {
    pub oid:            ObjectId,
    pub target_len:     usize,
    pub is_absolute:    bool,
    pub creation_tick:  u64,
}

impl SymlinkMetadata {
    /// Crée des métadonnées depuis une cible.
    pub fn from_target(oid: ObjectId, target: &[u8]) -> Self {
        SymlinkMetadata {
            oid,
            target_len:    target.len(),
            is_absolute:   target.first() == Some(&b'/'),
            creation_tick: crate::arch::time::read_ticks(),
        }
    }
}

// -- Fonctions de vérification structurelle -----------------------------------

/// Vérifie la structure d une cible de symlink.
///
/// - Pas vide.
/// - Longueur ≤ SYMLINK_TARGET_MAX.
/// - Pas de NUL.
/// - Valide UTF-8.
pub fn validate_symlink_target(raw: &[u8]) -> ExofsResult<()> {
    if raw.is_empty()                { return Err(ExofsError::InvalidPathComponent); }
    if raw.len() > SYMLINK_TARGET_MAX { return Err(ExofsError::PathTooLong); }
    if raw.contains(&0u8)            { return Err(ExofsError::InvalidPathComponent); }
    core::str::from_utf8(raw).map_err(|_| ExofsError::InvalidPathComponent)?;
    Ok(())
}

/// Vérifie et retourne un `SymlinkTarget`.
pub fn checked_symlink_target(raw: &[u8], oid: ObjectId) -> ExofsResult<SymlinkTarget> {
    validate_symlink_target(raw)?;
    SymlinkTarget::new(raw, oid)
}

// -- Tests supplémentaires ---------------------------------------------------
#[cfg(test)]
mod extra_tests {
    use super::*;

    fn fake_oid(b: u8) -> ObjectId { let mut a = [0u8; 32]; a[0] = b; ObjectId(a) }

    #[test] fn test_resolve_context_follow() {
        let mut ctx = SymlinkResolveContext::new(b"/a/b", 5).unwrap();
        ctx.follow(b"/c/d").unwrap();
        assert_eq!(ctx.current(), b"/c/d");
        assert_eq!(ctx.depth, 1);
    }

    #[test] fn test_resolve_context_exhaustion() {
        let mut ctx = SymlinkResolveContext::new(b"/loop", 2).unwrap();
        ctx.follow(b"/loop").unwrap();
        ctx.follow(b"/loop").unwrap();
        assert!(ctx.is_exhausted());
        assert!(ctx.follow(b"/loop").is_err());
    }

    #[test] fn test_metadata() {
        let oid = fake_oid(9);
        let m = SymlinkMetadata::from_target(oid.clone(), b"/var/run");
        assert!(m.is_absolute);
        assert_eq!(m.target_len, 8);
    }

    #[test] fn test_validate_target_ok() {
        validate_symlink_target(b"/valid").unwrap();
    }

    #[test] fn test_validate_target_nul() {
        let bad = b"path\x00nul";
        assert!(validate_symlink_target(bad).is_err());
    }

    #[test] fn test_checked_target() {
        let oid = fake_oid(10);
        let t = checked_symlink_target(b"/usr/local", oid).unwrap();
        assert_eq!(t.as_bytes(), b"/usr/local");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_oid(b: u8) -> ObjectId { let mut a = [0u8; 32]; a[0] = b; ObjectId(a) }

    #[test] fn test_symlink_target_new() {
        let t = SymlinkTarget::new(b"/home/user", fake_oid(1)).unwrap();
        assert_eq!(t.as_bytes(), b"/home/user");
        assert!(t.is_absolute);
    }
    #[test] fn test_symlink_target_relative() {
        let t = SymlinkTarget::new(b"../other", fake_oid(2)).unwrap();
        assert!(!t.is_absolute);
    }
    #[test] fn test_symlink_target_empty() {
        assert!(matches!(
            SymlinkTarget::new(b"", fake_oid(3)),
            Err(ExofsError::InvalidPathComponent)
        ));
    }
    #[test] fn test_store_lookup() {
        let store = SymlinkStore::new_const();
        let oid = fake_oid(4);
        store.store(&oid, b"/var/run").unwrap();
        let res = store.lookup(&oid).unwrap();
        assert_eq!(res, b"/var/run");
    }
    #[test] fn test_store_invalidate() {
        let store = SymlinkStore::new_const();
        let oid = fake_oid(5);
        store.store(&oid, b"/tmp").unwrap();
        store.invalidate(&oid);
        assert!(store.lookup(&oid).is_none());
    }
    #[test] fn test_depth_limit() {
        // Resolver qui boucle toujours.
        let result = resolve_symlink_chain(
            b"/loop",
            40,
            |_| Some(b"/loop".to_vec()),
        );
        assert!(matches!(result, Err(ExofsError::TooManySymlinks)));
    }
    #[test] fn test_resolve_no_symlink() {
        let res = resolve_symlink_chain(
            b"/home/user/file",
            40,
            |_| None, // Aucun composant est un symlink.
        ).unwrap();
        assert_eq!(res.depth, 0);
        assert!(res.is_absolute);
    }
    #[test] fn test_is_valid_target() {
        assert!(is_valid_symlink_target(b"/valid/path"));
        assert!(!is_valid_symlink_target(b""));
        assert!(!is_valid_symlink_target(b"path\0with\0nul"));
    }
}
