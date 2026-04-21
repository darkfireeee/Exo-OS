#![no_std]
#![no_main]

//! # memory_server — politique mémoire Ring 1
//!
//! Ce serveur expose un plan de contrôle mémoire cohérent avec GI-06 :
//! - quotas par PID ;
//! - régions anonymes backed localement par `mmap` ;
//! - handles opaques plutôt que des adresses physiques ;
//! - sous-service SHM de contrôle pour les régions partageables.

use core::panic::PanicInfo;

use spin::Mutex;

mod allocator;
mod ipc_bridge;
mod mmap_service;
mod shm_server;

use ipc_bridge::{
    register_endpoint, send_heartbeat, send_reply, MemoryReply, MemoryRequest, MEMORY_MSG_ALLOC,
    MEMORY_MSG_FREE, MEMORY_MSG_HEARTBEAT, MEMORY_MSG_PROTECT, MEMORY_MSG_QUERY,
    MEMORY_MSG_QUOTA_QUERY, MEMORY_MSG_QUOTA_SET, MEMORY_MSG_SHM_ATTACH, MEMORY_MSG_SHM_CREATE,
    MEMORY_MSG_SHM_DESTROY,
};
use mmap_service::MemoryService;

static MEMORY_SERVICE: Mutex<MemoryService> = Mutex::new(MemoryService::new());

#[no_mangle]
pub extern "C" fn _start() -> ! {
    register_endpoint();

    let mut request = MemoryRequest::zeroed();

    loop {
        match ipc_bridge::recv_request(&mut request) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(_) => continue,
        }

        let reply = dispatch(&request);
        let _ = send_reply(request.sender_pid, &reply);
    }
}

fn dispatch(request: &MemoryRequest) -> MemoryReply {
    if request.msg_type == MEMORY_MSG_HEARTBEAT {
        return send_heartbeat();
    }

    let mut service = MEMORY_SERVICE.lock();

    match request.msg_type {
        MEMORY_MSG_ALLOC => service.handle_alloc(request.sender_pid, &request.payload),
        MEMORY_MSG_FREE => service.handle_free(request.sender_pid, &request.payload),
        MEMORY_MSG_PROTECT => service.handle_protect(request.sender_pid, &request.payload),
        MEMORY_MSG_QUERY => service.handle_query(request.sender_pid, &request.payload),
        MEMORY_MSG_SHM_CREATE => shm_server::handle_create(&mut service, request.sender_pid, &request.payload),
        MEMORY_MSG_SHM_ATTACH => shm_server::handle_attach(&mut service, request.sender_pid, &request.payload),
        MEMORY_MSG_SHM_DESTROY => shm_server::handle_destroy(&mut service, request.sender_pid, &request.payload),
        MEMORY_MSG_QUOTA_SET => service.handle_quota_set(request.sender_pid, &request.payload),
        MEMORY_MSG_QUOTA_QUERY => service.handle_quota_query(request.sender_pid, &request.payload),
        _ => MemoryReply::error(exo_syscall_abi::EINVAL),
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        // SAFETY: panic terminale pour un serveur no_std monothread.
        unsafe { core::arch::asm!("hlt", options(nostack, nomem)); }
    }
}
