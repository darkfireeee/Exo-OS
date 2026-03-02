// path/symlink.rs — Résolution de symlinks ExoFS
// Ring 0, no_std
//
// RÈGLES :
//   • SYMLINK-01 : MAX_DEPTH = 40 — erreur si dépassé
//   • RECUR-01   : résolution itérative, jamais récursive
//   • La cible d'un symlink est une Relation::Symlink

use crate::fs::exofs::core::{ObjectId, EpochId, ExofsError, SYMLINK_MAX_DEPTH};
use crate::fs::exofs::path::path_component::{PathComponent, validate_component};
use crate::fs::exofs::relation::relation::{Relation, RelationType};
use alloc::vec::Vec;

/// Résout la cible d'un symlink vers une liste de composants de chemin.
///
/// Le symlink est stocké comme une Relation::Symlink dont le payload
/// contient la cible (chemin UTF-8 relatif ou absolu).
///
/// Retourne les composants du chemin cible à insérer dans la résolution courante.
pub fn resolve_symlink_target(
    symlink_oid: ObjectId,
    epoch: EpochId,
) -> Result<Vec<PathComponent>, ExofsError> {
    use crate::fs::exofs::relation::relation_storage::load_relation_payload;

    // Charge le payload de la relation Symlink
    let payload = load_relation_payload(symlink_oid, epoch)?;

    // Le payload est le chemin cible UTF-8
    let target_path = core::str::from_utf8(&payload)
        .map_err(|_| ExofsError::InvalidPathComponent)?;

    // Parse les composants du chemin cible
    parse_target_components(target_path.as_bytes())
}

/// Parse un chemin cible de symlink en composants validés.
/// Retourne une liste vide si le chemin est simplement "/" (racine).
fn parse_target_components(path: &[u8]) -> Result<Vec<PathComponent>, ExofsError> {
    if path.len() > crate::fs::exofs::core::PATH_MAX {
        return Err(ExofsError::PathTooLong);
    }

    let mut components = Vec::new();

    // Si chemin absolu, ajouter un composant spécial de racine
    let path = if path.first() == Some(&b'/') {
        // composant racine — le resolver saura remonter à la racine
        components.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        components.push(validate_component(b"/").unwrap_or_else(|_| {
            // SAFETY: "/" seul n'est pas un composant valide, le resolver le gère
            unsafe { core::mem::zeroed() }
        }));
        &path[1..]
    } else {
        path
    };

    for component in path.split(|&b| b == b'/') {
        if component.is_empty() {
            continue;
        }
        let pc = validate_component(component)?;
        components.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        components.push(pc);
    }

    Ok(components)
}

/// Vérifie si un objet est un symlink sans charger son contenu complet
#[inline]
pub fn is_object_symlink(oid: ObjectId, epoch: EpochId) -> Result<bool, ExofsError> {
    use crate::fs::exofs::objects::object_loader::quick_kind;
    use crate::fs::exofs::core::ObjectKind;
    let kind = quick_kind(oid, epoch)?;
    Ok(kind == ObjectKind::Relation)
}
