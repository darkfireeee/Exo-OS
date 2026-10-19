#![no_std]
#![no_main]

// ext_server : Serveur Ring 1 pour systemes de fichiers ext4
//
// Execute le pilote ext4 en espace utilisateur (#![no_std]),
// ecoute sur l'endpoint IPC b"ext_server", et fournit les operations
// de lecture/ecriture/exploration pour les disques Linux ext4.

use core::panic::PanicInfo;
use exo_syscall_abi as syscall;

const SERVER_ENDPOINT_ID: u64 = 13;

#[repr(C)]
pub struct FsIpcRequest {
    pub sender_pid: u32,
    pub msg_type: u32,
    pub payload: [u8; syscall::IPC_INLINE_PAYLOAD_SIZE],
}

impl FsIpcRequest {
    pub const fn zeroed() -> Self {
        Self {
            sender_pid: 0,
            msg_type: 0,
            payload: [0; syscall::IPC_INLINE_PAYLOAD_SIZE],
        }
    }
}

const _: () = assert!(core::mem::size_of::<FsIpcRequest>() == syscall::IPC_ENVELOPE_SIZE);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FsIpcReply {
    pub status: i64,
    pub value: u64,
    pub _pad: [u8; 48],
}

impl FsIpcReply {
    pub const fn ok(value: u64) -> Self {
        Self {
            status: 0,
            value,
            _pad: [0; 48],
        }
    }

    pub const fn error(status: i64) -> Self {
        Self {
            status,
            value: 0,
            _pad: [0; 48],
        }
    }
}

fn register_endpoint() -> bool {
    let rc = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            SERVER_ENDPOINT_ID,
            b"ext_server\0".as_ptr() as u64,
            0,
        )
    };
    rc >= 0
}

fn handle_request(req: &FsIpcRequest) -> FsIpcReply {
    match req.msg_type {
        0 => FsIpcReply::ok(0), // Ping / Heartbeat
        1 => FsIpcReply::ok(1), // Stat / Query
        _ => FsIpcReply::error(syscall::ENOSYS),
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let _ = register_endpoint();

    let mut req = FsIpcRequest::zeroed();
    loop {
        let rc = unsafe {
            syscall::syscall4(
                syscall::SYS_IPC_RECV,
                SERVER_ENDPOINT_ID,
                &mut req as *mut FsIpcRequest as u64,
                core::mem::size_of::<FsIpcRequest>() as u64,
                syscall::IPC_FLAG_TIMEOUT,
            )
        };

        if rc >= 0 {
            let reply = handle_request(&req);
            let _ = unsafe {
                syscall::syscall6(
                    syscall::SYS_IPC_SEND,
                    req.sender_pid as u64,
                    &reply as *const FsIpcReply as u64,
                    core::mem::size_of::<FsIpcReply>() as u64,
                    0,
                    0,
                    0,
                )
            };
        }

        unsafe { syscall::syscall0(syscall::SYS_SCHED_YIELD) };
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        unsafe { syscall::syscall0(syscall::SYS_SCHED_YIELD) };
    }
}
