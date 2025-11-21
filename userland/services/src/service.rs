//! # Service Trait - Interface Commune pour Services
//!
//! Définit le contrat que tous les services doivent respecter.

use alloc::vec::Vec;
use exo_types::{Capability, Result, Rights};

/// État de santé d'un service
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    /// Service fonctionne normalement
    Healthy,
    /// Service dégradé mais fonctionnel
    Degraded,
    /// Service en erreur
    Unhealthy,
}

/// Capabilities requises par un service
#[derive(Debug, Clone)]
pub struct ServiceCapabilities {
    /// Rights nécessaires pour le fonctionnement
    pub required_rights: Vec<Rights>,
    
    /// Capabilities optionnelles (meilleures performances si présentes)
    pub optional_rights: Vec<Rights>,
}

/// Trait que tous les services doivent implémenter
pub trait Service {
    /// Nom unique du service
    fn name(&self) -> &str;
    
    /// Capabilities requises pour fonctionner
    fn capabilities_required(&self) -> &ServiceCapabilities;
    
    /// Liste des services dont ce service dépend
    ///
    /// Ces services seront démarrés avant celui-ci
    fn dependencies(&self) -> &[&str] {
        &[] // Par défaut: pas de dépendances
    }
    
    /// Démarre le service
    ///
    /// Appelé par le gestionnaire de services (init)
    fn start(&mut self) -> Result<()>;
    
    /// Arrête le service proprement
    ///
    /// Doit nettoyer les ressources et terminer les tâches en cours
    fn stop(&mut self) -> Result<()>;
    
    /// Vérifie l'état de santé du service
    ///
    /// Appelé périodiquement par le gestionnaire
    fn health_check(&self) -> HealthStatus {
        HealthStatus::Healthy // Par défaut: toujours healthy
    }
    
    /// Redémarre le service
    ///
    /// Implémentation par défaut: stop puis start
    fn restart(&mut self) -> Result<()> {
        self.stop()?;
        self.start()
    }
}
