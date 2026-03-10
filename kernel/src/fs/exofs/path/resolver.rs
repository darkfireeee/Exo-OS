//! resolver.rs — Résolution itérative de chemins ExoFS
//!
//! Règles critiques appliquées :
//!  - RECUR-01 : zéro récursion — boucle `while` explicite
//!  - PATH-07  : ResolveContext alloué sur le tas (Vec<PathComponent>)
//!  - OOM-02   : try_reserve avant tout push
//!  - ARITH-02 : arithmétique vérifiée


extern crate alloc;
use alloc::vec::Vec;

use crate::fs::exofs::core::{ExofsError, ExofsResult, ObjectId};
use crate::fs::exofs::path::path_component::{
    PathComponent, PathParser, PATH_MAX,
};
use crate::fs::exofs::path::path_cache::{cached_lookup, CacheLookup, PATH_CACHE};
use crate::fs::exofs::path::symlink::{SYMLINK_MAX_DEPTH, SYMLINK_STORE};
use crate::fs::exofs::path::mount_point::MOUNT_TABLE;
use crate::fs::exofs::path::canonicalize::canonicalize_to_vec;

// ─────────────────────────────────────────────────────────────────────────────
// Trait PathResolver
// ─────────────────────────────────────────────────────────────────────────────

/// Interface que tout back-end de système de fichiers doit implémenter
/// pour permettre la résolution de chemins.
pub trait PathResolver {
    /// Cherche `name` dans `dir_oid`.
    ///
    /// Retourne `(oid, is_symlink)` ou `None` si absent.
    fn lookup_in_dir(
        &self,
        dir_oid:   &ObjectId,
        name:      &PathComponent,
    ) -> ExofsResult<Option<(ObjectId, bool)>>;

    /// Lit la cible d'un lien symbolique.
    fn symlink_target(&self, oid: &ObjectId) -> ExofsResult<Vec<u8>>;

    /// OID racine du système de fichiers.
    fn root_oid(&self) -> ObjectId;


}

// ─────────────────────────────────────────────────────────────────────────────
// ResolveContext — état sur le tas (PATH-07)
// ─────────────────────────────────────────────────────────────────────────────

/// Contexte d'une résolution en cours.
///
/// Alloué sur le tas pour éviter `[u8; PATH_MAX]` sur la pile (PATH-07).
pub struct ResolveContext {
    /// OID courant lors de la descente.
    pub current_oid:    ObjectId,
    /// Composants restants à résoudre (ordre LIFO — dépilé par pop).
    pub remaining:      Vec<PathComponent>,
    /// Profondeur de résolution de symlinks.
    pub symlink_depth:  usize,
    /// Chemin canonique original (pour le cache).
    pub canonical_path: Vec<u8>,
    /// Drapeaux de résolution.
    pub flags:          ResolveFlags,
}

impl ResolveContext {
    /// Crée un contexte depuis un chemin canonique.
    pub fn new(
        root:           ObjectId,
        canonical:      Vec<u8>,
        flags:          ResolveFlags,
    ) -> ExofsResult<Self> {
        let mut remaining: Vec<PathComponent> = Vec::new();
        let mut parser = PathParser::new(&canonical)?;
        while let Some(c) = parser.next_component()? {
            if c.as_bytes() == b"." { continue; }
            if c.as_bytes() == b".." {
                remaining.pop();
                continue;
            }
            remaining.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            remaining.push(c);
        }
        remaining.reverse(); // pop() donnera le prochain composant
        Ok(ResolveContext {
            current_oid:   root,
            remaining,
            symlink_depth: 0,
            canonical_path: canonical,
            flags,
        })
    }

    /// `true` si tous les composants ont été consommés.
    #[inline]
    pub fn is_done(&self) -> bool { self.remaining.is_empty() }
}

// ─────────────────────────────────────────────────────────────────────────────
// ResolveFlags
// ─────────────────────────────────────────────────────────────────────────────

/// Options de résolution.
#[derive(Clone, Copy, Debug, Default)]
pub struct ResolveFlags {
    /// Ne pas suivre le symlink terminal.
    pub no_follow_last: bool,
    /// Ne pas traverser les points de montage.
    pub no_cross_mount: bool,
    /// Utiliser le cache.
    pub use_cache:      bool,
    /// Mettre en cache le résultat.
    pub cache_result:   bool,
}

impl ResolveFlags {
    /// Drapeaux par défaut (cache activé).
    pub const fn default_flags() -> Self {
        ResolveFlags {
            no_follow_last: false,
            no_cross_mount: false,
            use_cache:      true,
            cache_result:   true,
        }
    }

    /// Résolution sans suivi de symlink final (pour `lstat`).
    pub const fn lstat_flags() -> Self {
        ResolveFlags {
            no_follow_last: true,
            no_cross_mount: false,
            use_cache:      true,
            cache_result:   true,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ResolveResult
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat enrichi d'une résolution.
#[derive(Debug)]
pub struct ResolveResult {
    /// OID de l'objet résolu.
    pub oid:          ObjectId,
    /// `true` si l'objet terminal est un symlink (en mode `no_follow_last`).
    pub is_symlink:   bool,
    /// Nombre de symlinks suivis.
    pub symlink_hops: usize,
}

// ─────────────────────────────────────────────────────────────────────────────
// resolve_path — point d'entrée principal
// ─────────────────────────────────────────────────────────────────────────────

/// Résout `path` depuis la racine du `resolver`.
///
/// Cette fonction est l'implémentation de référence :
///   1. Canonicalise le chemin.
///   2. Consulte le cache (si activé).
///   3. Descend composant par composant (RECUR-01).
///   4. Traverse les symlinks (itératif).
///   5. Traverse les points de montage.
///   6. Met en cache le résultat final (si activé).
pub fn resolve_path<R: PathResolver>(
    path:     &[u8],
    resolver: &R,
) -> ExofsResult<ObjectId> {
    let result = resolve_path_full(path, resolver, ResolveFlags::default_flags())?;
    Ok(result.oid)
}

/// Résout avec retour enrichi et drapeaux explicites.
pub fn resolve_path_full<R: PathResolver>(
    path:     &[u8],
    resolver: &R,
    flags:    ResolveFlags,
) -> ExofsResult<ResolveResult> {
    if path.len() > PATH_MAX { return Err(ExofsError::PathTooLong); }

    // 1. Canonicalisation
    let canonical = canonicalize_to_vec(path)?;

    // 2. Lookup cache
    if flags.use_cache {
        if let CacheLookup::Hit(oid) = cached_lookup(&canonical) {
            return Ok(ResolveResult {
                oid,
                is_symlink:   false,
                symlink_hops: 0,
            });
        }
    }

    // 3. Préparer le contexte
    let root = resolver.root_oid();
    let mut ctx = ResolveContext::new(root, canonical.clone(), flags)?;

    // 4. Descente itérative (RECUR-01 — zéro récursion)
    let mut iters:      usize = 0;
    let     max_iters:  usize = 8192;

    loop {
        if iters >= max_iters { return Err(ExofsError::InternalError); }
        iters = iters.checked_add(1).ok_or(ExofsError::OffsetOverflow)?;

        if ctx.is_done() {
            // 5. Points de montage
            if !flags.no_cross_mount {
                if let Some(mounted) = MOUNT_TABLE.lookup_mount(&ctx.current_oid) {
                    ctx.current_oid = mounted;
                }
            }
            break;
        }

        let comp = ctx.remaining.pop().unwrap(); // safe: !is_done()
        let is_last = ctx.remaining.is_empty();

        // Lookup dans le répertoire courant
        let found = resolver.lookup_in_dir(&ctx.current_oid, &comp)?;
        let (oid, is_symlink) = match found {
            None        => return Err(ExofsError::ObjectNotFound),
            Some(entry) => entry,
        };

        // Traversée du point de montage
        let effective_oid = if !flags.no_cross_mount {
            MOUNT_TABLE.lookup_mount(&oid).unwrap_or_else(|| oid.clone())
        } else {
            oid.clone()
        };

        if is_symlink && !(is_last && flags.no_follow_last) {
            // Résolution du symlink
            let new_depth = ctx.symlink_depth
                .checked_add(1)
                .ok_or(ExofsError::OffsetOverflow)?;
            if new_depth > SYMLINK_MAX_DEPTH {
                return Err(ExofsError::TooManySymlinks);
            }
            ctx.symlink_depth = new_depth;

            // Chercher d'abord dans le store local
            let raw_target = if let Some(t) = SYMLINK_STORE.lookup(&oid) {
                t
            } else {
                resolver.symlink_target(&oid)?
            };

            if raw_target.len() > PATH_MAX {
                return Err(ExofsError::PathTooLong);
            }

            // Préparer une nouvelle séquence de composants depuis la cible
            let target_canonical = canonicalize_to_vec(&raw_target)?;
            let new_root = if raw_target.first() == Some(&b'/') {
                resolver.root_oid()
            } else {
                ctx.current_oid.clone()
            };

            // Reconstruire la file de composants (remaining) depuis la cible
            let mut new_remaining: Vec<PathComponent> = Vec::new();
            let mut parser = PathParser::new(&target_canonical)?;
            while let Some(c) = parser.next_component()? {
                if c.as_bytes() == b"." { continue; }
                if c.as_bytes() == b".." {
                    new_remaining.pop();
                    continue;
                }
                new_remaining.try_reserve(1)
                    .map_err(|_| ExofsError::NoMemory)?;
                new_remaining.push(c);
            }
            // Ajouter les composants restants du chemin original
            // (ils sont déjà en ordre LIFO dans ctx.remaining)
            while let Some(old_comp) = ctx.remaining.pop() {
                new_remaining.try_reserve(1)
                    .map_err(|_| ExofsError::NoMemory)?;
                new_remaining.push(old_comp);
            }
            new_remaining.reverse();

            ctx.current_oid = new_root;
            ctx.remaining   = new_remaining;
            continue;
        }

        ctx.current_oid = effective_oid;
    }

    let result_oid = ctx.current_oid.clone();

    // 6. Mise en cache
    if flags.cache_result {
        PATH_CACHE.insert_path(&canonical, result_oid.clone());
    }

    Ok(ResolveResult {
        oid:          result_oid,
        is_symlink:   false,
        symlink_hops: ctx.symlink_depth,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Résout le chemin sans suivre le symlink terminal (`lstat` semantics).
pub fn resolve_no_follow<R: PathResolver>(
    path:     &[u8],
    resolver: &R,
) -> ExofsResult<ResolveResult> {
    resolve_path_full(path, resolver, ResolveFlags::lstat_flags())
}

/// Résout un chemin et vérifie qu'il existe.
pub fn path_exists<R: PathResolver>(path: &[u8], resolver: &R) -> bool {
    resolve_path(path, resolver).is_ok()
}

/// Résout le répertoire parent.
pub fn resolve_parent<R: PathResolver>(
    path:     &[u8],
    resolver: &R,
) -> ExofsResult<ObjectId> {
    let canonical = canonicalize_to_vec(path)?;
    let slash = canonical.iter().rposition(|&b| b == b'/');
    let parent = match slash {
        None | Some(0) => b"/" as &[u8],
        Some(p)        => &canonical[..p],
    };
    resolve_path(parent, resolver)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn oid(b: u8) -> ObjectId { let mut a = [0u8; 32]; a[0] = b; ObjectId(a) }

    struct MockResolver;
    impl PathResolver for MockResolver {
        fn lookup_in_dir(
            &self,
            _dir: &ObjectId,
            comp: &PathComponent,
        ) -> ExofsResult<Option<(ObjectId, bool)>> {
            match comp.as_bytes() {
                b"usr"  => Ok(Some((oid(10), false))),
                b"bin"  => Ok(Some((oid(11), false))),
                b"lnk"  => Ok(Some((oid(20), true))),
                _       => Ok(None),
            }
        }
        fn symlink_target(&self, _oid: &ObjectId) -> ExofsResult<Vec<u8>> {
            let mut v = Vec::new(); v.extend_from_slice(b"/usr/bin"); Ok(v)
        }
        fn root_oid(&self) -> ObjectId { oid(1) }
    }

    #[test] fn test_resolve_simple() {
        let r = MockResolver;
        let res = resolve_path(b"/usr/bin", &r).unwrap();
        assert_eq!(res.0[0], 11);
    }

    #[test] fn test_resolve_not_found() {
        let r = MockResolver;
        assert!(resolve_path(b"/usr/missing", &r).is_err());
    }

    #[test] fn test_resolve_symlink() {
        let r = MockResolver;
        // /lnk → /usr/bin
        let res = resolve_path(b"/lnk", &r).unwrap();
        assert_eq!(res.0[0], 11);
    }

    #[test] fn test_resolve_no_follow() {
        let r = MockResolver;
        let res = resolve_no_follow(b"/lnk", &r).unwrap();
        // Avec no_follow_last, on doit rester sur le symlink lui-même
        assert_eq!(res.oid.0[0], 20);
    }

    #[test] fn test_path_exists() {
        let r = MockResolver;
        assert!(path_exists(b"/usr", &r));
        assert!(!path_exists(b"/nonexistent", &r));
    }

    #[test] fn test_resolve_parent() {
        let r = MockResolver;
        let p = resolve_parent(b"/usr/bin", &r).unwrap();
        assert_eq!(p.0[0], 10);
    }
}
