//! # Logique d'entrée des Appels Système
//!
//! Ce module contient la logique de dispatch entre le "fast path" et le "slow path".

pub mod fast_path;
pub mod slow_path;
pub mod validation;

use crate::syscall::{abi, handlers};

/// Gère un syscall du "slow path".
///
/// Ce chemin est pris pour les opérations plus complexes qui nécessitent
/// une validation complète des arguments et potentiellement des interactions
/// avec d'autres sous-systèmes du noyau (VFS, planificateur, etc.).
pub fn handle(number: usize, args: abi::SyscallArgs) -> isize {
    // Validation des arguments depuis l'espace utilisateur.
    // C'est une étape cruciale pour la sécurité.
    // La validation est déléguée à chaque handler qui connaît ses besoins.
    // Par exemple, `sys_read` doit valider son pointeur de buffer.

    // Dispatch vers le handler approprié.
    // La table de dispatch est générée automatiquement par `handlers::dispatch`.
    let result = handlers::dispatch(number, &args);

    // Convertir le résultat en `isize` pour le retour à l'espace utilisateur.
    abi::result_to_isize(result)
}