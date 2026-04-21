//! Dépendances de démarrage Ring1 pour `init_server`.
//!
//! Source canonique :
//! `ExoOS_Arborescence_V4(Phase server Et Lib).md`
//! + `ExoOS_Architecture_v7.md`

use super::service_table::{self, ServiceMetadata};

#[inline]
pub fn metadata(name: &str) -> Option<&'static ServiceMetadata> {
    service_table::metadata(name)
}

#[inline]
pub fn ready_timeout_ms(name: &str) -> u64 {
    metadata(name)
        .map(|service| service.ready_timeout_ms)
        .unwrap_or(250)
}

#[inline]
pub fn dependencies_satisfied<F>(name: &str, mut has_service: F) -> bool
where
    F: FnMut(&str) -> bool,
{
    let Some(service) = metadata(name) else {
        return false;
    };

    service.requires.iter().copied().all(|dep| has_service(dep))
}

#[allow(dead_code)]
#[inline]
pub fn is_critical(name: &str) -> bool {
    metadata(name).map(|service| service.critical).unwrap_or(false)
}
