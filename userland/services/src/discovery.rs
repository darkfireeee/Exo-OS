//! # Service Discovery - Découverte et Connexion entre Services
//!
//! Permet aux services de se trouver et de se connecter entre eux.

use alloc::{string::String, vec::Vec};
use exo_types::{ErrorCode, ExoError, Result};
use log::debug;

/// Informations sur un service découvert
#[derive(Debug, Clone)]
pub struct ServiceInfo {
    /// Nom du service
    pub name: String,

    /// ID du service
    pub id: u64,

    /// Endpoint IPC pour se connecter au service
    pub endpoint: String,

    /// État actuel du service
    pub is_running: bool,
}

/// Client pour la découverte de services
pub struct ServiceDiscovery;

impl ServiceDiscovery {
    /// Trouve un service par son nom
    ///
    /// # Arguments
    /// - `name` - Nom du service à rechercher
    ///
    /// # Returns
    /// Informations sur le service si trouvé
    pub fn find_service(name: &str) -> Result<ServiceInfo> {
        debug!("Looking up service: {}", name);

        // TODO: Requête IPC au registry central (probablement dans init)
        // QueryService { name: String } -> ServiceInfo

        // Pour l'instant, retourner une erreur
        Err(ExoError::with_message(
            ErrorCode::NotFound,
            "Service discovery not yet implemented",
        ))
    }

    /// Liste tous les services enregistrés
    ///
    /// # Returns
    /// Liste des services disponibles
    pub fn list_services() -> Result<Vec<ServiceInfo>> {
        debug!("Listing all services");

        // TODO: Requête IPC au registry
        // ListServices {} -> Vec<ServiceInfo>

        // Pour l'instant, retourner liste vide
        Ok(Vec::new())
    }

    /// Attend qu'un service soit disponible
    ///
    /// Bloque jusqu'à ce que le service soit démarré ou timeout
    ///
    /// # Arguments
    /// - `name` - Nom du service à attendre
    /// - `timeout_ms` - Timeout en millisecondes (0 = infini)
    pub fn wait_for_service(name: &str, timeout_ms: u64) -> Result<ServiceInfo> {
        debug!("Waiting for service: {} (timeout: {}ms)", name, timeout_ms);

        // TODO: Implémentation avec polling ou notification
        // Boucle:
        //   1. Tenter find_service()
        //   2. Si trouvé et running, retourner
        //   3. Sleep puis réessayer
        //   4. Si timeout dépassé, erreur

        Err(ExoError::with_message(
            ErrorCode::Timeout,
            "Service wait not yet implemented",
        ))
    }
}
