#![no_std]
#![no_main]

use core::panic::PanicInfo;
use exo_syscall_abi as syscall;

const SERVER_ENDPOINT_ID: u64 = 13;
const IPC_RECV_TIMEOUT_MS: u64 = 5_000;

const VIRTIO_MSG_HEARTBEAT: u32 = 0;
const VIRTIO_MSG_STATUS: u32 = 1;

#[repr(C)]
struct VirtioRequest {
    sender_pid: u32,
    msg_type: u32,
    payload: [u8; syscall::IPC_INLINE_PAYLOAD_SIZE],
}

impl VirtioRequest {
    const fn zeroed() -> Self {
        Self {
            sender_pid: 0,
            msg_type: 0,
            payload: [0; syscall::IPC_INLINE_PAYLOAD_SIZE],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VirtioReply {
    status: i64,
    backend_ready: u32,
    queue_count: u32,
    flags: u32,
    _pad: [u8; 44],
}

impl VirtioReply {
    const fn ok() -> Self {
        Self {
            status: 0,
            backend_ready: 1,
            queue_count: 1,
            flags: 0,
            _pad: [0; 44],
        }
    }

    const fn error(status: i64) -> Self {
        Self {
            status,
            backend_ready: 0,
            queue_count: 0,
            flags: 0,
            _pad: [0; 44],
        }
    }
}

const _: () = assert!(core::mem::size_of::<VirtioRequest>() == syscall::IPC_ENVELOPE_SIZE);
const _: () = assert!(core::mem::offset_of!(VirtioRequest, payload) == syscall::IPC_HEADER_SIZE);

fn register_endpoint() {
    let name = b"virtio_drivers";
    let _ = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            SERVER_ENDPOINT_ID,
        )
    };
}

fn recv_request(request: &mut VirtioRequest) -> Result<bool, i64> {
    let rc = unsafe {
        syscall::syscall4(
            syscall::SYS_IPC_RECV,
            SERVER_ENDPOINT_ID,
            request as *mut VirtioRequest as u64,
            core::mem::size_of::<VirtioRequest>() as u64,
            syscall::IPC_FLAG_TIMEOUT | IPC_RECV_TIMEOUT_MS,
        )
    };

    if rc == syscall::ETIMEDOUT {
        return Ok(false);
    }
    if rc < 0 {
        return Err(rc);
    }
    Ok(true)
}

fn send_reply(destination_pid: u32, reply: &VirtioReply) {
    if destination_pid == 0 {
        return;
    }

    let _ = unsafe {
        syscall::syscall6(
            syscall::SYS_IPC_SEND,
            destination_pid as u64,
            reply as *const VirtioReply as u64,
            core::mem::size_of::<VirtioReply>() as u64,
            0,
            0,
            0,
        )
    };
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    register_endpoint();
    let mut request = VirtioRequest::zeroed();

    loop {
        match recv_request(&mut request) {
            Ok(true) => {}
            Ok(false) | Err(_) => continue,
        }

        let reply = match request.msg_type {
            VIRTIO_MSG_HEARTBEAT | VIRTIO_MSG_STATUS => VirtioReply::ok(),
            _ => VirtioReply::error(syscall::EINVAL),
        };
        send_reply(request.sender_pid, &reply);
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
