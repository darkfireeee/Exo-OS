use core::sync::atomic::Ordering;

use super::{protocol, service_manager, supervisor, Service};

fn checkpoint_tag(services: &[Service]) -> [u8; 32] {
    let mut tag = [0u8; 32];
    let mut idx = 0usize;
    while idx < services.len() {
        let pid = services[idx].current_pid();
        let delay = services[idx].restart_delay_ticks.load(Ordering::Relaxed);
        let base = (idx * 3) % 32;
        tag[base] ^= idx as u8;
        tag[(base + 1) % 32] ^= (pid & 0xFF) as u8;
        tag[(base + 2) % 32] ^= ((pid >> 8) as u8) ^ (delay as u8);
        tag[(base + 7) % 32] ^= ((pid >> 16) as u8).wrapping_add((delay >> 8) as u8);
        idx += 1;
    }
    tag
}

pub fn prepare_isolation_reply(services: &[Service]) -> protocol::InitReply {
    let running_count = service_manager::running_count(services) as u32;
    let running_mask = supervisor::running_mask(services);
    let snapshot_tag = checkpoint_tag(services);
    protocol::isolation_reply(&snapshot_tag, running_count, running_mask)
}
