//! # Signatures Module — Base de signatures et moteurs de détection
//!
//! Module principal pour la gestion des signatures de sécurité :
//! - `database` : stockage statique des signatures (max 256)
//! - `matcher` : correspondance de patterns (exact, wildcard, fuzzy)
//! - `yara` : moteur de règles YARA-like
//! - `update` : mise à jour signée Ed25519 avec rollback

pub mod database;
pub mod matcher;
pub mod yara;
pub mod update;

/// Initialise tous les sous-modules de signatures.
pub fn signatures_init() {
    database::database_init();
    yara::yara_init();
    update::update_init();
}
