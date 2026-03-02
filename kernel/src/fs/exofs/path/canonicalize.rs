// path/canonicalize.rs — Normalisation de chemins ExoFS
// Ring 0, no_std
//
// Résout /../ et /./ dans un chemin brut, en place.
// Utilise le buffer per-CPU — jamais de tableau statique sur stack.

use crate::fs::exofs::core::{ExofsError, PATH_MAX};
use alloc::vec::Vec;

/// Normalise un chemin en résolvant les composants '.' et '..'.
///
/// Résultat écrit dans `output` (slice réutilisé).
/// Retourne le nombre d'octets du chemin normalisé dans output.
///
/// # Règles
/// - Travaille sur des octets bruts (pas de conversion string)
/// - Utilise une pile Vec<&[u8]> allouée sur le heap (règle RECUR-04)
/// - Jamais de récursion (règle RECUR-01)
pub fn canonicalize_path(
    input: &[u8],
    output: &mut Vec<u8>,
) -> Result<usize, ExofsError> {
    if input.len() > PATH_MAX {
        return Err(ExofsError::PathTooLong);
    }

    output.clear();
    output.try_reserve(input.len() + 1).map_err(|_| ExofsError::NoMemory)?;

    // Pile des composants validés (indices dans input)
    let mut stack: Vec<&[u8]> = Vec::new();
    stack.try_reserve(64).map_err(|_| ExofsError::NoMemory)?;

    let is_absolute = input.first() == Some(&b'/');
    let path = if is_absolute { &input[1..] } else { input };

    for component in path.split(|&b| b == b'/') {
        match component {
            b"" | b"." => {
                // Ignore
            }
            b".." => {
                // Remonte d'un niveau si possible
                stack.pop();
            }
            name => {
                stack.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                stack.push(name);
            }
        }
    }

    // Reconstruit le chemin normalisé
    if is_absolute {
        output.push(b'/');
    }

    for (i, component) in stack.iter().enumerate() {
        if i > 0 {
            output.push(b'/');
        }
        output.extend_from_slice(component);
    }

    Ok(output.len())
}

/// Normalise un chemin en place dans un buffer mutable.
/// Écrase le contenu de `buf[..path_len]`.
/// Retourne la nouvelle longueur.
pub fn canonicalize_in_place(buf: &mut [u8], path_len: usize) -> Result<usize, ExofsError> {
    if path_len > buf.len() || path_len > PATH_MAX {
        return Err(ExofsError::PathTooLong);
    }

    let mut tmp = Vec::new();
    canonicalize_path(&buf[..path_len], &mut tmp)?;
    let out_len = tmp.len();
    if out_len > buf.len() {
        return Err(ExofsError::PathTooLong);
    }
    buf[..out_len].copy_from_slice(&tmp);
    Ok(out_len)
}
