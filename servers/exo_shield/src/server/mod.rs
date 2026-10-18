//! # exo_shield::server — Sous-système serveur IPC et orchestration NGAV
//!
//! Organisation modulaire ségréguée :
//! - `protocol`    : Constantes IPC, formats d'enveloppe, statistiques et compteurs globaux
//! - `pipeline`    : Pipeline de traitement des hooks de sécurité, confinement et inférence ML
//! - `gate`        : Porte d'autorisation IPC et vérification des capabilities
//! - `handlers`    : Gestionnaires individuels de commandes IPC et table de dispatch
//! - `feed`        : Dépilage du flux d'événements de sécurité du noyau (SYS_EXO_SHIELD_DRAIN)
//! - `maintenance` : Tâches de maintenance périodique et planificateur de scans

pub mod protocol;
pub mod pipeline;
pub mod gate;
pub mod handlers;
pub mod feed;
pub mod maintenance;

pub use protocol::*;
pub use pipeline::*;
pub use gate::*;
pub use handlers::*;
pub use feed::*;
pub use maintenance::*;
