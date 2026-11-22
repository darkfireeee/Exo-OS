use core::sync::atomic::Ordering;

use super::{dependency, service_manager, service_table, Service};

#[inline]
pub fn dependency_ready(services: &[Service], dep: &str) -> bool {
    if dep == "init_server" || service_manager::service_ready(services, dep) {
        return true;
    }
    if let Some(meta) = dependency::metadata(dep) {
        if !meta.critical && service_manager::service_dead(services, dep) {
            return true;
        }
    }
    false
}

#[inline]
pub fn can_start(services: &[Service], name: &str) -> bool {
    let optional = dependency::optional_dependencies(name);
    dependency::dependencies_satisfied(name, |dep| {
        dependency_ready(services, dep) || optional.contains(&dep)
    })
}

#[inline]
pub fn note_child_exit(services: &[Service], pid: u32) -> Option<usize> {
    let idx = service_manager::service_index_by_pid(services, pid)?;
    if services[idx].is_disabled() {
        services[idx].disable();
    } else {
        services[idx].mark_dead();
    }
    Some(idx)
}

#[inline]
pub fn runtime_index_by_name(services: &[Service], raw_name: &[u8]) -> Option<usize> {
    service_table::runtime_index_by_name(services, raw_name)
}

#[inline]
pub fn running_mask(services: &[Service]) -> u64 {
    service_table::runtime_running_mask(services)
}

#[inline]
pub fn restart_delay_ticks(service: &Service) -> u32 {
    service.restart_delay_ticks.load(Ordering::Relaxed)
}

/// Closed ExoPhoenix Strate 1 whitelist (P2): services outside this set belong to Strate 2
/// and are relaunched asynchronously after the 1 s SLA window closes (`t5`).
#[inline]
pub fn is_strate1_service(name: &str) -> bool {
    matches!(
        name,
        "ipc_router"
            | "ipc_broker"
            | "memory_server"
            | "init_server"
            | "vfs_server"
            | "crypto_server"
            | "device_server"
            | "fb_server"
    )
}

/// Marks all stopped/transient Strate 2 services eligible for immediate parallel relaunch
/// without blocking Strate 1 recovery (P2), returning the count scheduled.
pub fn schedule_strate2_async_relaunch(services: &[Service]) -> u32 {
    let mut scheduled = 0u32;
    let mut idx = 0usize;
    while idx < services.len() {
        let svc = &services[idx];
        if !is_strate1_service(svc.name) && !svc.is_disabled() {
            svc.restart_delay_ticks.store(0, Ordering::Release);
            scheduled = scheduled.saturating_add(1);
        }
        idx += 1;
    }
    scheduled
}

/// Returns `true` once all enabled Strate 2 services are alive and ready (`t6` milestone).
pub fn all_strate2_ready(services: &[Service]) -> bool {
    let mut idx = 0usize;
    while idx < services.len() {
        let svc = &services[idx];
        if !is_strate1_service(svc.name) && !svc.is_disabled() && !svc.is_ready() {
            return false;
        }
        idx += 1;
    }
    true
}
