use core::sync::atomic::Ordering;

use super::{dependency, service_manager, service_table, Service};

#[inline]
pub fn dependency_ready(services: &[Service], dep: &str) -> bool {
    dep == "init_server" || service_manager::service_started(services, dep)
}

#[inline]
pub fn can_start(services: &[Service], name: &str) -> bool {
    dependency::dependencies_satisfied(name, |dep| dependency_ready(services, dep))
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
