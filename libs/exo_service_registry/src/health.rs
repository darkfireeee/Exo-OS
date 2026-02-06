//! Health checking pour services
//!
//! Fournit:
//! - Heartbeat monitoring automatique
//! - Ping/pong health checks
//! - Detection de services crashed
//! - Recovery automatique

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::format;
use core::fmt;

use crate::types::{ServiceName, RegistryError, RegistryResult};
use crate::registry::Registry;

/// Configuration du health checker
#[derive(Debug, Clone)]
pub struct HealthConfig {
    /// Intervalle de check en secondes
    pub check_interval_secs: u64,

    /// Timeout pour un ping en millisecondes
    pub ping_timeout_ms: u64,

    /// Nombre de tentatives avant de marquer Failed
    pub max_failures: u32,

    /// Activer le recovery automatique
    pub auto_recovery: bool,

    /// Delay avant retry après échec (secondes)
    pub recovery_delay_secs: u64,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            check_interval_secs: 30,
            ping_timeout_ms: 1000,
            max_failures: 3,
            auto_recovery: true,
            recovery_delay_secs: 10,
        }
    }
}

impl HealthConfig {
    /// Crée une config par défaut
    pub fn new() -> Self {
        Self::default()
    }

    /// Définit l'intervalle de check
    pub fn with_check_interval(mut self, secs: u64) -> Self {
        self.check_interval_secs = secs;
        self
    }

    /// Définit le timeout de ping
    pub fn with_ping_timeout(mut self, ms: u64) -> Self {
        self.ping_timeout_ms = ms;
        self
    }

    /// Définit le nombre max d'échecs
    pub fn with_max_failures(mut self, max: u32) -> Self {
        self.max_failures = max;
        self
    }

    /// Active/désactive l'auto-recovery
    pub fn with_auto_recovery(mut self, enable: bool) -> Self {
        self.auto_recovery = enable;
        self
    }
}

/// Statut de health d'un service
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    /// Service en bonne santé
    Healthy,

    /// Service dégradé (répond lentement)
    Degraded,

    /// Service ne répond pas
    Unhealthy,

    /// Service non vérifié (pas encore de check)
    Unknown,
}

impl HealthStatus {
    /// Vérifie si le service est ok
    #[inline]
    pub const fn is_ok(&self) -> bool {
        matches!(self, Self::Healthy | Self::Degraded)
    }

    /// Vérifie si le service est down
    #[inline]
    pub const fn is_down(&self) -> bool {
        matches!(self, Self::Unhealthy)
    }
}

impl fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Healthy => write!(f, "healthy"),
            Self::Degraded => write!(f, "degraded"),
            Self::Unhealthy => write!(f, "unhealthy"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// Résultat d'un health check
#[derive(Debug, Clone)]
pub struct HealthCheckResult {
    /// Nom du service
    pub service_name: ServiceName,

    /// Statut de health
    pub status: HealthStatus,

    /// Temps de réponse en microsecondes
    pub response_time_us: u64,

    /// Message d'erreur (si unhealthy)
    pub error_message: Option<String>,

    /// Timestamp du check
    pub checked_at: u64,
}

impl HealthCheckResult {
    /// Crée un résultat de succès
    pub fn healthy(name: ServiceName, response_time_us: u64, timestamp: u64) -> Self {
        Self {
            service_name: name,
            status: HealthStatus::Healthy,
            response_time_us,
            error_message: None,
            checked_at: timestamp,
        }
    }

    /// Crée un résultat dégradé
    pub fn degraded(name: ServiceName, response_time_us: u64, timestamp: u64) -> Self {
        Self {
            service_name: name,
            status: HealthStatus::Degraded,
            response_time_us,
            error_message: Some("slow response".into()),
            checked_at: timestamp,
        }
    }

    /// Crée un résultat unhealthy
    pub fn unhealthy(name: ServiceName, error: String, timestamp: u64) -> Self {
        Self {
            service_name: name,
            status: HealthStatus::Unhealthy,
            response_time_us: 0,
            error_message: Some(error),
            checked_at: timestamp,
        }
    }
}

/// Health checker principal
pub struct HealthChecker {
    /// Configuration
    config: HealthConfig,

    /// Derniers résultats de check
    last_results: Vec<HealthCheckResult>,
}

impl HealthChecker {
    /// Crée un nouveau health checker
    pub fn new() -> Self {
        Self::with_config(HealthConfig::default())
    }

    /// Crée avec une config custom
    pub fn with_config(config: HealthConfig) -> Self {
        Self {
            config,
            last_results: Vec::new(),
        }
    }

    /// Ping un service spécifique
    ///
    /// Envoie un ping IPC et attend la réponse
    pub fn ping(&self, name: &ServiceName) -> HealthCheckResult {
        // Dans une vraie implémentation, on enverrait un IPC ping
        // Pour l'instant, on simule un succès
        let timestamp = 0; // TODO: timestamp réel
        HealthCheckResult::healthy(name.clone(), 500, timestamp)
    }

    /// Check tous les services du registry
    pub fn check_all(&mut self, registry: &Registry) -> Vec<HealthCheckResult> {
        let services = registry.list();
        let mut results = Vec::with_capacity(services.len());

        for (name, info) in services {
            let result = if info.is_available() {
                self.ping(&name)
            } else {
                HealthCheckResult::unhealthy(
                    name.clone(),
                    format!("service status: {}", info.status()),
                    0,
                )
            };

            results.push(result);
        }

        self.last_results = results.clone();
        results
    }

    /// Check un service spécifique dans le registry
    pub fn check_service(
        &mut self,
        registry: &Registry,
        name: &ServiceName,
    ) -> RegistryResult<HealthCheckResult> {
        let services = registry.list();
        let info = services
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, i)| i)
            .ok_or_else(|| RegistryError::ServiceNotFound(name.to_string()))?;

        let result = if info.is_available() {
            self.ping(name)
        } else {
            HealthCheckResult::unhealthy(
                name.clone(),
                format!("service status: {}", info.status()),
                0,
            )
        };

        Ok(result)
    }

    /// Retourne les services unhealthy
    pub fn get_unhealthy_services(&self) -> Vec<&HealthCheckResult> {
        self.last_results
            .iter()
            .filter(|r| r.status == HealthStatus::Unhealthy)
            .collect()
    }

    /// Retourne les services degraded
    pub fn get_degraded_services(&self) -> Vec<&HealthCheckResult> {
        self.last_results
            .iter()
            .filter(|r| r.status == HealthStatus::Degraded)
            .collect()
    }

    /// Retourne les derniers résultats
    pub fn last_results(&self) -> &[HealthCheckResult] {
        &self.last_results
    }

    /// Retourne la config
    pub fn config(&self) -> &HealthConfig {
        &self.config
    }

    /// Recovery automatique des services failed
    ///
    /// Tente de redémarrer les services marqués comme failed
    pub fn recover_failed_services(&self, registry: &mut Registry) -> Vec<ServiceName> {
        if !self.config.auto_recovery {
            return Vec::new();
        }

        let mut recovered = Vec::new();

        for result in &self.last_results {
            if result.status == HealthStatus::Unhealthy {
                // Tente un ping de recovery
                let recovery_result = self.ping(&result.service_name);

                if recovery_result.status.is_ok() {
                    // Service revenu online
                    if let Err(_) = registry.heartbeat(&result.service_name) {
                        // Si le heartbeat échoue, le service n'est plus dans le registry
                        continue;
                    }
                    recovered.push(result.service_name.clone());
                }
            }
        }

        recovered
    }

    /// Statistiques de health
    pub fn stats(&self) -> HealthStats {
        let total = self.last_results.len();
        let healthy = self
            .last_results
            .iter()
            .filter(|r| r.status == HealthStatus::Healthy)
            .count();
        let degraded = self
            .last_results
            .iter()
            .filter(|r| r.status == HealthStatus::Degraded)
            .count();
        let unhealthy = self
            .last_results
            .iter()
            .filter(|r| r.status == HealthStatus::Unhealthy)
            .count();

        let avg_response_time = if total > 0 {
            self.last_results.iter().map(|r| r.response_time_us).sum::<u64>() / total as u64
        } else {
            0
        };

        HealthStats {
            total_services: total,
            healthy_count: healthy,
            degraded_count: degraded,
            unhealthy_count: unhealthy,
            avg_response_time_us: avg_response_time,
        }
    }
}

impl Default for HealthChecker {
    fn default() -> Self {
        Self::new()
    }
}

/// Statistiques de health
#[derive(Debug, Clone, Copy)]
pub struct HealthStats {
    /// Nombre total de services
    pub total_services: usize,

    /// Nombre de services healthy
    pub healthy_count: usize,

    /// Nombre de services degraded
    pub degraded_count: usize,

    /// Nombre de services unhealthy
    pub unhealthy_count: usize,

    /// Temps de réponse moyen (microsecondes)
    pub avg_response_time_us: u64,
}

impl HealthStats {
    /// Taux de services healthy
    pub fn health_rate(&self) -> f64 {
        if self.total_services == 0 {
            0.0
        } else {
            self.healthy_count as f64 / self.total_services as f64
        }
    }

    /// Taux de disponibilité (healthy + degraded)
    pub fn availability_rate(&self) -> f64 {
        if self.total_services == 0 {
            0.0
        } else {
            (self.healthy_count + self.degraded_count) as f64 / self.total_services as f64
        }
    }
}

impl fmt::Display for HealthStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Health: {}/{} ({:.1}%), Degraded: {}, Unhealthy: {}, Avg: {}μs",
            self.healthy_count,
            self.total_services,
            self.health_rate() * 100.0,
            self.degraded_count,
            self.unhealthy_count,
            self.avg_response_time_us
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ServiceInfo;

    #[test]
    fn test_health_config() {
        let config = HealthConfig::new()
            .with_check_interval(60)
            .with_ping_timeout(2000)
            .with_max_failures(5);

        assert_eq!(config.check_interval_secs, 60);
        assert_eq!(config.ping_timeout_ms, 2000);
        assert_eq!(config.max_failures, 5);
    }

    #[test]
    fn test_health_status() {
        assert!(HealthStatus::Healthy.is_ok());
        assert!(HealthStatus::Degraded.is_ok());
        assert!(!HealthStatus::Unhealthy.is_ok());

        assert!(HealthStatus::Unhealthy.is_down());
        assert!(!HealthStatus::Healthy.is_down());
    }

    #[test]
    fn test_health_check_result() {
        let name = ServiceName::new("test").unwrap();

        let result = HealthCheckResult::healthy(name.clone(), 500, 1000);
        assert_eq!(result.status, HealthStatus::Healthy);
        assert_eq!(result.response_time_us, 500);
        assert!(result.error_message.is_none());

        let result = HealthCheckResult::unhealthy(name, "timeout".into(), 1000);
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert!(result.error_message.is_some());
    }

    #[test]
    fn test_health_checker() {
        let mut checker = HealthChecker::new();
        let name = ServiceName::new("test_service").unwrap();

        let result = checker.ping(&name);
        assert_eq!(result.status, HealthStatus::Healthy);
    }

    #[test]
    fn test_health_stats() {
        let stats = HealthStats {
            total_services: 10,
            healthy_count: 8,
            degraded_count: 1,
            unhealthy_count: 1,
            avg_response_time_us: 500,
        };

        assert_eq!(stats.health_rate(), 0.8);
        assert_eq!(stats.availability_rate(), 0.9);
    }

    #[test]
    fn test_health_stats_display() {
        let stats = HealthStats {
            total_services: 10,
            healthy_count: 8,
            degraded_count: 1,
            unhealthy_count: 1,
            avg_response_time_us: 500,
        };

        let display = format!("{}", stats);
        assert!(display.contains("8/10"));
        assert!(display.contains("80.0%"));
    }
}
