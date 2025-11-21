//! # Implémentation du Slow Path pour les Syscalls
//!
//! Ce module contient la logique de gestion pour les syscalls complexes.
//! Il effectue le dispatch vers les handlers appropriés dans le module `handlers`.

use crate::syscall::abi::SyscallArgs;
use crate::syscall::handlers;

/// Gère un syscall du "slow path".
///
/// Cette fonction sert de pont. Elle pourrait être enrichie pour inclure
/// du logging, du profiling, ou d'autres logiques transversales avant
/// d'appeler le handler final.
pub fn handle(number: usize, args: SyscallArgs) -> isize {
    // Dans une implémentation plus avancée, on pourrait ajouter ici :
    // - Du logging détaillé pour le débogage.
    // - Du profiling pour mesurer les performances de chaque syscall.
    // - De la vérification de sécurité (par ex, seccomp).

    // Dispatch vers le handler spécifique dans `handlers/`.
    let result = handlers::dispatch(number, &args);

    // La conversion en `isize` est déjà faite dans `syscall::handle`.
    // On retourne juste le résultat pour l'instant.
    // Note: La structure actuelle fait la conversion dans `syscall::handle`
    // après avoir appelé `entry::slow_path::handle`. C'est cohérent.
    result
}