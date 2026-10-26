#![no_std]
#![no_main]

//! # `virtio_drivers` — Serveur Ring 1 VirtIO Control Plane & VirtIO-GPU (`0x1AF4:0x1050`)
//!
//! Expose :
//! - `VIRTIO_MSG_HEARTBEAT` (`0`) & `VIRTIO_MSG_STATUS` (`1`) pour `init_server`
//! - `VIRTIO_GPU_MSG_TRANSFER_FLUSH` (`2`) : `VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D` + `RESOURCE_FLUSH` sur `controlq`
//! - `VIRTIO_GPU_MSG_MOVE_CURSOR` (`3`) : `VIRTIO_GPU_CMD_MOVE_CURSOR` sur `cursorq`

use core::panic::PanicInfo;
use exo_syscall_abi as syscall;
use exo_virtio_gpu::VirtioGpuDevice;

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
    const fn ok(queue_count: u32, flags: u32) -> Self {
        Self {
            status: 0,
            backend_ready: 1,
            queue_count,
            flags,
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

#[inline]
fn read_u32_le(payload: &[u8], offset: usize) -> u32 {
    if offset + 4 > payload.len() {
        return 0;
    }
    u32::from_le_bytes([
        payload[offset],
        payload[offset + 1],
        payload[offset + 2],
        payload[offset + 3],
    ])
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    register_endpoint();
    let mut gpu = VirtioGpuDevice::new();
    let gpu_hw = gpu.probe_and_init();
    let queue_count = if gpu_hw { 2 } else { 1 };
    let gpu_flags = if gpu_hw { 1 } else { 0 };

    let mut request = VirtioRequest::zeroed();

    loop {
        match recv_request(&mut request) {
            Ok(true) => {}
            Ok(false) | Err(_) => continue,
        }

        let reply = match request.msg_type {
            VIRTIO_MSG_HEARTBEAT | VIRTIO_MSG_STATUS => VirtioReply::ok(queue_count, gpu_flags),
            syscall::VIRTIO_GPU_MSG_TRANSFER_FLUSH => {
                let x = read_u32_le(&request.payload, 0);
                let y = read_u32_le(&request.payload, 4);
                let w = read_u32_le(&request.payload, 8);
                let h = read_u32_le(&request.payload, 12);
                let rc = gpu.transfer_and_flush(x, y, w, h);
                if rc == 0 {
                    VirtioReply::ok(queue_count, gpu_flags)
                } else {
                    VirtioReply::error(rc)
                }
            }
            syscall::VIRTIO_GPU_MSG_MOVE_CURSOR => {
                let x = read_u32_le(&request.payload, 0);
                let y = read_u32_le(&request.payload, 4);
                let rc = gpu.move_cursor(x, y);
                if rc == 0 {
                    VirtioReply::ok(queue_count, gpu_flags)
                } else {
                    VirtioReply::error(rc)
                }
            }
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
