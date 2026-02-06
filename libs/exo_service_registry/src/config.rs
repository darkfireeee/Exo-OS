//! Configuration file loading - TOML parser
//!
//! Parse et gère les fichiers de configuration pour le registry daemon
//! et les clients.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::boxed::Box;
use core::str::FromStr;

use crate::RegistryConfig;

#[cfg(feature = "ipc")]
use crate::daemon::DaemonConfig;

/// Configuration complète du système registry
#[derive(Debug, Clone)]
pub struct SystemConfig {
    /// Configuration du registry core
    pub registry: RegistryConfig,

    /// Configuration du daemon (si IPC enabled)
    #[cfg(feature = "ipc")]
    pub daemon: DaemonConfig,

    /// Configuration du storage
    pub storage: StorageConfig,

    /// Configuration IPC
    pub ipc: IpcConfig,

    /// Configuration health check
    pub health: HealthConfig,
}

/// Configuration du storage backend
#[derive(Debug, Clone)]
pub struct StorageConfig {
    /// Type de backend ("memory" ou "toml")
    pub backend_type: String,

    /// Chemin du fichier TOML (si backend = toml)
    pub toml_path: String,
}

/// Configuration IPC
#[derive(Debug, Clone)]
pub struct IpcConfig {
    /// Chemin du socket Unix
    pub socket_path: String,

    /// Nombre max de connexions
    pub max_connections: usize,

    /// Timeout en millisecondes
    pub timeout_ms: u64,
}

/// Configuration health check
#[derive(Debug, Clone)]
pub struct HealthConfig {
    /// Activer le health checking
    pub enabled: bool,

    /// Intervalle entre checks (secondes)
    pub check_interval_secs: u64,

    /// Timeout pour ping (millisecondes)
    pub ping_timeout_ms: u64,

    /// Max failures avant marquer unhealthy
    pub max_failures: u32,

    /// Auto-recovery activé
    pub auto_recovery: bool,
}

impl Default for SystemConfig {
    fn default() -> Self {
        Self {
            registry: RegistryConfig::new(),
            #[cfg(feature = "ipc")]
            daemon: DaemonConfig::default(),
            storage: StorageConfig::default(),
            ipc: IpcConfig::default(),
            health: HealthConfig::default(),
        }
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            backend_type: "memory".into(),
            toml_path: "/var/lib/exo/registry.toml".into(),
        }
    }
}

impl Default for IpcConfig {
    fn default() -> Self {
        Self {
            socket_path: "/var/run/exo/registry.sock".into(),
            max_connections: 100,
            timeout_ms: 5000,
        }
    }
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            check_interval_secs: 30,
            ping_timeout_ms: 1000,
            max_failures: 3,
            auto_recovery: true,
        }
    }
}

/// Simple TOML parser pour no_std
///
/// Parse un sous-ensemble de TOML suffisant pour notre config.
pub struct TomlParser {
    lines: Vec<String>,
    current_section: String,
}

impl TomlParser {
    /// Crée un nouveau parser
    pub fn new(content: &str) -> Self {
        let lines: Vec<String> = content
            .lines()
            .map(|l| l.trim().into())
            .filter(|l: &String| !l.is_empty() && !l.starts_with('#'))
            .collect();

        Self {
            lines,
            current_section: String::new(),
        }
    }

    /// Parse le contenu TOML
    pub fn parse(&mut self) -> Result<SystemConfig, &'static str> {
        let mut config = SystemConfig::default();

        for line in &self.lines {
            if line.starts_with('[') && line.ends_with(']') {
                // Section header
                self.current_section = line[1..line.len()-1].into();
            } else if let Some(pos) = line.find('=') {
                // Key = value
                let key = line[..pos].trim();
                let value = line[pos+1..].trim();

                self.apply_config(&mut config, key, value)?;
            }
        }

        Ok(config)
    }

    /// Applique une valeur de config
    fn apply_config(
        &self,
        config: &mut SystemConfig,
        key: &str,
        value: &str,
    ) -> Result<(), &'static str> {
        // Retire les quotes
        let value = value.trim_matches('"').trim_matches('\'');

        match self.current_section.as_str() {
            "registry" => self.apply_registry_config(&mut config.registry, key, value)?,
            #[cfg(feature = "ipc")]
            "daemon" => self.apply_daemon_config(&mut config.daemon, key, value)?,
            "storage" => self.apply_storage_config(&mut config.storage, key, value)?,
            "ipc" => self.apply_ipc_config(&mut config.ipc, key, value)?,
            "health" => self.apply_health_config(&mut config.health, key, value)?,
            _ => (),
        }

        Ok(())
    }

    fn apply_registry_config(
        &self,
        config: &mut RegistryConfig,
        key: &str,
        value: &str,
    ) -> Result<(), &'static str> {
        match key {
            "cache_size" => {
                let size = value.parse().map_err(|_| "Invalid cache_size")?;
                *config = config.clone().with_cache_size(size);
            }
            "cache_ttl_secs" => {
                let ttl = value.parse().map_err(|_| "Invalid cache_ttl_secs")?;
                *config = config.clone().with_cache_ttl(ttl);
            }
            "bloom_size" => {
                let size = value.parse().map_err(|_| "Invalid bloom_size")?;
                *config = config.clone().with_bloom_size(size);
            }
            "stale_threshold_secs" => {
                let threshold = value.parse().map_err(|_| "Invalid stale_threshold_secs")?;
                *config = config.clone().with_stale_threshold(threshold);
            }
            _ => (),
        }
        Ok(())
    }

    #[cfg(feature = "ipc")]
    fn apply_daemon_config(
        &self,
        config: &mut DaemonConfig,
        key: &str,
        value: &str,
    ) -> Result<(), &'static str> {
        match key {
            "max_connections" => {
                let max = value.parse().map_err(|_| "Invalid max_connections")?;
                *config = config.clone().with_max_connections(max);
            }
            "request_queue_size" => {
                let size = value.parse().map_err(|_| "Invalid request_queue_size")?;
                *config = config.clone().with_queue_size(size);
            }
            "verbose" => {
                let verbose = value == "true";
                *config = config.clone().with_verbose(verbose);
            }
            _ => (),
        }
        Ok(())
    }

    fn apply_storage_config(
        &self,
        config: &mut StorageConfig,
        key: &str,
        value: &str,
    ) -> Result<(), &'static str> {
        match key {
            "backend" => config.backend_type = value.into(),
            "path" => config.toml_path = value.into(),
            _ => (),
        }
        Ok(())
    }

    fn apply_ipc_config(
        &self,
        config: &mut IpcConfig,
        key: &str,
        value: &str,
    ) -> Result<(), &'static str> {
        match key {
            "socket_path" => config.socket_path = value.into(),
            "max_connections" => {
                config.max_connections = value.parse()
                    .map_err(|_| "Invalid max_connections")?;
            }
            "timeout_ms" => {
                config.timeout_ms = value.parse()
                    .map_err(|_| "Invalid timeout_ms")?;
            }
            _ => (),
        }
        Ok(())
    }

    fn apply_health_config(
        &self,
        config: &mut HealthConfig,
        key: &str,
        value: &str,
    ) -> Result<(), &'static str> {
        match key {
            "enabled" => config.enabled = value == "true",
            "check_interval_secs" => {
                config.check_interval_secs = value.parse()
                    .map_err(|_| "Invalid check_interval_secs")?;
            }
            "ping_timeout_ms" => {
                config.ping_timeout_ms = value.parse()
                    .map_err(|_| "Invalid ping_timeout_ms")?;
            }
            "max_failures" => {
                config.max_failures = value.parse()
                    .map_err(|_| "Invalid max_failures")?;
            }
            "auto_recovery" => config.auto_recovery = value == "true",
            _ => (),
        }
        Ok(())
    }
}

/// Charge une configuration depuis un fichier TOML
pub fn load_config_from_file(path: &str) -> Result<SystemConfig, &'static str> {
    // Dans un vrai système, on lirait le fichier
    // Pour no_std, on suppose que le contenu est fourni
    Err("File I/O not available in no_std")
}

/// Charge une configuration depuis une string TOML
pub fn load_config_from_str(content: &str) -> Result<SystemConfig, &'static str> {
    let mut parser = TomlParser::new(content);
    parser.parse()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_toml_parser_basic() {
        let toml = r#"
[registry]
cache_size = 500
cache_ttl_secs = 120
bloom_size = 100000

[storage]
backend = "toml"
path = "/var/lib/exo/registry.toml"

[ipc]
socket_path = "/var/run/exo/registry.sock"
max_connections = 200
timeout_ms = 10000

[health]
enabled = true
check_interval_secs = 60
ping_timeout_ms = 2000
max_failures = 5
auto_recovery = true
"#;

        let config = load_config_from_str(toml).unwrap();

        assert_eq!(config.registry.cache_size, 500);
        assert_eq!(config.registry.cache_ttl_secs, 120);
        assert_eq!(config.storage.backend_type, "toml");
        assert_eq!(config.ipc.max_connections, 200);
        assert_eq!(config.health.check_interval_secs, 60);
        assert!(config.health.enabled);
    }

    #[test]
    fn test_default_config() {
        let config = SystemConfig::default();

        assert_eq!(config.registry.cache_size, 100);
        assert_eq!(config.storage.backend_type, "memory");
        assert_eq!(config.ipc.socket_path, "/var/run/exo/registry.sock");
        assert!(config.health.enabled);
    }
}
