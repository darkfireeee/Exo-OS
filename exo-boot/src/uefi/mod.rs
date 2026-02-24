//! uefi/ — Chemin UEFI du bootloader Exo-OS.
//!
//! Ce module regroupe toutes les interactions avec le firmware UEFI :
//! - Point d'entrée EFI (entry.rs)
//! - Boot Services wrapper (services.rs)  
//! - Vérification Secure Boot chaîne de confiance (secure_boot.rs)
//! - ExitBootServices avec gestion du point de non-retour (exit.rs)
//! - Protocoles UEFI utilisés (protocols/)
//!
//! RÈGLES ARCHITECTURALES (DOC10/BOOT-*) :
//!   BOOT-01 : Exo-boot est un binaire séparé du kernel — zéro code partagé.
//!   BOOT-02 : Signature Ed25519 vérifiée AVANT tout chargement kernel.
//!   BOOT-06 : ExitBootServices = point de non-retour — aucun BS après.

pub mod entry;
pub mod exit;
pub mod protocols;
pub mod secure_boot;
pub mod services;
