use super::Service;

#[inline]
pub fn service_started(services: &[Service], name: &str) -> bool {
    services
        .iter()
        .find(|service| service.name == name)
        .map(|service| service.current_pid() != 0)
        .unwrap_or(false)
}

#[inline]
pub fn service_index_by_pid(services: &[Service], pid: u32) -> Option<usize> {
    services
        .iter()
        .position(|service| service.current_pid() == pid)
}

#[inline]
pub fn running_count(services: &[Service]) -> usize {
    services
        .iter()
        .filter(|service| service.current_pid() != 0)
        .count()
}
