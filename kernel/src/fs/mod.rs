// Couche 3 : système de fichiers
// Ring 0, no_std
//
// fs/ est la couche la plus haute du noyau — dépend de memory/, scheduler/,
// process/, security/. Initialisée en Phase 7 de kernel_init().

pub mod elf_loader_impl;
/// ExoFS — système de fichiers natif Exo-OS (journalisé par epoch)
pub mod exofs;
