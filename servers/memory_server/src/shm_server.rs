use crate::ipc_bridge::MemoryReply;
use crate::mmap_service::MemoryService;

pub fn handle_create(service: &mut MemoryService, sender_pid: u32, payload: &[u8]) -> MemoryReply {
    service.create_shared_region(sender_pid, payload)
}

pub fn handle_attach(service: &mut MemoryService, sender_pid: u32, payload: &[u8]) -> MemoryReply {
    service.attach_shared_region(sender_pid, payload)
}

pub fn handle_destroy(service: &mut MemoryService, sender_pid: u32, payload: &[u8]) -> MemoryReply {
    service.destroy_shared_region(sender_pid, payload)
}
