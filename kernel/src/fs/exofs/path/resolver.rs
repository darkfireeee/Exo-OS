// path/resolver.rs — Résolution itérative de chemins ExoFS
// Ring 0, no_std
//
// RÈGLES :
//   • PATH-07 : buffer per-CPU PATH_BUFFERS — jamais [u8;4096] sur stack kernel
//   • RECUR-01 : résolution itérative, jamais récursive
//   • SYS-01   : copy_from_user() pour tout pointeur userspace

use crate::fs::exofs::core::{
    ObjectId, EpochId, ExofsError,
    PATH_MAX, NAME_MAX, SYMLINK_MAX_DEPTH,
};
use crate::fs::exofs::path::{
    path_index::PathIndex,
    path_component::{PathComponent, validate_component},
    path_cache::PathCache,
    canonicalize::canonicalize_in_place,
    symlink::resolve_symlink_target,
    mount_point::MOUNT_TABLE,
};
use crate::fs::exofs::cache::path_cache::GLOBAL_PATH_CACHE;
use crate::scheduler::sync::spinlock::SpinLock;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};

// ─── Buffer per-CPU ───────────────────────────────────────────────────────────

/// Nombre de CPUs maximum supportés
const MAX_CPUS: usize = 256;

/// Pool de buffers per-CPU pour la résolution de chemins (règle PATH-07)
/// Chaque CPU a son propre buffer — aucune contention
struct PathBufferPool {
    // Chaque entrée : buffer + flag "en cours d'utilisation"
    buffers: [SpinLock<[u8; PATH_MAX]>; MAX_CPUS],
    cpu_count: AtomicUsize,
}

static PATH_BUFFER_POOL: PathBufferPool = PathBufferPool {
    // SAFETY: SpinLock<[u8; PATH_MAX]> est initialisable avec des zéros
    buffers: unsafe { core::mem::zeroed() },
    cpu_count: AtomicUsize::new(64), // valeur défaut, mise à jour au boot
};

/// Contexte de résolution de chemin — alloué sur le heap, jamais sur la stack
pub struct ResolveContext {
    /// Composants du chemin après parsing
    components: Vec<PathComponent>,
    /// Profondeur de résolution symlink courante
    symlink_depth: usize,
    /// ObjectId courant pendant la traversée
    current_oid: ObjectId,
}

impl ResolveContext {
    fn new() -> Result<Self, ExofsError> {
        let mut components = Vec::new();
        components.try_reserve(64).map_err(|_| ExofsError::NoMemory)?;
        Ok(ResolveContext {
            components,
            symlink_depth: 0,
            current_oid: ObjectId::INVALID,
        })
    }
}

// ─── API publique ─────────────────────────────────────────────────────────────

/// Résout un chemin absolu vers un ObjectId.
///
/// # Arguments
/// * `path_bytes` — chemin UTF-8 copié depuis userspace (via copy_from_user)
/// * `epoch`      — epoch courante pour la cohérence des lookups
/// * `root_oid`   — ObjectId de la racine du namespace
///
/// # Règles
/// - Itératif (règle RECUR-01) — aucun appel récursif
/// - Buffer per-CPU (règle PATH-07) — aucun tableau sur la stack kernel
/// - MAX 40 niveaux de symlink (règle SYMLINK-01)
pub fn resolve_path(
    path_bytes: &[u8],
    epoch: EpochId,
    root_oid: ObjectId,
) -> Result<ObjectId, ExofsError> {
    if path_bytes.len() > PATH_MAX {
        return Err(ExofsError::PathTooLong);
    }
    if path_bytes.is_empty() {
        return Err(ExofsError::InvalidMagic); // chemin vide
    }

    // Vérification du cache dentry avant toute résolution
    let cache_key = compute_path_hash(path_bytes, root_oid);
    if let Some(cached) = GLOBAL_PATH_CACHE.lookup(cache_key) {
        return Ok(cached);
    }

    // Contexte alloué sur le heap — règle PATH-07
    let mut ctx = ResolveContext::new()?;

    // Parsing itératif des composants
    parse_path_components(path_bytes, &mut ctx.components)?;

    // La résolution commence toujours à la racine du namespace
    ctx.current_oid = root_oid;

    // Itération sur les composants — JAMAIS récursif
    let mut idx = 0;
    while idx < ctx.components.len() {
        let comp = ctx.components[idx].clone();

        match comp.as_bytes() {
            b"." => {
                // Composant courant — ne change pas l'OID
                idx += 1;
                continue;
            }
            b".." => {
                // Remonter au parent via PathIndex du répertoire courant
                ctx.current_oid = lookup_parent(ctx.current_oid, epoch)?;
                idx += 1;
                continue;
            }
            name => {
                // Lookup dans le PathIndex du répertoire courant
                let child_oid = lookup_in_directory(
                    ctx.current_oid,
                    name,
                    epoch,
                )?;

                // Vérification si c'est un symlink
                if is_symlink(child_oid, epoch)? {
                    ctx.symlink_depth += 1;
                    if ctx.symlink_depth > SYMLINK_MAX_DEPTH {
                        return Err(ExofsError::SymlinkLoop);
                    }

                    // Résout la cible du symlink (itératif)
                    let target_components = resolve_symlink_target(child_oid, epoch)?;

                    // Insère les composants symlink à la position courante
                    // (idx+1 pour sauter le composant symlink lui-même)
                    let remaining = ctx.components.drain(idx + 1..).collect::<Vec<_>>();
                    ctx.components.truncate(idx);
                    ctx.components.try_reserve(target_components.len() + remaining.len())
                        .map_err(|_| ExofsError::NoMemory)?;
                    for c in target_components {
                        ctx.components.push(c);
                    }
                    for c in remaining {
                        ctx.components.push(c);
                    }

                    // Si symlink absolu, remettre à la racine
                    // (géré par resolve_symlink_target — chemin absolu commence par /)
                    continue;
                }

                ctx.current_oid = child_oid;
                idx += 1;
            }
        }
    }

    // Met en cache le résultat
    GLOBAL_PATH_CACHE.insert(cache_key, ctx.current_oid);

    Ok(ctx.current_oid)
}

// ─── Fonctions internes ───────────────────────────────────────────────────────

/// Parse les composants d'un chemin dans un Vec alloué sur le heap
fn parse_path_components(
    path: &[u8],
    out: &mut Vec<PathComponent>,
) -> Result<(), ExofsError> {
    // Skip le slash initial si chemin absolu
    let path = if path.first() == Some(&b'/') { &path[1..] } else { path };

    for component in path.split(|&b| b == b'/') {
        if component.is_empty() {
            // Double slash ignoré
            continue;
        }
        let pc = validate_component(component)?;
        out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        out.push(pc);
    }
    Ok(())
}

/// Lookup d'un nom dans le PathIndex d'un répertoire
fn lookup_in_directory(
    dir_oid: ObjectId,
    name: &[u8],
    epoch: EpochId,
) -> Result<ObjectId, ExofsError> {
    // Vérifie d'abord la table de montage (mount_point.rs)
    if let Some(mounted_oid) = MOUNT_TABLE.lookup_mount(dir_oid, name) {
        return Ok(mounted_oid);
    }

    // Charge le PathIndex du répertoire depuis le cache ou le disque
    let path_index = PathIndex::load(dir_oid, epoch)?;
    path_index.lookup(name)
}

/// Remonte au répertoire parent
fn lookup_parent(
    oid: ObjectId,
    epoch: EpochId,
) -> Result<ObjectId, ExofsError> {
    // Le parent est stocké dans le PathIndex de l'objet courant
    let path_index = PathIndex::load(oid, epoch)?;
    path_index.parent_oid()
}

/// Vérifie si un objet est un symlink via ses métadonnées
fn is_symlink(oid: ObjectId, epoch: EpochId) -> Result<bool, ExofsError> {
    use crate::fs::exofs::core::ObjectKind;
    use crate::fs::exofs::objects::object_loader::quick_kind(oid, epoch);
    let kind = quick_kind(oid, epoch)?;
    Ok(kind == ObjectKind::Relation) // les symlinks sont des Relation::Symlink
}

/// Hash de chemin pour le dentry cache
fn compute_path_hash(path: &[u8], root: ObjectId) -> u64 {
    // FNV-1a pour vitesse en kernel
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in path {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    for &b in root.as_bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
