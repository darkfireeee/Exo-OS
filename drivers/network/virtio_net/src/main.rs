#![no_std]
#![no_main]

use core::panic::PanicInfo;

use exo_syscall_abi as syscall;

mod net;

const SERVER_ENDPOINT_ID: u64 = 14;

#[repr(C)]
struct DriverRequest {
    sender_pid: u32,
    msg_type: u32,
    payload: [u8; syscall::IPC_INLINE_PAYLOAD_SIZE],
}

impl DriverRequest {
    const fn zeroed() -> Self {
        Self {
            sender_pid: 0,
            msg_type: 0,
            payload: [0; syscall::IPC_INLINE_PAYLOAD_SIZE],
        }
    }
}

static mut VIRTIO_NET: net::VirtioNet = net::VirtioNet::new();

#[no_mangle]
pub extern "C" fn _start() -> ! {
    register_endpoint();
    let mut request = DriverRequest::zeroed();
    loop {
        let rc = recv(&mut request);
        if rc <= 0 {
            continue;
        }
        unsafe {
            match request.msg_type {
                net::NET_CTRL_DRIVER_INIT if request.payload.len() >= 32 => {
                    let init = core::ptr::read_unaligned(
                        request.payload.as_ptr() as *const net::DriverInitMsg
                    );
                    VIRTIO_NET.apply_driver_init(init);
                }
                net::NET_CTRL_RX_RELEASE if request.payload.len() >= 48 => {
                    let msg = core::ptr::read_unaligned(
                        request.payload.as_ptr() as *const net::RxReleaseMsg
                    );
                    let _ = VIRTIO_NET.process_rx_releases(&msg);
                }
                net::NET_CTRL_MAC_QUERY => {}
                _ => {}
            }
            let _ = VIRTIO_NET.flush_tx();
        }
    }
}

fn register_endpoint() {
    let name = b"virtio_net";
    unsafe {
        let _ = syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            SERVER_ENDPOINT_ID,
        );
    }
}

fn recv(request: &mut DriverRequest) -> i64 {
    unsafe {
        syscall::syscall4(
            syscall::SYS_IPC_RECV,
            SERVER_ENDPOINT_ID,
            request as *mut DriverRequest as u64,
            core::mem::size_of::<DriverRequest>() as u64,
            syscall::IPC_FLAG_TIMEOUT | 100,
        )
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nostack, nomem));
        }
    }
}
